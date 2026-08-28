use crate::error::KernelError;
use crate::event::OperationStatus;
use crate::ids::{AgentId, ExecutorId, OperationId};
use crate::state::ExecutionState;

#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub max_concurrent_operations: usize,
    pub max_concurrent_agents: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_operations: 8,
            max_concurrent_agents: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleDecision {
    pub runnable_agents: Vec<AgentId>,
    pub runnable_operations: Vec<OperationId>,
    pub blocked_operations: Vec<OperationId>,
}

/// Mechanical scheduler: the model decides semantic work; this decides
/// what is allowed to run now given leases, concurrency, and dependencies.
pub fn schedule(
    exec: &ExecutionState,
    cfg: &SchedulerConfig,
) -> Result<ScheduleDecision, KernelError> {
    let running_ops = exec
        .operations
        .values()
        .filter(|op| {
            matches!(
                op.status,
                OperationStatus::Leased
                    | OperationStatus::Running
                    | OperationStatus::WaitingExternal
            )
        })
        .count();
    let running_agents = exec
        .agents
        .values()
        .filter(|a| {
            matches!(
                a.status,
                crate::event::AgentStatus::Running | crate::event::AgentStatus::WaitingExternal
            )
        })
        .count();

    let mut runnable_operations = Vec::new();
    let mut blocked_operations = Vec::new();
    let mut slots = cfg.max_concurrent_operations.saturating_sub(running_ops);
    for op in exec.operations.values() {
        match op.status {
            OperationStatus::Ready | OperationStatus::Orphaned => {
                if let Some(agent) = exec.agents.get(&op.agent_id) {
                    if let Some(cap_id) = agent.capability_id {
                        if let Some(cap) = exec.capabilities.get(&cap_id) {
                            if !cap.command_allowlist.is_empty() {
                                if let Some(cmd) = op.argv.first() {
                                    if !command_allowed(cmd, &cap.command_allowlist) {
                                        blocked_operations.push(op.id);
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                if slots == 0 {
                    blocked_operations.push(op.id);
                } else {
                    runnable_operations.push(op.id);
                    slots -= 1;
                }
            }
            _ => {}
        }
    }

    let mut agent_slots = cfg.max_concurrent_agents.saturating_sub(running_agents);
    let mut runnable_agents = Vec::new();
    for agent in exec.agents.values() {
        if agent.status == crate::event::AgentStatus::Runnable && agent_slots > 0 {
            runnable_agents.push(agent.id);
            agent_slots -= 1;
        }
    }

    let _ = ExecutorId::new();
    Ok(ScheduleDecision {
        runnable_agents,
        runnable_operations,
        blocked_operations,
    })
}

/// Allowlist match is an executable boundary, never an arbitrary suffix.
///
/// * An entry with a path separator (`/` or `\`) matches only that exact argv0.
/// * A bare name (no separator) matches argv0 itself or an argv0 whose
///   final path component is exactly that name.
///
/// So `echo` allows `echo` and `/bin/echo`, but not `/tmp/malicious-echo`,
/// `my-echo`, or `bash`. `/bin/echo` allows only `/bin/echo`.
pub fn command_allowed(argv0: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|allow| executable_matches(argv0, allow))
}

fn executable_matches(argv0: &str, allow: &str) -> bool {
    if argv0 == allow {
        return true;
    }
    if !is_bare_executable_name(allow) {
        return false;
    }
    executable_basename(argv0) == Some(allow)
}

fn is_bare_executable_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\')
}

fn executable_basename(cmd: &str) -> Option<&str> {
    let trimmed = cmd.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    trimmed.rsplit(['/', '\\']).next()
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod scheduler_tests;
