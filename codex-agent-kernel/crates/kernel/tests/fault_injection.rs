use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use codex_agent_kernel::error::KernelError;
use codex_agent_kernel::log::EventLog;
use codex_agent_kernel::reduce_all;
use pretty_assertions::assert_eq;

fn fault_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cak-fault")
}

fn run_fault(cmd: &str, path: &Path) -> std::process::Output {
    Command::new(fault_bin())
        .args([cmd, path.to_str().expect("utf8 path")])
        .output()
        .expect("spawn cak-fault")
}

#[test]
fn hard_kill_during_append_leaves_openable_or_explicit_corruption() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..25 {
        let path = dir.path().join(format!("kill-{i}.cak"));
        let mut child = Command::new(fault_bin())
            .args(["write-loop", path.to_str().unwrap()])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(20 + (i % 5) * 5));
        let _ = child.kill();
        let _ = child.wait();
        if !path.exists() {
            continue;
        }
        match EventLog::open(&path) {
            Ok(log) => {
                reduce_all(log.records()).unwrap();
            }
            Err(KernelError::LogCorrupt { .. })
            | Err(KernelError::ChecksumMismatch)
            | Err(KernelError::LogIntegrity(_))
            | Err(KernelError::Io(_)) => {}
            Err(other) => panic!("unexpected open error after kill: {other}"),
        }
    }
}

#[test]
fn stale_worker_commit_is_rejected_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lease.cak");
    let setup = run_fault("setup-lease", &path);
    assert!(
        setup.status.success(),
        "setup-lease failed: {}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let winner = run_fault("expire-commit", &path);
    assert!(
        winner.status.success(),
        "expire-commit failed: {}",
        String::from_utf8_lossy(&winner.stderr)
    );
    let stale = run_fault("stale-commit", &path);
    assert!(
        !stale.status.success(),
        "stale worker commit must be rejected"
    );
    let log = EventLog::open(&path).unwrap();
    let state = reduce_all(log.records()).unwrap();
    let exec = state.executions.values().next().unwrap();
    let op = exec.operations.values().next().unwrap();
    assert_eq!(op.status, codex_agent_kernel::OperationStatus::Succeeded);
    assert_eq!(op.lease.as_ref().unwrap().generation, 2);
}
