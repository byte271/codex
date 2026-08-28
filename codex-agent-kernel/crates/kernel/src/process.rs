use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::error::KernelError;
use crate::event::StreamKind;
use crate::ids::{AttemptId, OperationId};

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub operation_id: OperationId,
    pub attempt_id: AttemptId,
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone)]
pub enum ProcessReport {
    Started {
        operation_id: OperationId,
        attempt_id: AttemptId,
        pid: u32,
        start_key: String,
    },
    Output {
        operation_id: OperationId,
        attempt_id: AttemptId,
        stream: StreamKind,
        offset: u64,
        bytes: Vec<u8>,
    },
    Exited {
        operation_id: OperationId,
        attempt_id: AttemptId,
        exit_code: i32,
        signal: Option<String>,
        killed_externally: bool,
    },
    Failed {
        operation_id: OperationId,
        attempt_id: AttemptId,
        reason: String,
    },
    /// Liveness could not be proven after restart. Not an exit code.
    Unresolved {
        operation_id: OperationId,
        attempt_id: AttemptId,
        reason: String,
    },
}

/// Result of reconciling a persisted `start_key` against the OS.
///
/// `Alive` is returned only when process *identity* is proven. Existence of a
/// PID without identity is `Unknown`. Callers must not invent an exit code for
/// `Unknown` or `Dead`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessLiveness {
    Alive,
    Dead,
    Unknown,
}

pub struct ProcessSupervisor {
    tx: Sender<ProcessReport>,
    rx: Receiver<ProcessReport>,
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    pub fn try_recv(&self) -> Result<Option<ProcessReport>, KernelError> {
        match self.rx.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(KernelError::Process("supervisor channel closed".into()))
            }
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<ProcessReport>, KernelError> {
        match self.rx.recv_timeout(timeout) {
            Ok(msg) => Ok(Some(msg)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(KernelError::Process("supervisor channel closed".into()))
            }
        }
    }

    pub fn spawn(&self, spec: LaunchSpec) -> Result<u32, KernelError> {
        let tx = self.tx.clone();
        if spec.argv.is_empty() {
            return Err(KernelError::Process("empty argv".into()));
        }
        let mut cmd = Command::new(&spec.argv[0]);
        if spec.argv.len() > 1 {
            cmd.args(&spec.argv[1..]);
        }
        if !spec.cwd.is_empty() {
            cmd.current_dir(&spec.cwd);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .map_err(|err| KernelError::Process(err.to_string()))?;
        let pid = child.id();
        let start_key = process_start_key(pid);
        tx.send(ProcessReport::Started {
            operation_id: spec.operation_id,
            attempt_id: spec.attempt_id,
            pid,
            start_key,
        })
        .map_err(|err| KernelError::Process(err.to_string()))?;
        thread::spawn(move || watch_child(child, spec, tx));
        Ok(pid)
    }

    /// Poll a process that survived a runtime restart. This is not Child
    /// reattachment: stdout/stderr and the wait status are already lost.
    pub fn monitor_existing(
        &self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        start_key: String,
    ) {
        let tx = self.tx.clone();
        thread::spawn(move || poll_existing_process(operation_id, attempt_id, start_key, tx));
    }
}

fn poll_existing_process(
    operation_id: OperationId,
    attempt_id: AttemptId,
    start_key: String,
    tx: Sender<ProcessReport>,
) {
    loop {
        match process_liveness(&start_key) {
            ProcessLiveness::Alive => thread::sleep(Duration::from_millis(50)),
            ProcessLiveness::Dead => {
                let _ = tx.send(ProcessReport::Failed {
                    operation_id,
                    attempt_id,
                    reason: "lost-after-restart".into(),
                });
                break;
            }
            ProcessLiveness::Unknown => {
                let _ = tx.send(ProcessReport::Unresolved {
                    operation_id,
                    attempt_id,
                    reason: "process-identity-uncertain-after-restart".into(),
                });
                break;
            }
        }
    }
}

fn watch_child(mut child: Child, spec: LaunchSpec, tx: Sender<ProcessReport>) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut drains = Vec::new();
    if let Some(out) = stdout {
        let tx = tx.clone();
        let spec = spec.clone();
        drains.push(thread::spawn(move || {
            drain_stream(out, StreamKind::Stdout, spec, tx)
        }));
    }
    if let Some(err) = stderr {
        let tx = tx.clone();
        let spec = spec.clone();
        drains.push(thread::spawn(move || {
            drain_stream(err, StreamKind::Stderr, spec, tx)
        }));
    }
    let wait_result = child.wait();
    for drain in drains {
        let _ = drain.join();
    }
    match wait_result {
        Ok(status) => {
            let (exit_code, signal) = decode_status(status);
            let _ = tx.send(ProcessReport::Exited {
                operation_id: spec.operation_id,
                attempt_id: spec.attempt_id,
                exit_code,
                signal,
                killed_externally: false,
            });
        }
        Err(err) => {
            let _ = tx.send(ProcessReport::Failed {
                operation_id: spec.operation_id,
                attempt_id: spec.attempt_id,
                reason: err.to_string(),
            });
        }
    }
}

fn drain_stream(
    mut pipe: impl Read,
    stream: StreamKind,
    spec: LaunchSpec,
    tx: Sender<ProcessReport>,
) {
    let mut offset = 0u64;
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                let _ = tx.send(ProcessReport::Output {
                    operation_id: spec.operation_id,
                    attempt_id: spec.attempt_id,
                    stream,
                    offset,
                    bytes,
                });
                offset += n as u64;
            }
            Err(_) => break,
        }
    }
}

fn decode_status(status: std::process::ExitStatus) -> (i32, Option<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return (128 + sig, Some(sig.to_string()));
        }
    }
    (status.code().unwrap_or(-1), None)
}

pub fn process_start_key(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        if let Some(start) = linux_stat_starttime(pid) {
            return format!("{pid}:{start}");
        }
    }
    format!("{pid}")
}

/// True only when [`process_liveness`] proves identity. Prefer that enum;
/// this wrapper exists for older call sites that meant "still the same process".
pub fn pid_still_matches(start_key: &str) -> bool {
    process_liveness(start_key) == ProcessLiveness::Alive
}

pub fn process_liveness(start_key: &str) -> ProcessLiveness {
    let mut parts = start_key.split(':');
    let Some(pid_str) = parts.next() else {
        return ProcessLiveness::Unknown;
    };
    let Ok(pid) = pid_str.parse::<u32>() else {
        return ProcessLiveness::Unknown;
    };

    #[cfg(target_os = "linux")]
    {
        linux_process_liveness(pid, parts.next())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = parts.next();
        // Never consult /proc outside Linux. PID existence is not identity.
        match pid_exists(pid) {
            Some(true) => ProcessLiveness::Unknown,
            Some(false) => ProcessLiveness::Dead,
            None => ProcessLiveness::Unknown,
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_stat_starttime(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit(' ').nth(19).map(str::to_string)
}

#[cfg(target_os = "linux")]
fn linux_process_liveness(pid: u32, expected_start: Option<&str>) -> ProcessLiveness {
    let proc_dir = format!("/proc/{pid}");
    if !std::path::Path::new(&proc_dir).exists() {
        return ProcessLiveness::Dead;
    }
    let Some(expected) = expected_start else {
        // PID-only keys cannot distinguish reuse.
        return ProcessLiveness::Unknown;
    };
    match linux_stat_starttime(pid) {
        Some(actual) if actual == expected => ProcessLiveness::Alive,
        Some(_) => ProcessLiveness::Dead,
        None => ProcessLiveness::Unknown,
    }
}

/// `Some(true)` if a process with this PID exists, `Some(false)` if it does
/// not, `None` if the check itself is inconclusive.
#[cfg(not(target_os = "linux"))]
fn pid_exists(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        unix_pid_exists(pid)
    }
    #[cfg(windows)]
    {
        windows_pid_exists(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        None
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unix_pid_exists(pid: u32) -> Option<bool> {
    let rc = libc_kill(pid as i32, 0);
    if rc == 0 {
        return Some(true);
    }
    let err = std::io::Error::last_os_error();
    // ESRCH is 3 on POSIX. Prefer the raw code so a missing PID is Dead,
    // not Unknown, even if ErrorKind mapping differs by platform.
    if err.raw_os_error() == Some(3) || err.kind() == std::io::ErrorKind::NotFound {
        Some(false)
    } else {
        None
    }
}

#[cfg(windows)]
fn windows_pid_exists(pid: u32) -> Option<bool> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, exit_code: *mut u32) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let err = std::io::Error::last_os_error();
            return if err.kind() == std::io::ErrorKind::PermissionDenied {
                None
            } else {
                Some(false)
            };
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(code == STILL_ACTIVE)
    }
}

pub fn kill_pid(pid: u32) -> Result<(), KernelError> {
    #[cfg(unix)]
    {
        let rc = libc_kill(pid as i32, 9);
        if rc != 0 {
            return Err(KernelError::Process(format!("kill {pid} failed")));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(KernelError::Process(
            "kill_pid is unix-only in this prototype".into(),
        ))
    }
}

#[cfg(unix)]
fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, sig) }
}

pub fn write_stdin_pid(_pid: u32, _bytes: &[u8]) -> Result<(), KernelError> {
    Err(KernelError::Process(
        "write_stdin is not attached in the first slice; use a single-shot argv".into(),
    ))
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod process_tests;
