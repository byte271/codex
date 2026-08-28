use std::collections::BTreeMap;

use crate::error::KernelError;
use crate::event::{
    AgentStatus, Event, EventRecord, GoalStatus, JoinFailurePolicy, JoinKind, JoinOutcome,
    NetworkScope, OperationStatus, SCHEMA_VERSION,
};
use crate::ids::{AgentId, EventId, ExecutionId, JoinId};
use crate::state::{
    AgentState, AttemptState, BarrierState, CapabilityState, CheckpointState, ExecutionState,
    ForkAncestry, GoalState, JoinState, KernelState, LeaseState, ModelTurnState, OperationState,
    OutputChunk, SnapshotState, TaskState,
};

pub fn reduce(state: &mut KernelState, record: &EventRecord) -> Result<(), KernelError> {
    record.verify_checksum()?;
    if record.schema_version != SCHEMA_VERSION {
        return Err(KernelError::UnsupportedSchema {
            found: record.schema_version,
            expected: SCHEMA_VERSION,
        });
    }
    if let Some(existing) = state.seen_event_ids.get(&record.event_id) {
        let _ = existing;
        return Ok(());
    }
    if let Some(existing) = state.idempotency_index.get(&record.idempotency_key) {
        if *existing != record.event_id {
            return Err(KernelError::IdempotencyConflict {
                key: record.idempotency_key.to_string(),
                existing: existing.to_string(),
                new_event: record.event_id.to_string(),
            });
        }
        return Ok(());
    }

    let expected = state
        .last_seq
        .get(&record.execution_id)
        .copied()
        .unwrap_or(0)
        + 1;
    if record.seq != expected {
        return Err(KernelError::NonMonotonicSeq {
            execution: record.execution_id.to_string(),
            expected,
            got: record.seq,
        });
    }

    apply_payload(state, record)?;
    state.seen_event_ids.insert(record.event_id);
    state
        .idempotency_index
        .insert(record.idempotency_key.clone(), record.event_id);
    state.last_seq.insert(record.execution_id, record.seq);
    Ok(())
}

pub fn reduce_all(records: &[EventRecord]) -> Result<KernelState, KernelError> {
    let mut state = KernelState::default();
    for record in records {
        reduce(&mut state, record)?;
    }
    Ok(state)
}

fn apply_payload(state: &mut KernelState, record: &EventRecord) -> Result<(), KernelError> {
    match &record.payload {
        Event::ExecutionCreated {
            created_at_ms,
            note,
        } => {
            if state.executions.contains_key(&record.execution_id) {
                return Err(KernelError::invalid(format!(
                    "execution {} already exists",
                    record.execution_id
                )));
            }
            if record.seq != 1 {
                return Err(KernelError::invalid(
                    "ExecutionCreated must be seq 1 for an execution",
                ));
            }
            state.executions.insert(
                record.execution_id,
                ExecutionState::new(record.execution_id, *created_at_ms, note.clone()),
            );
            Ok(())
        }
        Event::ExecutionForked {
            source_execution_id,
            from_event_id,
            from_seq,
            source_business_hash,
        } => apply_fork(
            state,
            record.execution_id,
            *source_execution_id,
            *from_event_id,
            *from_seq,
            source_business_hash.clone(),
        ),
        other => {
            let exec = state
                .executions
                .get_mut(&record.execution_id)
                .ok_or_else(|| KernelError::ExecutionNotFound(record.execution_id.to_string()))?;
            apply_execution_event(exec, other)
        }
    }
}

fn apply_fork(
    state: &mut KernelState,
    execution_id: ExecutionId,
    source_execution_id: ExecutionId,
    from_event_id: EventId,
    from_seq: u64,
    source_business_hash: String,
) -> Result<(), KernelError> {
    let exec = state
        .executions
        .get_mut(&execution_id)
        .ok_or_else(|| KernelError::ExecutionNotFound(execution_id.to_string()))?;
    if exec.ancestor.is_some() {
        return Err(KernelError::invalid("execution already has ancestry"));
    }
    exec.ancestor = Some(ForkAncestry {
        source_execution_id,
        from_event_id,
        from_seq,
        source_business_hash,
    });
    Ok(())
}

fn apply_execution_event(exec: &mut ExecutionState, event: &Event) -> Result<(), KernelError> {
    match event {
        Event::ExecutionCreated { .. } | Event::ExecutionForked { .. } => {
            Err(KernelError::invalid("internal: nested execution event"))
        }
        Event::GoalCreated {
            goal_id,
            objective,
            required_agent_ids,
        } => {
            if exec.goals.contains_key(goal_id) {
                return Err(KernelError::invalid(format!("goal {goal_id} exists")));
            }
            exec.goals.insert(
                *goal_id,
                GoalState {
                    id: *goal_id,
                    objective: objective.clone(),
                    status: GoalStatus::Active,
                    required_agent_ids: required_agent_ids.clone(),
                    require_all_required_agents_terminal: true,
                    require_no_running_operations: true,
                    require_barriers: Vec::new(),
                    model_turns_finished: 0,
                },
            );
            Ok(())
        }
        Event::GoalAcceptanceDefined {
            goal_id,
            require_all_required_agents_terminal,
            require_no_running_operations,
            require_barriers,
        } => {
            let goal = exec
                .goals
                .get_mut(goal_id)
                .ok_or_else(|| KernelError::invalid(format!("unknown goal {goal_id}")))?;
            if goal.status.is_terminal() {
                return Err(KernelError::invalid("cannot redefine a terminal goal"));
            }
            goal.require_all_required_agents_terminal = *require_all_required_agents_terminal;
            goal.require_no_running_operations = *require_no_running_operations;
            goal.require_barriers = require_barriers.clone();
            Ok(())
        }
        Event::ModelTurnFinished {
            turn_id,
            agent_id,
            summary,
        } => {
            if !exec.agents.contains_key(agent_id) {
                return Err(KernelError::invalid(format!(
                    "model turn for unknown agent {agent_id}"
                )));
            }
            exec.model_turns.push(ModelTurnState {
                turn_id: *turn_id,
                agent_id: *agent_id,
                summary: summary.clone(),
            });
            if let Some(agent) = exec.agents.get(agent_id) {
                if let Some(goal) = exec.goals.get_mut(&agent.goal_id) {
                    goal.model_turns_finished = goal.model_turns_finished.saturating_add(1);
                }
            }
            Ok(())
        }
        Event::GoalCompleted { goal_id } => {
            if !goal_completion_preconditions_met(exec, *goal_id)? {
                return Err(KernelError::invalid(
                    "GOAL_COMPLETED rejected: acceptance preconditions not met (model turn finish is not sufficient)",
                ));
            }
            let goal = exec.goals.get_mut(goal_id).expect("checked");
            goal.status = GoalStatus::Completed;
            Ok(())
        }
        Event::GoalFailed { goal_id, reason: _ } => {
            let goal = exec
                .goals
                .get_mut(goal_id)
                .ok_or_else(|| KernelError::invalid(format!("unknown goal {goal_id}")))?;
            if goal.status.is_terminal() {
                return Err(KernelError::invalid("goal already terminal"));
            }
            goal.status = GoalStatus::Failed;
            Ok(())
        }
        Event::AgentSpawned {
            agent_id,
            parent_agent_id,
            goal_id,
            task,
            snapshot_id,
            capability_id,
        } => {
            if exec.agents.contains_key(agent_id) {
                return Err(KernelError::invalid(format!("agent {agent_id} exists")));
            }
            if !exec.goals.contains_key(goal_id) {
                return Err(KernelError::invalid(format!("unknown goal {goal_id}")));
            }
            if let Some(parent) = parent_agent_id {
                if !exec.agents.contains_key(parent) {
                    return Err(KernelError::invalid(format!(
                        "unknown parent agent {parent}"
                    )));
                }
            }
            if let Some(cap) = capability_id {
                let cap_state = exec
                    .capabilities
                    .get(cap)
                    .cloned()
                    .ok_or_else(|| KernelError::invalid(format!("unknown capability {cap}")))?;
                if let Some(parent_cap_id) = cap_state.parent_capability_id {
                    if !capability_is_subset(exec, &cap_state, parent_cap_id) {
                        return Err(KernelError::invalid(
                            "child capability exceeds parent capability",
                        ));
                    }
                }
                if let Some(parent) = parent_agent_id {
                    if let Some(parent_agent) = exec.agents.get(parent) {
                        if let Some(parent_cap_id) = parent_agent.capability_id {
                            if !capability_is_subset(exec, &cap_state, parent_cap_id) {
                                return Err(KernelError::invalid(
                                    "child capability exceeds parent capability",
                                ));
                            }
                        }
                    }
                }
            }
            exec.agents.insert(
                *agent_id,
                AgentState {
                    id: *agent_id,
                    parent_agent_id: *parent_agent_id,
                    goal_id: *goal_id,
                    task: task.clone(),
                    status: AgentStatus::Created,
                    snapshot_id: *snapshot_id,
                    capability_id: *capability_id,
                    retained: true,
                },
            );
            Ok(())
        }
        Event::AgentStatusChanged {
            agent_id,
            from,
            to,
            reason: _,
        } => {
            let agent = exec
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| KernelError::invalid(format!("unknown agent {agent_id}")))?;
            if agent.status != *from {
                return Err(KernelError::invalid(format!(
                    "agent {agent_id} is {:?}, not {:?}",
                    agent.status, from
                )));
            }
            if agent.status.is_terminal() && *to != agent.status {
                return Err(KernelError::invalid(
                    "committed/terminal agent status is monotonic",
                ));
            }
            if !legal_agent_transition(*from, *to) {
                return Err(KernelError::invalid(format!(
                    "illegal agent transition {:?} -> {:?}",
                    from, to
                )));
            }
            agent.status = *to;
            Ok(())
        }
        Event::TaskCreated {
            task_id,
            goal_id,
            agent_id,
            description,
        } => {
            if exec.tasks.contains_key(task_id) {
                return Err(KernelError::invalid(format!("task {task_id} exists")));
            }
            if !exec.goals.contains_key(goal_id) || !exec.agents.contains_key(agent_id) {
                return Err(KernelError::invalid(
                    "task references unknown goal or agent",
                ));
            }
            exec.tasks.insert(
                *task_id,
                TaskState {
                    id: *task_id,
                    goal_id: *goal_id,
                    agent_id: *agent_id,
                    description: description.clone(),
                },
            );
            Ok(())
        }
        Event::JoinCreated {
            join_id,
            waiter_agent_id,
            children,
            kind,
            failure_policy,
        } => {
            if exec.joins.contains_key(join_id) {
                return Err(KernelError::invalid(format!("join {join_id} exists")));
            }
            if children.is_empty() {
                return Err(KernelError::invalid("join requires at least one child"));
            }
            if !exec.agents.contains_key(waiter_agent_id) {
                return Err(KernelError::invalid("join waiter unknown"));
            }
            for child in children {
                if !exec.agents.contains_key(child) {
                    return Err(KernelError::invalid(format!("join child {child} unknown")));
                }
            }
            exec.joins.insert(
                *join_id,
                JoinState {
                    id: *join_id,
                    waiter_agent_id: *waiter_agent_id,
                    children: children.clone(),
                    kind: *kind,
                    failure_policy: *failure_policy,
                    observed: BTreeMap::new(),
                    outcome: None,
                },
            );
            Ok(())
        }
        Event::JoinChildTerminal {
            join_id,
            child_id,
            child_status,
        } => {
            let actual_status = exec.agents.get(child_id).map(|agent| agent.status);
            let join = exec
                .joins
                .get_mut(join_id)
                .ok_or_else(|| KernelError::invalid(format!("unknown join {join_id}")))?;
            if join.outcome.is_some() {
                return Err(KernelError::invalid("join already satisfied"));
            }
            if !join.children.contains(child_id) {
                return Err(KernelError::invalid("child is not part of join"));
            }
            let Some(actual_status) = actual_status else {
                return Err(KernelError::invalid("unknown join child"));
            };
            if !actual_status.is_terminal() {
                return Err(KernelError::invalid(
                    "join child is still running; cannot observe terminal",
                ));
            }
            if actual_status != *child_status {
                return Err(KernelError::invalid(
                    "join child status does not match agent state",
                ));
            }
            join.observed.insert(*child_id, *child_status);
            Ok(())
        }
        Event::JoinSatisfied { join_id, outcome } => {
            let expected = expected_join_outcome(exec, *join_id)?;
            if expected != Some(*outcome) {
                return Err(KernelError::invalid(format!(
                    "join outcome {outcome:?} does not match observed children ({expected:?})"
                )));
            }
            let join = exec.joins.get_mut(join_id).expect("checked");
            join.outcome = Some(*outcome);
            Ok(())
        }
        Event::OperationCreated {
            operation_id,
            agent_id,
            attempt_id,
            argv,
            cwd,
            executor_hint,
        } => {
            if exec.operations.contains_key(operation_id) {
                return Err(KernelError::invalid(format!(
                    "operation {operation_id} exists"
                )));
            }
            if !exec.agents.contains_key(agent_id) {
                return Err(KernelError::invalid("operation agent unknown"));
            }
            let mut attempts = BTreeMap::new();
            attempts.insert(
                *attempt_id,
                AttemptState {
                    id: *attempt_id,
                    pid: None,
                    start_key: None,
                    exit_code: None,
                    signal: None,
                    killed_externally: false,
                    output_chunks: Vec::new(),
                },
            );
            exec.operations.insert(
                *operation_id,
                OperationState {
                    id: *operation_id,
                    agent_id: *agent_id,
                    status: OperationStatus::Created,
                    argv: argv.clone(),
                    cwd: cwd.clone(),
                    executor_hint: executor_hint.clone(),
                    attempts,
                    active_attempt: Some(*attempt_id),
                    lease: None,
                    committed_attempt: None,
                    committed_exit_code: None,
                },
            );
            Ok(())
        }
        Event::OperationScheduled { operation_id } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("cannot schedule a terminal operation"));
            }
            if !matches!(
                op.status,
                OperationStatus::Created | OperationStatus::Orphaned | OperationStatus::Unknown
            ) {
                return Err(KernelError::invalid(format!(
                    "cannot schedule from {:?}",
                    op.status
                )));
            }
            op.status = OperationStatus::Ready;
            Ok(())
        }
        Event::OperationLeased {
            operation_id,
            attempt_id,
            generation,
            owner,
            expires_at_ms,
            capability_id,
            executor_id,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("cannot lease a terminal operation"));
            }
            if !matches!(
                op.status,
                OperationStatus::Ready | OperationStatus::Orphaned
            ) {
                return Err(KernelError::invalid(format!(
                    "cannot lease from {:?}",
                    op.status
                )));
            }
            if let Some(existing) = &op.lease {
                if !existing.expired && existing.generation >= *generation {
                    return Err(KernelError::invalid(
                        "operation already has an active authoritative lease",
                    ));
                }
                if *generation != existing.generation + 1 {
                    return Err(KernelError::invalid(
                        "lease generation must increase by exactly 1",
                    ));
                }
            } else if *generation != 1 {
                return Err(KernelError::invalid("first lease generation must be 1"));
            }
            if !op.attempts.contains_key(attempt_id) {
                op.attempts.insert(
                    *attempt_id,
                    AttemptState {
                        id: *attempt_id,
                        pid: None,
                        start_key: None,
                        exit_code: None,
                        signal: None,
                        killed_externally: false,
                        output_chunks: Vec::new(),
                    },
                );
            }
            op.active_attempt = Some(*attempt_id);
            op.lease = Some(LeaseState {
                generation: *generation,
                owner: *owner,
                expires_at_ms: *expires_at_ms,
                executor_id: executor_id.clone(),
                capability_id: *capability_id,
                expired: false,
            });
            op.status = OperationStatus::Leased;
            Ok(())
        }
        Event::ProcessStarted {
            operation_id,
            attempt_id,
            pid,
            start_key,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid(
                    "a committed operation cannot return to RUNNING",
                ));
            }
            if !matches!(
                op.status,
                OperationStatus::Leased | OperationStatus::WaitingExternal
            ) {
                return Err(KernelError::invalid(format!(
                    "ProcessStarted from {:?}",
                    op.status
                )));
            }
            let attempt = op
                .attempts
                .get_mut(attempt_id)
                .ok_or_else(|| KernelError::invalid("unknown attempt"))?;
            attempt.pid = Some(*pid);
            attempt.start_key = Some(start_key.clone());
            op.active_attempt = Some(*attempt_id);
            op.status = OperationStatus::Running;
            Ok(())
        }
        Event::ProcessOutputAvailable {
            operation_id,
            attempt_id,
            stream,
            offset,
            byte_len,
            chunk_hash,
        } => {
            let op = require_op(exec, *operation_id)?;
            if !matches!(
                op.status,
                OperationStatus::Running
                    | OperationStatus::WaitingExternal
                    | OperationStatus::Committing
            ) {
                return Err(KernelError::invalid(
                    "output is only valid for a live process attempt",
                ));
            }
            let attempt = op
                .attempts
                .get_mut(attempt_id)
                .ok_or_else(|| KernelError::invalid("unknown attempt"))?;
            attempt.output_chunks.push(OutputChunk {
                stream: *stream,
                offset: *offset,
                byte_len: *byte_len,
                chunk_hash: *chunk_hash,
            });
            Ok(())
        }
        Event::ProcessExited {
            operation_id,
            attempt_id,
            exit_code,
            signal,
            killed_externally,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("process already committed"));
            }
            if !matches!(
                op.status,
                OperationStatus::Running
                    | OperationStatus::WaitingExternal
                    | OperationStatus::Leased
            ) {
                return Err(KernelError::invalid(format!(
                    "ProcessExited from {:?}",
                    op.status
                )));
            }
            let attempt = op
                .attempts
                .get_mut(attempt_id)
                .ok_or_else(|| KernelError::invalid("unknown attempt"))?;
            if attempt.exit_code.is_some() {
                return Err(KernelError::invalid("attempt already has ProcessExited"));
            }
            attempt.exit_code = Some(*exit_code);
            attempt.signal = signal.clone();
            attempt.killed_externally = *killed_externally;
            op.status = OperationStatus::Committing;
            Ok(())
        }
        Event::OperationCommitted {
            operation_id,
            attempt_id,
            lease_generation,
            exit_code,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("operation already terminal"));
            }
            if op.status != OperationStatus::Committing {
                return Err(KernelError::invalid(
                    "commit requires ProcessExited (Committing)",
                ));
            }
            let lease = op
                .lease
                .as_ref()
                .ok_or_else(|| KernelError::invalid("commit requires a lease"))?;
            if lease.expired {
                return Err(KernelError::StaleLease {
                    operation: operation_id.to_string(),
                    current: lease.generation,
                    used: *lease_generation,
                });
            }
            if lease.generation != *lease_generation {
                return Err(KernelError::StaleLease {
                    operation: operation_id.to_string(),
                    current: lease.generation,
                    used: *lease_generation,
                });
            }
            let attempt = op
                .attempts
                .get(attempt_id)
                .ok_or_else(|| KernelError::invalid("unknown attempt"))?;
            let Some(process_exit) = attempt.exit_code else {
                return Err(KernelError::invalid(
                    "commit must identify an attempt that has ProcessExited",
                ));
            };
            if process_exit != *exit_code {
                return Err(KernelError::invalid(
                    "commit exit_code must match ProcessExited",
                ));
            }
            op.committed_attempt = Some(*attempt_id);
            op.committed_exit_code = Some(*exit_code);
            op.status = if *exit_code == 0 {
                OperationStatus::Succeeded
            } else {
                OperationStatus::Failed
            };
            Ok(())
        }
        Event::OperationFailed {
            operation_id,
            attempt_id: _,
            lease_generation,
            reason: _,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("operation already terminal"));
            }
            if let (Some(used), Some(lease)) = (lease_generation, op.lease.as_ref()) {
                if lease.generation != *used || lease.expired {
                    return Err(KernelError::StaleLease {
                        operation: operation_id.to_string(),
                        current: lease.generation,
                        used: *used,
                    });
                }
            }
            op.status = OperationStatus::Failed;
            Ok(())
        }
        Event::OperationUnresolved {
            operation_id,
            attempt_id: _,
            reason: _,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("operation already terminal"));
            }
            op.status = OperationStatus::Unknown;
            Ok(())
        }
        Event::OperationCancelled {
            operation_id,
            reason: _,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Err(KernelError::invalid("operation already terminal"));
            }
            op.status = OperationStatus::Cancelled;
            Ok(())
        }
        Event::LeaseExpired {
            operation_id,
            generation,
        } => {
            let op = require_op(exec, *operation_id)?;
            if op.status.is_terminal() {
                return Ok(());
            }
            let lease = op
                .lease
                .as_mut()
                .ok_or_else(|| KernelError::invalid("no lease to expire"))?;
            if lease.generation != *generation {
                return Err(KernelError::invalid("lease generation mismatch on expire"));
            }
            lease.expired = true;
            op.status = OperationStatus::Orphaned;
            Ok(())
        }
        Event::CheckpointTaken { seq, state_hash } => {
            exec.checkpoints.push(CheckpointState {
                seq: *seq,
                state_hash: state_hash.clone(),
            });
            Ok(())
        }
        Event::ContextSnapshotAttached {
            snapshot_id,
            agent_id,
            parent_snapshot_id,
            chunk_hashes,
            byte_len,
        } => {
            if !exec.agents.contains_key(agent_id) {
                return Err(KernelError::invalid("snapshot agent unknown"));
            }
            exec.snapshots.insert(
                *snapshot_id,
                SnapshotState {
                    id: *snapshot_id,
                    agent_id: *agent_id,
                    parent_snapshot_id: *parent_snapshot_id,
                    chunk_hashes: chunk_hashes.clone(),
                    byte_len: *byte_len,
                },
            );
            if let Some(agent) = exec.agents.get_mut(agent_id) {
                agent.snapshot_id = Some(*snapshot_id);
            }
            Ok(())
        }
        Event::CapabilityGranted {
            capability_id,
            parent_capability_id,
            agent_id,
            filesystem_scope,
            network,
            command_allowlist,
            deadline_ms,
            approval_evidence,
        } => {
            if exec.capabilities.contains_key(capability_id) {
                return Err(KernelError::invalid("capability exists"));
            }
            let cap_state = CapabilityState {
                id: *capability_id,
                parent_capability_id: *parent_capability_id,
                agent_id: *agent_id,
                filesystem_scope: filesystem_scope.clone(),
                network: *network,
                command_allowlist: command_allowlist.clone(),
                deadline_ms: *deadline_ms,
                approval_evidence: approval_evidence.clone(),
            };
            exec.capabilities.insert(*capability_id, cap_state.clone());
            let attach = match exec.agents.get(agent_id).and_then(|a| a.capability_id) {
                None => parent_capability_id.is_none(),
                Some(existing) => capability_is_subset(exec, &cap_state, existing),
            };
            if attach {
                if let Some(agent) = exec.agents.get_mut(agent_id) {
                    agent.capability_id = Some(*capability_id);
                }
            }
            Ok(())
        }
        Event::BarrierCreated {
            barrier_id,
            expected_arrivals,
        } => {
            if *expected_arrivals == 0 {
                return Err(KernelError::invalid(
                    "barrier expected_arrivals must be > 0",
                ));
            }
            exec.barriers.insert(
                *barrier_id,
                BarrierState {
                    id: *barrier_id,
                    expected_arrivals: *expected_arrivals,
                    arrivals: Vec::new(),
                    satisfied: false,
                },
            );
            Ok(())
        }
        Event::BarrierArrived {
            barrier_id,
            agent_id,
        } => {
            let barrier = exec
                .barriers
                .get_mut(barrier_id)
                .ok_or_else(|| KernelError::invalid("unknown barrier"))?;
            if barrier.satisfied {
                return Err(KernelError::invalid("barrier already satisfied"));
            }
            if barrier.arrivals.contains(agent_id) {
                return Ok(());
            }
            barrier.arrivals.push(*agent_id);
            Ok(())
        }
        Event::BarrierSatisfied { barrier_id } => {
            let barrier = exec
                .barriers
                .get_mut(barrier_id)
                .ok_or_else(|| KernelError::invalid("unknown barrier"))?;
            if barrier.arrivals.len() as u32 != barrier.expected_arrivals {
                return Err(KernelError::invalid(
                    "barrier cannot be satisfied before all arrivals",
                ));
            }
            barrier.satisfied = true;
            Ok(())
        }
        Event::RetentionChanged { agent_id, retained } => {
            let agent = exec
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| KernelError::invalid("unknown agent"))?;
            agent.retained = *retained;
            Ok(())
        }
    }
}

fn require_op(
    exec: &mut ExecutionState,
    id: crate::ids::OperationId,
) -> Result<&mut OperationState, KernelError> {
    exec.operations
        .get_mut(&id)
        .ok_or_else(|| KernelError::invalid(format!("unknown operation {id}")))
}

fn legal_agent_transition(from: AgentStatus, to: AgentStatus) -> bool {
    if from == to {
        return true;
    }
    match (from, to) {
        (
            AgentStatus::Created,
            AgentStatus::Runnable | AgentStatus::Running | AgentStatus::Cancelled,
        ) => true,
        (
            AgentStatus::Runnable,
            AgentStatus::Running
            | AgentStatus::WaitingJoin
            | AgentStatus::Cancelled
            | AgentStatus::Failed,
        ) => true,
        (
            AgentStatus::Running,
            AgentStatus::WaitingJoin
            | AgentStatus::WaitingExternal
            | AgentStatus::Succeeded
            | AgentStatus::Failed
            | AgentStatus::Cancelled
            | AgentStatus::Interrupted
            | AgentStatus::Orphaned,
        ) => true,
        (
            AgentStatus::WaitingJoin,
            AgentStatus::Succeeded
            | AgentStatus::Failed
            | AgentStatus::Cancelled
            | AgentStatus::Running
            | AgentStatus::Interrupted,
        ) => true,
        (
            AgentStatus::WaitingExternal,
            AgentStatus::Running
            | AgentStatus::Failed
            | AgentStatus::Cancelled
            | AgentStatus::Orphaned,
        ) => true,
        (
            AgentStatus::Interrupted,
            AgentStatus::Runnable | AgentStatus::Running | AgentStatus::Cancelled,
        ) => true,
        (
            AgentStatus::Orphaned,
            AgentStatus::Runnable | AgentStatus::Failed | AgentStatus::Cancelled,
        ) => true,
        (_, _) => false,
    }
}

fn expected_join_outcome(
    exec: &ExecutionState,
    join_id: JoinId,
) -> Result<Option<JoinOutcome>, KernelError> {
    let join = exec
        .joins
        .get(&join_id)
        .ok_or_else(|| KernelError::invalid("unknown join"))?;
    match join.kind {
        JoinKind::Any => {
            if join.observed.is_empty() {
                return Ok(None);
            }
            let status = *join.observed.values().next().expect("non-empty");
            Ok(Some(status_to_join_outcome(status)))
        }
        JoinKind::All => {
            let all_present = join
                .children
                .iter()
                .all(|id| join.observed.contains_key(id));
            if !all_present {
                if join.failure_policy == JoinFailurePolicy::FailFast
                    && join
                        .observed
                        .values()
                        .any(|status| *status == AgentStatus::Failed)
                {
                    return Ok(Some(JoinOutcome::Failed));
                }
                return Ok(None);
            }
            if join
                .observed
                .values()
                .any(|status| *status == AgentStatus::Failed)
            {
                return Ok(Some(JoinOutcome::Failed));
            }
            if join
                .observed
                .values()
                .any(|status| *status == AgentStatus::Cancelled)
            {
                return Ok(Some(JoinOutcome::Cancelled));
            }
            Ok(Some(JoinOutcome::Succeeded))
        }
    }
}

fn status_to_join_outcome(status: AgentStatus) -> JoinOutcome {
    match status {
        AgentStatus::Succeeded => JoinOutcome::Succeeded,
        AgentStatus::Cancelled => JoinOutcome::Cancelled,
        _ => JoinOutcome::Failed,
    }
}

pub fn goal_completion_preconditions_met(
    exec: &ExecutionState,
    goal_id: crate::ids::GoalId,
) -> Result<bool, KernelError> {
    let goal = exec
        .goals
        .get(&goal_id)
        .ok_or_else(|| KernelError::invalid(format!("unknown goal {goal_id}")))?;
    if goal.status.is_terminal() {
        return Ok(false);
    }
    if goal.require_all_required_agents_terminal {
        let required: Vec<AgentId> = if goal.required_agent_ids.is_empty() {
            exec.agents
                .values()
                .filter(|agent| agent.goal_id == goal_id)
                .map(|agent| agent.id)
                .collect()
        } else {
            goal.required_agent_ids.clone()
        };
        if required.is_empty() {
            return Ok(false);
        }
        for agent_id in &required {
            match exec.agents.get(agent_id) {
                Some(agent) if agent.status == AgentStatus::Succeeded => {}
                Some(_) | None => return Ok(false),
            }
        }
    }
    if goal.require_no_running_operations {
        let running = exec.operations.values().any(|op| {
            let agent_ok = exec
                .agents
                .get(&op.agent_id)
                .map(|agent| agent.goal_id == goal_id)
                .unwrap_or(false);
            agent_ok && !op.status.is_terminal() && op.status != OperationStatus::Created
        });
        if running {
            return Ok(false);
        }
    }
    for barrier_id in &goal.require_barriers {
        match exec.barriers.get(barrier_id) {
            Some(barrier) if barrier.satisfied => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn capability_is_subset(
    exec: &ExecutionState,
    child: &CapabilityState,
    parent_id: crate::ids::CapabilityId,
) -> bool {
    let Some(parent) = exec.capabilities.get(&parent_id) else {
        return false;
    };
    if child.network == NetworkScope::Allow && parent.network == NetworkScope::Deny {
        return false;
    }
    if !child.filesystem_scope.starts_with(&parent.filesystem_scope) {
        return false;
    }
    if parent.command_allowlist.is_empty() {
        return true;
    }
    child
        .command_allowlist
        .iter()
        .all(|cmd| parent.command_allowlist.contains(cmd))
}

pub fn join_is_satisfied(
    exec: &ExecutionState,
    join_id: JoinId,
) -> Result<Option<JoinOutcome>, KernelError> {
    expected_join_outcome(exec, join_id)
}
