use crate::model::{Request, Response};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub trait Transport: Send {
    fn request(&mut self, request: &Request) -> Result<Response, String>;
}

pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    responses: mpsc::Receiver<Result<Response, String>>,
}
impl Worker {
    pub fn spawn() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = Command::new(executable);
        command.arg("--worker");
        Self::start(command)
    }
    fn start(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("Worker input is unavailable")?;
        let stdout = child.stdout.take().ok_or("Worker output is unavailable")?;
        let (tx, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let response = line.map_err(|e| e.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|e| format!("Invalid worker response: {e}"))
                });
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            responses,
        })
    }
    pub fn request(&mut self, request: &Request) -> Result<Response, String> {
        self.request_with_timeout(request, Duration::from_secs(30))
    }
    fn request_with_timeout(
        &mut self,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, String> {
        serde_json::to_writer(&mut self.stdin, request).map_err(|e| e.to_string())?;
        writeln!(self.stdin)
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())?;
        self.responses
            .recv_timeout(timeout)
            .map_err(|e| format!("Hardware worker stopped responding: {e}"))?
    }
}
impl Transport for Worker {
    fn request(&mut self, request: &Request) -> Result<Response, String> {
        Worker::request(self, request)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Reap off the controller thread: an uninterruptible kernel call must
        // never block the UI, shutdown, or subsequent error reporting.
        if self.child.try_wait().ok().flatten().is_none() {
            let pid = self.child.id() as libc::pid_t;
            std::thread::spawn(move || unsafe {
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hung_worker_has_a_deadline() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let mut worker = Worker::start(cmd).unwrap();
        let start = std::time::Instant::now();
        assert!(worker
            .request_with_timeout(&Request::Discover, Duration::from_millis(40))
            .is_err());
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn crashed_worker_returns_an_error() {
        let cmd = Command::new("false");
        let mut worker = Worker::start(cmd).unwrap();
        assert!(worker
            .request_with_timeout(&Request::Discover, Duration::from_secs(1))
            .is_err());
    }
}
