use crate::error::KernelError;
use crate::event::{AgentStatus, Event, EventRecord, GoalStatus, OperationStatus};
use crate::ids::EventId;
use crate::state::ExecutionState;

pub fn render_tree(exec: &ExecutionState) -> String {
    let mut out = String::new();
    if let Some(goal) = exec.goals.values().next() {
        out.push_str(&format!(
            "Goal {} [{}]\n  {}\n",
            short(&goal.id.to_string()),
            status_goal(goal.status),
            goal.objective
        ));
    } else {
        out.push_str("Goal (none)\n");
    }
    let mut agents: Vec<_> = exec.agents.values().collect();
    agents.sort_by_key(|a| a.task.clone());
    for agent in agents {
        let indent = if agent.parent_agent_id.is_some() {
            "│   "
        } else {
            ""
        };
        let branch = if agent.parent_agent_id.is_some() {
            "└── "
        } else {
            "├── "
        };
        out.push_str(&format!(
            "{indent}{branch}Agent {} [{}]\n{indent}│     {}\n",
            short(&agent.id.to_string()),
            status_agent(agent.status),
            agent.task
        ));
        for op in exec
            .operations
            .values()
            .filter(|op| op.agent_id == agent.id)
        {
            let lease = op
                .lease
                .as_ref()
                .map(|l| {
                    format!(
                        " leased gen={} {}{}",
                        l.generation,
                        l.executor_id,
                        if l.expired { " EXPIRED" } else { "" }
                    )
                })
                .unwrap_or_default();
            let exit = op
                .committed_exit_code
                .map(|c| format!(" exit {c}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "{indent}│     └── op {} [{}{}{}]\n",
                short(&op.id.to_string()),
                status_op(op.status),
                exit,
                lease
            ));
        }
    }
    for join in exec.joins.values() {
        let waiting: Vec<String> = join
            .children
            .iter()
            .filter(|id| !join.observed.contains_key(id))
            .map(|id| short(&id.to_string()))
            .collect();
        out.push_str(&format!(
            "└── Join {} [{:?}] waiting on {}\n",
            short(&join.id.to_string()),
            join.outcome,
            if waiting.is_empty() {
                "(none)".to_string()
            } else {
                waiting.join(", ")
            }
        ));
    }
    out
}

pub fn render_trace(
    records: &[EventRecord],
    until: Option<EventId>,
) -> Result<String, KernelError> {
    let mut out = String::new();
    for record in records {
        if let Some(until) = until {
            if record.event_id == until {
                explain_record(&mut out, record);
                break;
            }
        }
        explain_record(&mut out, record);
    }
    Ok(out)
}

fn explain_record(out: &mut String, record: &EventRecord) {
    let summary = match &record.payload {
        Event::ExecutionCreated { note, .. } => format!("execution created ({note})"),
        Event::ExecutionForked {
            source_execution_id,
            from_event_id,
            ..
        } => format!("forked from {source_execution_id} at {from_event_id}"),
        Event::GoalCreated { objective, .. } => format!("goal created: {objective}"),
        Event::GoalAcceptanceDefined { goal_id, .. } => {
            format!("goal {goal_id} acceptance rules defined")
        }
        Event::ModelTurnFinished { agent_id, summary, .. } => {
            format!("MODEL_FINISHED_TURN agent={agent_id} {summary}")
        }
        Event::GoalCompleted { goal_id } => format!("GOAL_COMPLETED {goal_id}"),
        Event::GoalFailed { goal_id, reason } => format!("goal {goal_id} failed: {reason}"),
        Event::AgentSpawned {
            agent_id,
            parent_agent_id,
            task,
            snapshot_id,
            ..
        } => format!(
            "agent {agent_id} spawned parent={parent_agent_id:?} snapshot={snapshot_id:?} task={task}"
        ),
        Event::AgentStatusChanged { agent_id, from, to, reason } => {
            format!("agent {agent_id} {from:?}->{to:?} ({reason})")
        }
        Event::TaskCreated { task_id, description, .. } => {
            format!("task {task_id} {description}")
        }
        Event::JoinCreated {
            join_id,
            children,
            kind,
            ..
        } => format!("join {join_id} {kind:?} children={}", children.len()),
        Event::JoinChildTerminal {
            join_id,
            child_id,
            child_status,
        } => format!("join {join_id} observed {child_id} {child_status:?}"),
        Event::JoinSatisfied { join_id, outcome } => format!("join {join_id} {outcome:?}"),
        Event::OperationCreated { operation_id, argv, .. } => {
            format!("operation {operation_id} created argv={argv:?}")
        }
        Event::OperationScheduled { operation_id } => format!("operation {operation_id} scheduled"),
        Event::OperationLeased {
            operation_id,
            generation,
            executor_id,
            ..
        } => format!("operation {operation_id} leased gen={generation} executor={executor_id}"),
        Event::ProcessStarted {
            operation_id,
            pid,
            start_key,
            ..
        } => format!("process started op={operation_id} pid={pid} start_key={start_key}"),
        Event::ProcessOutputAvailable {
            operation_id,
            byte_len,
            chunk_hash,
            ..
        } => format!("process output op={operation_id} {byte_len}B hash={chunk_hash}"),
        Event::ProcessExited {
            operation_id,
            exit_code,
            signal,
            killed_externally,
            ..
        } => format!(
            "process exited op={operation_id} code={exit_code} signal={signal:?} killed_externally={killed_externally}"
        ),
        Event::OperationCommitted {
            operation_id,
            attempt_id,
            lease_generation,
            exit_code,
        } => format!(
            "operation {operation_id} committed attempt={attempt_id} gen={lease_generation} exit={exit_code}"
        ),
        Event::OperationFailed {
            operation_id,
            reason,
            ..
        } => format!("operation {operation_id} failed: {reason}"),
        Event::OperationUnresolved {
            operation_id,
            reason,
            ..
        } => format!("operation {operation_id} unresolved: {reason}"),
        Event::OperationCancelled { operation_id, reason } => {
            format!("operation {operation_id} cancelled: {reason}")
        }
        Event::LeaseExpired { operation_id, generation } => {
            format!("lease expired op={operation_id} gen={generation}")
        }
        Event::CheckpointTaken { seq, state_hash } => {
            format!("checkpoint seq={seq} hash={state_hash}")
        }
        Event::ContextSnapshotAttached {
            snapshot_id,
            agent_id,
            byte_len,
            ..
        } => format!("snapshot {snapshot_id} attached to {agent_id} ({byte_len}B)"),
        Event::CapabilityGranted {
            capability_id,
            agent_id,
            approval_evidence,
            ..
        } => format!(
            "capability {capability_id} granted to {agent_id} evidence={approval_evidence}"
        ),
        Event::BarrierCreated { barrier_id, expected_arrivals } => {
            format!("barrier {barrier_id} expect={expected_arrivals}")
        }
        Event::BarrierArrived { barrier_id, agent_id } => {
            format!("barrier {barrier_id} arrival {agent_id}")
        }
        Event::BarrierSatisfied { barrier_id } => format!("barrier {barrier_id} satisfied"),
        Event::RetentionChanged { agent_id, retained } => {
            format!("agent {agent_id} retained={retained}")
        }
    };
    out.push_str(&format!(
        "#{:<4} {}  {}\n",
        record.seq, record.event_id, summary
    ));
}

fn status_goal(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "ACTIVE",
        GoalStatus::Blocked => "BLOCKED",
        GoalStatus::Completed => "DONE",
        GoalStatus::Failed => "FAILED",
        GoalStatus::Cancelled => "CANCELLED",
    }
}

fn status_agent(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Created => "CREATED",
        AgentStatus::Runnable => "RUNNABLE",
        AgentStatus::Running => "RUNNING",
        AgentStatus::WaitingJoin => "WAITING_JOIN",
        AgentStatus::WaitingExternal => "WAITING",
        AgentStatus::Succeeded => "DONE",
        AgentStatus::Failed => "FAILED",
        AgentStatus::Cancelled => "CANCELLED",
        AgentStatus::Interrupted => "INTERRUPTED",
        AgentStatus::Orphaned => "ORPHANED",
    }
}

fn status_op(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Created => "CREATED",
        OperationStatus::Ready => "READY",
        OperationStatus::Leased => "LEASED",
        OperationStatus::Running => "RUNNING",
        OperationStatus::WaitingExternal => "WAITING",
        OperationStatus::Committing => "COMMITTING",
        OperationStatus::Succeeded => "DONE",
        OperationStatus::Failed => "FAILED",
        OperationStatus::Cancelled => "CANCELLED",
        OperationStatus::Orphaned => "ORPHANED",
        OperationStatus::Unknown => "UNKNOWN",
    }
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}
