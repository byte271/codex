use std::time::Duration;

use codex_agent_kernel::event::OperationStatus;
use codex_agent_kernel::ids::{EventId, LeaseOwnerId};
use codex_agent_kernel::log::EventLog;
use codex_agent_kernel::replay::{fork_from, replay_until};
use codex_agent_kernel::runtime::Runtime;
use codex_agent_kernel::state::{prefix_hashes, state_hash};
use codex_agent_kernel::{
    naive_child_storage_bytes, shared_child_storage_bytes, ContentStore, Kernel, KernelError,
};
use pretty_assertions::assert_eq;

fn echo_argv(msg: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), format!("echo {msg}")]
    } else {
        vec!["/bin/echo".into(), msg.into()]
    }
}

fn true_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), "exit 0".into()]
    } else {
        // macOS GitHub runners have `true` on PATH but not always `/bin/true`.
        vec!["true".into()]
    }
}

fn sleep_argv(seconds: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd".into(),
            "/C".into(),
            format!(
                "ping -n {} 127.0.0.1 >nul",
                seconds.parse::<u32>().unwrap_or(2) + 1
            ),
        ]
    } else {
        vec!["/bin/sleep".into(), seconds.into()]
    }
}

#[test]
fn process_echo_commits_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "echo").unwrap();
    let (goal, _) = rt.kernel.create_goal("echo", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "echo", None, None)
        .unwrap();
    rt.kernel
        .create_operation(
            agent,
            echo_argv("kernel-ok"),
            dir.path().to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
    rt.wait_idle(Duration::from_secs(5)).unwrap();
    let exec = rt.kernel.execution().unwrap();
    let op = exec.operations.values().next().unwrap();
    assert_eq!(op.status, OperationStatus::Succeeded);
    assert_eq!(op.committed_exit_code, Some(0));
}

#[test]
fn silent_and_immediate_exit() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "true").unwrap();
    let (goal, _) = rt.kernel.create_goal("true", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "true", None, None)
        .unwrap();
    rt.kernel
        .create_operation(
            agent,
            true_argv(),
            dir.path().to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
    rt.wait_idle(Duration::from_secs(5)).unwrap();
    let op = rt
        .kernel
        .execution()
        .unwrap()
        .operations
        .values()
        .next()
        .unwrap();
    assert_eq!(op.status, OperationStatus::Succeeded);
}

#[test]
fn huge_output_is_chunked_into_cas() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "huge").unwrap();
    let (goal, _) = rt.kernel.create_goal("huge", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "huge", None, None)
        .unwrap();
    let argv = if cfg!(windows) {
        vec![
            "cmd".into(),
            "/C".into(),
            "fsutil file createnew nul 0".into(),
        ]
    } else {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "dd if=/dev/zero bs=200000 count=1 2>/dev/null".into(),
        ]
    };
    rt.kernel
        .create_operation(agent, argv, dir.path().to_string_lossy().into_owned(), None)
        .unwrap();
    rt.wait_idle(Duration::from_secs(8)).unwrap();
    let op = rt
        .kernel
        .execution()
        .unwrap()
        .operations
        .values()
        .next()
        .unwrap();
    assert!(
        !op.attempts
            .values()
            .next()
            .unwrap()
            .output_chunks
            .is_empty()
            || cfg!(windows)
    );
}

#[test]
fn crash_restart_replays_same_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "crash").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel.start_agent(agent).unwrap();
    rt.kernel.complete_agent(agent).unwrap();
    let (h1, b1) = rt.kernel.hashes().unwrap();
    drop(rt);
    let rt2 = Runtime::open(dir.path()).unwrap();
    let (h2, b2) = rt2.kernel.hashes().unwrap();
    assert_eq!(h1, h2);
    assert_eq!(b1, b2);
}

#[test]
fn torn_write_is_discarded_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.cak");
    {
        let log = EventLog::create(&path).unwrap();
        let mut k = Kernel::from_log(log).unwrap();
        k.create_execution("torn").unwrap();
        k.log.inject_torn_write(b"{\"partial\":true").unwrap();
        // drop without treating torn bytes as a record
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    Kernel::from_log(log).unwrap();
}

#[test]
fn projection_rebuild_matches_reducer() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "proj").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel.start_agent(agent).unwrap();
    rt.kernel.complete_agent(agent).unwrap();
    rt.project().unwrap();
    let exec_id = rt.execution_id().unwrap();
    let from_log = state_hash(rt.kernel.execution().unwrap()).unwrap().to_hex();
    let from_sqlite = rt
        .projection
        .load_execution(exec_id)
        .unwrap()
        .map(|e| state_hash(&e).unwrap().to_hex())
        .unwrap();
    assert_eq!(from_log, from_sqlite);
    rt.projection.corrupt_execution_row(exec_id).unwrap();
    let rebuilt = rt.rebuild_projection_from_scratch().unwrap();
    assert_eq!(rebuilt, from_log);
}

#[test]
fn replay_until_matches_prefix_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "replay").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel.start_agent(agent).unwrap();
    let records = rt.kernel.log.records().to_vec();
    let hashes = prefix_hashes(&records).unwrap();
    let mid = records[2].event_id;
    let report = replay_until(rt.root().join("events.cak"), Some(mid)).unwrap();
    assert_eq!(report.prefix_hashes.last().unwrap().1, hashes[2].1.to_hex());
}

#[test]
fn fork_preserves_business_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "fork").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel.start_agent(agent).unwrap();
    let from = rt.kernel.log.last().unwrap().event_id;
    let dest = dir.path().join("forked");
    let (_id, src, fork) = fork_from(rt.root().join("events.cak"), dest, from).unwrap();
    assert_eq!(src, fork);
}

#[test]
fn unknown_replay_cutoff_returns_event_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "replay-missing").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel.start_agent(agent).unwrap();
    let missing = EventId::new();
    match replay_until(rt.root().join("events.cak"), Some(missing)) {
        Err(KernelError::EventNotFound(id)) => assert_eq!(id, missing.to_string()),
        Err(other) => panic!("expected EventNotFound, got {other}"),
        Ok(report) => panic!(
            "unknown cutoff must not replay the whole log ({} events)",
            report.events
        ),
    }
    let full = replay_until(rt.root().join("events.cak"), None).unwrap();
    assert!(full.events > 0);
}

#[test]
fn lost_process_after_restart_is_not_silently_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "lost-proc").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "sleep", None, None)
        .unwrap();
    let (op, attempt, _) = rt
        .kernel
        .create_operation(
            agent,
            sleep_argv("30"),
            dir.path().to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
    let gen = rt
        .kernel
        .lease_operation(op, attempt, LeaseOwnerId::new(), "local", 60_000, None)
        .unwrap();
    rt.kernel
        .process_started(op, attempt, 999_999, "999999:0")
        .unwrap();
    drop(rt);
    let rt2 = Runtime::open(dir.path()).unwrap();
    let op_state = rt2
        .kernel
        .execution()
        .unwrap()
        .operations
        .get(&op)
        .cloned()
        .unwrap();
    assert_eq!(op_state.status, OperationStatus::Failed);
    assert_eq!(
        op_state.committed_exit_code, None,
        "dead pid must not invent exit -1"
    );
    let _ = gen;
}

#[test]
fn restart_surviving_process_reaches_terminal_state() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("hold.open");
    std::fs::write(&lock, b"1").unwrap();
    let mut rt = Runtime::create(dir.path(), "survive").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "hold", None, None)
        .unwrap();
    let argv = if cfg!(windows) {
        sleep_argv("8")
    } else {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("while [ -f '{}' ]; do sleep 0.05; done", lock.display()),
        ]
    };
    let (op, _, _) = rt
        .kernel
        .create_operation(agent, argv, dir.path().to_string_lossy().into_owned(), None)
        .unwrap();
    rt.drive(32).unwrap();
    for _ in 0..40 {
        let status = rt
            .kernel
            .execution()
            .unwrap()
            .operations
            .get(&op)
            .unwrap()
            .status;
        if status == OperationStatus::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
        rt.drive(8).unwrap();
    }
    let before = rt
        .kernel
        .execution()
        .unwrap()
        .operations
        .get(&op)
        .cloned()
        .unwrap();
    assert_eq!(before.status, OperationStatus::Running);
    assert_eq!(before.committed_exit_code, None);
    drop(rt);

    let mut rt2 = Runtime::open(dir.path()).unwrap();
    let after_open = rt2
        .kernel
        .execution()
        .unwrap()
        .operations
        .get(&op)
        .cloned()
        .unwrap();
    assert_eq!(after_open.committed_exit_code, None);
    assert_ne!(
        after_open.status,
        OperationStatus::Succeeded,
        "restart must not invent a successful exit"
    );

    if cfg!(target_os = "linux") {
        assert_eq!(after_open.status, OperationStatus::Running);
        let _ = std::fs::remove_file(&lock);
        rt2.wait_idle(Duration::from_secs(8)).unwrap();
        let terminal = rt2
            .kernel
            .execution()
            .unwrap()
            .operations
            .get(&op)
            .cloned()
            .unwrap();
        assert_eq!(terminal.status, OperationStatus::Failed);
        assert_eq!(
            terminal.committed_exit_code, None,
            "poll-detected death is not a wait(2) status"
        );
    } else {
        assert_eq!(
            after_open.status,
            OperationStatus::Unknown,
            "without process identity, recovery must preserve uncertainty, got {:?}",
            after_open.status
        );
        let _ = std::fs::remove_file(&lock);
    }
}

#[test]
fn context_sharing_beats_naive_copy_at_100_children() {
    let parent = 1_000_000u64;
    let delta = 2_000u64;
    let naive = naive_child_storage_bytes(parent, delta, 100);
    let shared = shared_child_storage_bytes(parent, delta, 100);
    assert!(shared * 10 < naive);
    let dir = tempfile::tempdir().unwrap();
    let mut cas = ContentStore::open(dir.path()).unwrap();
    let parent_blob = vec![7u8; 64 * 1024];
    let parent_snap = cas.snapshot(None, &parent_blob, 4096).unwrap();
    let mut children = Vec::new();
    for i in 0..10 {
        let mut child = parent_blob.clone();
        child.extend_from_slice(&[i as u8; 128]);
        children.push(cas.snapshot_delta(&parent_snap, &child, 4096).unwrap());
    }
    let unique = cas.unique_bytes().unwrap();
    let duplicated: u64 = children.iter().map(|s| s.byte_len).sum::<u64>() + parent_snap.byte_len;
    assert!(unique < duplicated);
}

#[test]
fn capability_cannot_exceed_parent() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("cap").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (parent, _) = k.spawn_agent(None, goal, "p", None, None).unwrap();
    let (cap, _) = k
        .grant_capability(
            parent,
            None,
            "/repo",
            codex_agent_kernel::NetworkScope::Deny,
            vec!["/bin/echo".into()],
            "user-approved:echo",
        )
        .unwrap();
    k.append(
        codex_agent_kernel::Event::AgentSpawned {
            agent_id: codex_agent_kernel::AgentId::new(),
            parent_agent_id: Some(parent),
            goal_id: goal,
            task: "child".into(),
            snapshot_id: None,
            capability_id: Some(cap),
        },
        codex_agent_kernel::IdempotencyKey::new("x", "1"),
    )
    .unwrap();
    let (wide, _) = k
        .grant_capability(
            parent,
            Some(cap),
            "/repo",
            codex_agent_kernel::NetworkScope::Allow,
            vec!["/bin/echo".into(), "/bin/rm".into()],
            "should-fail",
        )
        .unwrap();
    let err = k
        .spawn_agent(Some(parent), goal, "evil", None, Some(wide))
        .unwrap_err();
    assert!(err.to_string().contains("capability"));
}

#[test]
fn spawn_failure_does_not_leave_operation_leased() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "spawn-fail").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "work", None, None)
        .unwrap();
    rt.kernel
        .create_operation(
            agent,
            vec!["/definitely-not-a-cak-binary".into()],
            dir.path().to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
    let _ = rt.wait_idle(Duration::from_secs(2));
    let op = rt
        .kernel
        .execution()
        .unwrap()
        .operations
        .values()
        .next()
        .unwrap();
    assert_eq!(op.status, OperationStatus::Failed);
    assert_ne!(op.status, OperationStatus::Leased);
}

#[cfg(unix)]
#[test]
fn stdout_stderr_are_drained_without_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::create(dir.path(), "pipes").unwrap();
    let (goal, _) = rt.kernel.create_goal("g", vec![]).unwrap();
    let (agent, _) = rt
        .kernel
        .spawn_agent(None, goal, "pipes", None, None)
        .unwrap();
    rt.kernel
        .create_operation(
            agent,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "dd if=/dev/zero bs=65536 count=8 >&2; echo ok".into(),
            ],
            dir.path().to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
    rt.wait_idle(Duration::from_secs(5)).unwrap();
    let op = rt
        .kernel
        .execution()
        .unwrap()
        .operations
        .values()
        .next()
        .unwrap();
    assert_eq!(op.status, OperationStatus::Succeeded);
}
