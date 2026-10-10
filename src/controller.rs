use crate::{
    model::{Monitor, Request},
    worker::{Transport, Worker},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub monitors: Vec<Monitor>,
    pub desired: BTreeMap<String, u8>,
    pub busy: bool,
    pub error: Option<String>,
}
impl Snapshot {
    pub fn percent(&self, m: &Monitor) -> Option<u8> {
        self.desired.get(&m.id).copied().or_else(|| m.percent())
    }
}
#[derive(Default)]
struct Pending {
    writes: BTreeMap<String, u8>,
    discover: bool,
    read: bool,
    due: Option<Instant>,
}
struct State {
    snapshot: Snapshot,
    pending: Pending,
    epoch: u64,
    retries: usize,
    stopped: bool,
    sleeping: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    tx: async_channel::Sender<Snapshot>,
}
#[derive(Clone)]
pub struct Controller(Arc<Shared>);
impl Controller {
    pub fn new(tx: async_channel::Sender<Snapshot>) -> Self {
        Self::with_factory(tx, || {
            Worker::spawn().map(|w| Box::new(w) as Box<dyn Transport>)
        })
    }
    fn with_factory(
        tx: async_channel::Sender<Snapshot>,
        factory: impl Fn() -> Result<Box<dyn Transport>, String> + Send + 'static,
    ) -> Self {
        let this = Self(Arc::new(Shared {
            state: Mutex::new(State {
                snapshot: Snapshot::default(),
                pending: Pending::default(),
                epoch: 0,
                retries: 0,
                stopped: false,
                sleeping: false,
            }),
            wake: Condvar::new(),
            tx,
        }));
        let worker = this.clone();
        std::thread::spawn(move || worker.work(factory));
        this.refresh(true);
        this
    }
    fn publish(&self, state: &State) {
        let _ = self.0.tx.try_send(state.snapshot.clone());
    }
    pub fn refresh(&self, topology: bool) {
        let mut state = self.0.state.lock().unwrap();
        if topology
            || (!state.snapshot.busy
                && (state.snapshot.monitors.is_empty()
                    || state.snapshot.monitors.iter().any(|m| m.maximum.is_none())))
        {
            state.epoch += 1;
            state.pending.writes.clear();
            state.snapshot.desired.clear();
            state.pending.discover = true;
            state.retries = 0;
            // Disable stale handles as soon as the topology signal arrives.
            state.snapshot.busy = true;
        } else {
            state.retries = 0;
            state.pending.read = true;
        }
        state.pending.due = Some(Instant::now() + Duration::from_millis(300));
        self.publish(&state);
        self.0.wake.notify_one();
    }
    pub fn set(&self, id: &str, percent: u8) {
        let mut state = self.0.state.lock().unwrap();
        if state.snapshot.busy
            || !state
                .snapshot
                .monitors
                .iter()
                .any(|m| m.id == id && m.maximum.is_some())
        {
            return;
        }
        let value = percent.min(100);
        state.pending.writes.insert(id.into(), value);
        state.snapshot.desired.insert(id.into(), value);
        state.pending.due = Some(Instant::now() + Duration::from_millis(90));
        self.publish(&state);
        self.0.wake.notify_one();
    }
    pub fn adjust(&self, delta: i32, excluded: &[String]) {
        let mut state = self.0.state.lock().unwrap();
        if state.snapshot.busy {
            return;
        }
        let values: Vec<_> = state
            .snapshot
            .monitors
            .iter()
            .filter(|m| !excluded.contains(&m.id))
            .filter_map(|m| {
                state
                    .snapshot
                    .percent(m)
                    .map(|p| (m.id.clone(), (i32::from(p) + delta).clamp(0, 100) as u8))
            })
            .collect();
        for (id, percent) in values {
            state.pending.writes.insert(id.clone(), percent);
            state.snapshot.desired.insert(id, percent);
        }
        state.pending.due = Some(Instant::now() + Duration::from_millis(90));
        self.publish(&state);
        self.0.wake.notify_one();
    }
    pub fn sleep(&self, sleeping: bool) {
        {
            let mut state = self.0.state.lock().unwrap();
            state.sleeping = sleeping;
            state.epoch += 1;
            state.pending.writes.clear();
            state.snapshot.desired.clear();
            state.snapshot.busy = sleeping;
            self.publish(&state);
        }
        if !sleeping {
            self.refresh(true);
        }
    }
    pub fn stop(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.stopped = true;
        self.0.wake.notify_one();
    }
    fn work(&self, factory: impl Fn() -> Result<Box<dyn Transport>, String>) {
        let mut worker: Option<Box<dyn Transport>> = None;
        loop {
            let (request, epoch) = {
                let mut state = self.0.state.lock().unwrap();
                loop {
                    if state.stopped {
                        return;
                    }
                    if state.sleeping {
                        state = self.0.wake.wait(state).unwrap();
                        continue;
                    }
                    if let Some(due) = state.pending.due {
                        if let Some(delay) = due.checked_duration_since(Instant::now()) {
                            state = self.0.wake.wait_timeout(state, delay).unwrap().0;
                            continue;
                        }
                        break;
                    }
                    state = self.0.wake.wait(state).unwrap();
                }
                state.pending.due = None;
                let request = if state.pending.discover || worker.is_none() {
                    state.pending.discover = false;
                    state.pending.read = false;
                    state.snapshot.busy = true;
                    Request::Discover
                } else if let Some((id, percent)) = state.pending.writes.pop_first() {
                    Request::Set { id, percent }
                } else if state.pending.read {
                    state.pending.read = false;
                    Request::Read
                } else {
                    continue;
                };
                self.publish(&state);
                (request, state.epoch)
            };
            let result = (|| {
                if worker.is_none() {
                    worker = Some(factory()?);
                }
                worker.as_mut().unwrap().request(&request)
            })();
            let failed_worker = result.is_err();
            if failed_worker {
                worker = None;
            }
            let mut state = self.0.state.lock().unwrap();
            // A hotplug during a call invalidates the result and every queued write.
            if state.epoch != epoch {
                continue;
            }
            state.snapshot.busy = false;
            match result {
                Ok(response) => {
                    state.snapshot.monitors = response.monitors;
                    state.snapshot.error = response.error;
                }
                Err(error) => {
                    state.snapshot.error = Some(error);
                    for m in &mut state.snapshot.monitors {
                        m.current = None;
                        m.maximum = None;
                    }
                    state.pending.writes.clear();
                    state.snapshot.desired.clear();
                }
            }
            if let Request::Set { id, percent } = &request {
                if state.snapshot.desired.get(id) == Some(percent)
                    && !state.pending.writes.contains_key(id)
                {
                    state.snapshot.desired.remove(id);
                }
            }
            let unavailable = state.snapshot.monitors.is_empty()
                || state.snapshot.monitors.iter().any(|m| m.error.is_some());
            if (failed_worker || state.snapshot.error.is_some() || unavailable) && state.retries < 3
            {
                let seconds = [1, 3, 8][state.retries];
                state.retries += 1;
                state.pending.discover = true;
                state.pending.due = Some(Instant::now() + Duration::from_secs(seconds));
            }
            if !state.pending.writes.is_empty()
                || state.pending.read
                || state.pending.discover && state.pending.due.is_none()
            {
                state.pending.due.get_or_insert(Instant::now());
            }
            self.publish(&state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Response;
    use std::sync::mpsc;
    struct Fake {
        log: mpsc::Sender<Request>,
        value: u16,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
    }
    impl Transport for Fake {
        fn request(&mut self, request: &Request) -> Result<Response, String> {
            let _ = self.log.send(request.clone());
            if let Request::Set { percent, .. } = request {
                if let Some(gate) = self.gate.take() {
                    let (lock, cv) = &*gate;
                    let mut open = lock.lock().unwrap();
                    while !*open {
                        open = cv.wait(open).unwrap();
                    }
                }
                self.value = u16::from(*percent);
            }
            Ok(Response {
                monitors: vec![Monitor {
                    id: "panel".into(),
                    name: "Test".into(),
                    serial: "123".into(),
                    connector: "DP-1".into(),
                    current: Some(self.value),
                    maximum: Some(100),
                    error: None,
                }],
                error: None,
            })
        }
    }
    fn setup(gate: Option<Arc<(Mutex<bool>, Condvar)>>) -> (Controller, mpsc::Receiver<Request>) {
        let (tx, _) = async_channel::unbounded();
        let (log, rx) = mpsc::channel();
        let controller = Controller::with_factory(tx, move || {
            Ok(Box::new(Fake {
                log: log.clone(),
                value: 40,
                gate: gate.clone(),
            }))
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Request::Discover
        ));
        // Wait for the discovery response to be committed, without timing assumptions.
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller
            .0
            .state
            .lock()
            .unwrap()
            .snapshot
            .monitors
            .is_empty()
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        (controller, rx)
    }
    #[test]
    fn key_repeats_accumulate_and_coalesce_without_rescans() {
        let (c, log) = setup(None);
        for _ in 0..8 {
            c.adjust(5, &[]);
        }
        match log.recv_timeout(Duration::from_secs(2)).unwrap() {
            Request::Set { percent, .. } => assert_eq!(percent, 80),
            other => panic!("Unexpected hardware request: {other:?}"),
        }
        assert!(log.recv_timeout(Duration::from_millis(200)).is_err());
        c.stop();
    }
    #[test]
    fn excluded_monitors_are_not_adjusted() {
        let (c, log) = setup(None);
        c.adjust(5, &["panel".into()]);
        assert!(log.recv_timeout(Duration::from_millis(200)).is_err());
        c.stop();
    }
    #[test]
    fn hotplug_discards_inflight_results_and_queued_writes() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (c, log) = setup(Some(gate.clone()));
        c.set("panel", 50);
        assert!(matches!(
            log.recv_timeout(Duration::from_secs(2)).unwrap(),
            Request::Set { percent: 50, .. }
        ));
        c.set("panel", 90);
        c.refresh(true);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(matches!(
            log.recv_timeout(Duration::from_secs(2)).unwrap(),
            Request::Discover
        ));
        assert!(log.recv_timeout(Duration::from_millis(200)).is_err());
        c.stop();
    }
    #[test]
    fn suspend_stops_hardware_requests_until_resume() {
        let (c, log) = setup(None);
        c.sleep(true);
        c.set("panel", 60);
        c.refresh(true);
        assert!(log.recv_timeout(Duration::from_millis(200)).is_err());
        c.sleep(false);
        assert!(matches!(
            log.recv_timeout(Duration::from_secs(2)).unwrap(),
            Request::Discover
        ));
        c.stop();
    }
}

#[cfg(test)]
mod multi_monitor_tests {
    use super::*;
    use crate::model::{raw_value, Response};
    use std::sync::mpsc;
    struct Multi {
        monitors: Vec<Monitor>,
        log: mpsc::Sender<Request>,
    }
    impl Transport for Multi {
        fn request(&mut self, request: &Request) -> Result<Response, String> {
            let _ = self.log.send(request.clone());
            if let Request::Set { id, percent } = request {
                let m = self.monitors.iter_mut().find(|m| &m.id == id).unwrap();
                m.current = Some(raw_value(*percent, m.maximum.unwrap()));
            }
            Ok(Response {
                monitors: self.monitors.clone(),
                error: None,
            })
        }
    }
    #[test]
    fn group_controls_keep_individual_levels_and_hardware_ranges() {
        let (tx, _) = async_channel::unbounded();
        let (log, rx) = mpsc::channel();
        let c = Controller::with_factory(tx, move || {
            Ok(Box::new(Multi {
                log: log.clone(),
                monitors: [("left", 20, 100), ("right", 153, 255)]
                    .into_iter()
                    .map(|(id, value, max)| Monitor {
                        id: id.into(),
                        name: id.into(),
                        serial: id.into(),
                        connector: id.into(),
                        current: Some(value),
                        maximum: Some(max),
                        error: None,
                    })
                    .collect(),
            }))
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Request::Discover
        ));
        let deadline = Instant::now() + Duration::from_secs(2);
        while c.0.state.lock().unwrap().snapshot.monitors.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        c.adjust(5, &[]);
        let mut requested = BTreeMap::new();
        for _ in 0..2 {
            if let Request::Set { id, percent } = rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                requested.insert(id, percent);
            }
        }
        assert_eq!(requested.get("left"), Some(&25));
        assert_eq!(requested.get("right"), Some(&65));
        c.adjust(-5, &["right".into()]);
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Request::Set { id, percent: 20 } if id == "left")
        );
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        c.stop();
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::model::Response;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    struct Unreliable {
        fail: bool,
        log: mpsc::Sender<bool>,
    }
    impl Transport for Unreliable {
        fn request(&mut self, _: &Request) -> Result<Response, String> {
            self.log.send(self.fail).unwrap();
            if self.fail {
                return Err("Simulated process exit".into());
            }
            Ok(Response {
                monitors: vec![Monitor {
                    id: "panel".into(),
                    name: "Test".into(),
                    serial: "1".into(),
                    connector: "DP-1".into(),
                    current: Some(30),
                    maximum: Some(100),
                    error: None,
                }],
                error: None,
            })
        }
    }
    #[test]
    fn failed_worker_is_replaced_and_discovered_before_reuse() {
        let (tx, _) = async_channel::unbounded();
        let (log, rx) = mpsc::channel();
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let c = Controller::with_factory(tx, move || {
            Ok(Box::new(Unreliable {
                fail: count.fetch_add(1, Ordering::SeqCst) == 0,
                log: log.clone(),
            }))
        });
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(!rx.recv_timeout(Duration::from_secs(3)).unwrap());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        c.stop();
    }
}
