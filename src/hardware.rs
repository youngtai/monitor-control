use crate::model::{raw_value, Monitor, Request, Response};
use sha2::{Digest, Sha256};
use std::{
    ffi::CStr,
    io::{BufRead, Write},
    os::fd::FromRawFd,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Info {
    model: [libc::c_char; 64],
    serial: [libc::c_char; 64],
    connector: [libc::c_char; 64],
    edid: [u8; 128],
    bus: libc::c_int,
}
unsafe extern "C" {
    fn mc_init() -> libc::c_int;
    fn mc_error(code: libc::c_int) -> *const libc::c_char;
    fn mc_discover(out: *mut Info, capacity: libc::c_int) -> libc::c_int;
    fn mc_read(index: libc::c_int, current: *mut u16, max: *mut u16) -> libc::c_int;
    fn mc_write(index: libc::c_int, value: u16) -> libc::c_int;
}
fn status(code: i32) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    // libddcutil owns this static, null-terminated error description.
    let ptr = unsafe { mc_error(code) };
    let detail = if ptr.is_null() {
        "Unknown DDC error".into()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };
    Err(format!("{detail} ({code})"))
}
fn text(value: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = value
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().into()
}
struct Hardware {
    monitors: Vec<Monitor>,
}
impl Hardware {
    fn discover(&mut self) -> Result<(), String> {
        self.monitors.clear();
        // All-zero storage is valid for the C ABI struct; C fills up to capacity.
        let mut infos: [Info; 64] = unsafe { std::mem::zeroed() };
        let count = unsafe { mc_discover(infos.as_mut_ptr(), infos.len() as i32) };
        if count < 0 {
            return status(count);
        }
        for info in infos.iter().take(count as usize) {
            let hash = format!("{:x}", Sha256::digest(info.edid));
            let connector = match text(&info.connector) {
                s if s.is_empty() => format!("i2c-{}", info.bus),
                s => s,
            };
            // Include connection identity so identical EDIDs can never select the wrong panel.
            self.monitors.push(Monitor {
                id: format!("{}@{}", &hash[..20], connector),
                name: text(&info.model),
                serial: text(&info.serial),
                connector,
                current: None,
                maximum: None,
                error: None,
            });
        }
        self.read_all();
        Ok(())
    }
    fn read_one(&mut self, index: usize) -> Result<(), String> {
        let (mut current, mut maximum) = (0, 0);
        status(unsafe { mc_read(index as i32, &mut current, &mut maximum) })?;
        if maximum == 0 {
            return Err("Monitor reported a zero brightness range".into());
        }
        self.monitors[index].current = Some(current);
        self.monitors[index].maximum = Some(maximum);
        self.monitors[index].error = None;
        Ok(())
    }
    fn read_all(&mut self) {
        for index in 0..self.monitors.len() {
            if let Err(e) = self.read_one(index) {
                self.monitors[index].current = None;
                self.monitors[index].maximum = None;
                self.monitors[index].error = Some(e);
            }
        }
    }
    fn execute(&mut self, request: Request) -> Response {
        let result = match request {
            Request::Discover => self.discover(),
            Request::Read => {
                self.read_all();
                Ok(())
            }
            Request::Set { id, percent } => self.set(&id, percent),
        };
        Response {
            monitors: self.monitors.clone(),
            error: result.err(),
        }
    }
    fn set(&mut self, id: &str, percent: u8) -> Result<(), String> {
        if percent > 100 {
            return Err("Brightness must be 0–100%".into());
        }
        let index = self
            .monitors
            .iter()
            .position(|m| m.id == id)
            .ok_or("Monitor disconnected; refresh required")?;
        let maximum = self.monitors[index]
            .maximum
            .ok_or("Brightness is unavailable on this monitor")?;
        let result = status(unsafe { mc_write(index as i32, raw_value(percent, maximum)) })
            .and_then(|_| self.read_one(index));
        if let Err(e) = &result {
            self.monitors[index].error = Some(e.clone());
        }
        result
    }
}

pub fn run_worker() -> Result<(), String> {
    // The worker must not survive the session agent, including crashes.
    unsafe {
        let parent = libc::getppid();
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 || libc::getppid() != parent {
            return Err("Cannot attach hardware worker to its parent".into());
        }
    }
    // libddcutil can print diagnostics. Reserve a dedicated duplicate for JSON,
    // and route all native stdout to stderr before initialization.
    let fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut output = unsafe { std::fs::File::from_raw_fd(fd) };
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    status(unsafe { mc_init() })?;
    let mut hardware = Hardware {
        monitors: Vec::new(),
    };
    for line in std::io::stdin().lock().lines() {
        let request =
            serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut output, &hardware.execute(request))
            .map_err(|e| e.to_string())?;
        writeln!(output)
            .and_then(|_| output.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
