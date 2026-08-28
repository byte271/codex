use crate::event::{AgentStatus, Event, OperationStatus, StreamKind};
use crate::ids::{AgentId, ContentHash, IdempotencyKey};
use crate::kernel::Kernel;
use crate::log::EventLog;
use crate::reducer::reduce;
use crate::state::KernelState;
use crate::state::OutputChunk;
use pretty_assertions::assert_eq;

#[test]
fn duplicate_event_id_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    let created = k.create_execution("dup").unwrap();
    let mut state = KernelState::default();
    reduce(&mut state, &created).unwrap();
    reduce(&mut state, &created).unwrap();
    assert_eq!(state.executions.len(), 1);
}

#[test]
fn committed_operation_cannot_return_to_running() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("op").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (agent, _) = k.spawn_agent(None, goal, "work", None, None).unwrap();
    k.start_agent(agent).unwrap();
    let (op, attempt, _) = k
        .create_operation(agent, vec!["/bin/true".into()], "/", None)
        .unwrap();
    let owner = crate::ids::LeaseOwnerId::new();
    let gen = k
        .lease_operation(op, attempt, owner, "local", 1000, None)
        .unwrap();
    k.process_started(op, attempt, 1, "1:0").unwrap();
    k.process_exited(op, attempt, 0, None, false).unwrap();
    k.commit_operation(op, attempt, gen, 0).unwrap();
    let err = k
        .append(
            Event::ProcessStarted {
                operation_id: op,
                attempt_id: attempt,
                pid: 2,
                start_key: "2:0".into(),
            },
            IdempotencyKey::new("proc_started_again", op),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("cannot return to RUNNING")
            || err.to_string().contains("invalid")
            || err.to_string().contains("ProcessStarted")
    );
}

#[test]
fn stale_lease_cannot_commit() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("lease").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (agent, _) = k.spawn_agent(None, goal, "work", None, None).unwrap();
    k.start_agent(agent).unwrap();
    let (op, attempt, _) = k
        .create_operation(agent, vec!["/bin/true".into()], "/", None)
        .unwrap();
    let owner_a = crate::ids::LeaseOwnerId::new();
    let owner_b = crate::ids::LeaseOwnerId::new();
    let gen41 = k
        .lease_operation(op, attempt, owner_a, "a", 1, None)
        .unwrap();
    assert_eq!(gen41, 1);
    k.expire_lease(op, gen41).unwrap();
    let attempt2 = crate::ids::AttemptId::new();
    let gen42 = k
        .lease_operation(op, attempt2, owner_b, "b", 1000, None)
        .unwrap();
    assert_eq!(gen42, 2);
    k.process_started(op, attempt2, 9, "9:0").unwrap();
    k.process_exited(op, attempt2, 0, None, false).unwrap();
    let err = k.commit_operation(op, attempt, gen41, 0).unwrap_err();
    match err {
        crate::error::KernelError::StaleLease { current, used, .. } => {
            assert_eq!(current, 2);
            assert_eq!(used, 1);
        }
        other => panic!("expected stale lease, got {other}"),
    }
    k.commit_operation(op, attempt2, gen42, 0).unwrap();
    assert_eq!(k.operation_status(op).unwrap(), OperationStatus::Succeeded);
}

#[test]
fn model_turn_does_not_complete_goal() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("goal").unwrap();
    let agent_placeholder = AgentId::new();
    let (goal, _) = k.create_goal("ship it", vec![agent_placeholder]).unwrap();
    let (agent, _) = k.spawn_agent(None, goal, "work", None, None).unwrap();
    k.define_acceptance(goal, vec![]).unwrap();
    k.start_agent(agent).unwrap();
    k.finish_model_turn(agent, "I am done").unwrap();
    let err = k.try_complete_goal(goal).unwrap_err();
    assert!(err.to_string().contains("preconditions"));
}

#[test]
fn two_agent_join_all_waits_for_both() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("join").unwrap();
    let (goal, _) = k.create_goal("parallel", vec![]).unwrap();
    let (parent, _) = k.spawn_agent(None, goal, "parent", None, None).unwrap();
    let (a, _) = k.spawn_agent(Some(parent), goal, "A", None, None).unwrap();
    let (b, _) = k.spawn_agent(Some(parent), goal, "B", None, None).unwrap();
    k.start_agent(parent).unwrap();
    k.start_agent(a).unwrap();
    k.start_agent(b).unwrap();
    let (join, _) = k
        .create_join(
            parent,
            vec![a, b],
            crate::event::JoinKind::All,
            crate::event::JoinFailurePolicy::WaitAll,
        )
        .unwrap();
    k.complete_agent(a).unwrap();
    k.observe_join_child(join, a).unwrap();
    assert!(k
        .execution()
        .unwrap()
        .joins
        .get(&join)
        .unwrap()
        .outcome
        .is_none());
    k.complete_agent(b).unwrap();
    k.observe_join_child(join, b).unwrap();
    assert_eq!(
        k.execution().unwrap().joins.get(&join).unwrap().outcome,
        Some(crate::event::JoinOutcome::Succeeded)
    );
    assert_eq!(
        k.execution().unwrap().agents.get(&parent).unwrap().status,
        AgentStatus::Succeeded
    );
}

#[test]
fn join_all_fails_if_child_fails() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("join-fail").unwrap();
    let (goal, _) = k.create_goal("parallel", vec![]).unwrap();
    let (parent, _) = k.spawn_agent(None, goal, "parent", None, None).unwrap();
    let (a, _) = k.spawn_agent(Some(parent), goal, "A", None, None).unwrap();
    let (b, _) = k.spawn_agent(Some(parent), goal, "B", None, None).unwrap();
    k.start_agent(parent).unwrap();
    k.start_agent(a).unwrap();
    k.start_agent(b).unwrap();
    let (join, _) = k
        .create_join(
            parent,
            vec![a, b],
            crate::event::JoinKind::All,
            crate::event::JoinFailurePolicy::WaitAll,
        )
        .unwrap();
    k.complete_agent(a).unwrap();
    k.fail_agent(b, "boom").unwrap();
    k.observe_join_child(join, a).unwrap();
    k.observe_join_child(join, b).unwrap();
    assert_eq!(
        k.execution().unwrap().joins.get(&join).unwrap().outcome,
        Some(crate::event::JoinOutcome::Failed)
    );
}

#[test]
fn join_terminal_event_cannot_forge_child_state() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("join-forge").unwrap();
    let (goal, _) = k.create_goal("parallel", vec![]).unwrap();
    let (parent, _) = k.spawn_agent(None, goal, "parent", None, None).unwrap();
    let (a, _) = k.spawn_agent(Some(parent), goal, "A", None, None).unwrap();
    k.start_agent(parent).unwrap();
    k.start_agent(a).unwrap();
    let (join, _) = k
        .create_join(
            parent,
            vec![a],
            crate::event::JoinKind::All,
            crate::event::JoinFailurePolicy::WaitAll,
        )
        .unwrap();
    let running_err = k
        .append(
            Event::JoinChildTerminal {
                join_id: join,
                child_id: a,
                child_status: AgentStatus::Succeeded,
            },
            IdempotencyKey::new("fake_join_running", format!("{join}:{a}")),
        )
        .unwrap_err();
    assert!(
        running_err.to_string().contains("still running"),
        "unexpected error: {running_err}"
    );

    k.complete_agent(a).unwrap();
    let mismatch_err = k
        .append(
            Event::JoinChildTerminal {
                join_id: join,
                child_id: a,
                child_status: AgentStatus::Failed,
            },
            IdempotencyKey::new("fake_join_status", format!("{join}:{a}:failed")),
        )
        .unwrap_err();
    assert!(
        mismatch_err.to_string().contains("does not match"),
        "unexpected error: {mismatch_err}"
    );
    assert!(k
        .execution()
        .unwrap()
        .joins
        .get(&join)
        .unwrap()
        .observed
        .is_empty());
}

#[test]
fn replay_hash_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("e.cak");
    let log = EventLog::create(&log_path).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("hash").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (agent, _) = k.spawn_agent(None, goal, "work", None, None).unwrap();
    k.start_agent(agent).unwrap();
    k.complete_agent(agent).unwrap();
    let (h1, b1) = k.hashes().unwrap();
    let k2 = Kernel::from_log(EventLog::open(&log_path).unwrap()).unwrap();
    let (h2, b2) = k2.hashes().unwrap();
    assert_eq!(h1, h2);
    assert_eq!(b1, b2);
}

#[test]
fn stdout_and_stderr_chunks_with_same_offset_and_len_are_both_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("streams").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (agent, _) = k.spawn_agent(None, goal, "work", None, None).unwrap();
    k.start_agent(agent).unwrap();
    let (op, attempt, _) = k
        .create_operation(agent, vec!["/bin/true".into()], "/", None)
        .unwrap();
    let owner = crate::ids::LeaseOwnerId::new();
    k.lease_operation(op, attempt, owner, "local", 1000, None)
        .unwrap();
    k.process_started(op, attempt, 1, "1:0").unwrap();

    let stdout = b"hello!!";
    let stderr = b"error!!";
    let stdout_rec = k
        .process_output(op, attempt, StreamKind::Stdout, 0, stdout)
        .unwrap();
    let stderr_rec = k
        .process_output(op, attempt, StreamKind::Stderr, 0, stderr)
        .unwrap();
    assert_ne!(stdout_rec.event_id, stderr_rec.event_id);
    assert_ne!(stdout_rec.idempotency_key, stderr_rec.idempotency_key);
    assert_eq!(
        k.process_output(op, attempt, StreamKind::Stdout, 0, stdout)
            .unwrap(),
        stdout_rec
    );

    let chunks = &k
        .execution()
        .unwrap()
        .operations
        .get(&op)
        .unwrap()
        .attempts
        .get(&attempt)
        .unwrap()
        .output_chunks;
    assert_eq!(
        chunks,
        &vec![
            OutputChunk {
                stream: StreamKind::Stdout,
                offset: 0,
                byte_len: stdout.len() as u64,
                chunk_hash: ContentHash::of_bytes(stdout),
            },
            OutputChunk {
                stream: StreamKind::Stderr,
                offset: 0,
                byte_len: stderr.len() as u64,
                chunk_hash: ContentHash::of_bytes(stderr),
            },
        ]
    );

    let output_streams: Vec<_> = k
        .log
        .records()
        .iter()
        .filter_map(|record| match &record.payload {
            Event::ProcessOutputAvailable { stream, .. } => Some(*stream),
            _ => None,
        })
        .collect();
    assert_eq!(output_streams, vec![StreamKind::Stdout, StreamKind::Stderr]);
}
