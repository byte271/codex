//! Compatibility map between current Codex concepts and kernel entities.
//!
//! This module does not depend on `codex-core`. Mappings are documentary and
//! used by tests as a checklist so an upstream engineer can evaluate the
//! architecture without a rewrite.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingQuality {
    Faithful,
    Approximate,
    ProjectionOnly,
    Unmapped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub codex: &'static str,
    pub kernel: &'static str,
    pub quality: MappingQuality,
    pub notes: &'static str,
}

pub fn mappings() -> Vec<Mapping> {
    vec![
        Mapping {
            codex: "thread / SessionId",
            kernel: "Execution",
            quality: MappingQuality::Approximate,
            notes: "A Codex thread is a conversation plus live runtime. Kernel Execution is the durable run. UI thread metadata remains a projection.",
        },
        Mapping {
            codex: "ext/goal ThreadGoal",
            kernel: "Goal",
            quality: MappingQuality::Approximate,
            notes: "Codex goals are SQLite rows completed by model tool update_goal. Kernel Goal completion is an acceptance-checked event, not MODEL_FINISHED_TURN.",
        },
        Mapping {
            codex: "subagent / AgentControl thread",
            kernel: "Agent",
            quality: MappingQuality::Approximate,
            notes: "Codex agent status is derived from turn events and is live. Kernel Agent status is reduced from the event log.",
        },
        Mapping {
            codex: "wait_agent v2 (mailbox wake)",
            kernel: "Join",
            quality: MappingQuality::Unmapped,
            notes: "Codex has no authoritative join. Kernel JoinAll/JoinAny is the proposed primitive.",
        },
        Mapping {
            codex: "tool call / unified exec process",
            kernel: "Operation + Attempt",
            quality: MappingQuality::Approximate,
            notes: "Codex exposes session_id, process_id, cell_id, call_id. Kernel uses one OperationId from launch to exit.",
        },
        Mapping {
            codex: "rollout JSONL RolloutItem",
            kernel: "EventRecord",
            quality: MappingQuality::Approximate,
            notes: "Rollout is canonical for session history, not for process/agent leases. Kernel events are the execution authority.",
        },
        Mapping {
            codex: "thread_history SQLite",
            kernel: "Projection",
            quality: MappingQuality::Faithful,
            notes: "Both are rebuildable views. Codex already documents this; the kernel preserves the rule.",
        },
        Mapping {
            codex: "state_5.sqlite thread metadata / spawn edges",
            kernel: "Projection + Agent parent links",
            quality: MappingQuality::ProjectionOnly,
            notes: "Codex indexes can lag JSONL. Kernel indexes are always rebuilt from the log.",
        },
        Mapping {
            codex: "queue_1.sqlite user messages",
            kernel: "Task READY + scheduler",
            quality: MappingQuality::Approximate,
            notes: "Codex queue is durable user input. Kernel tasks are durable work nodes.",
        },
        Mapping {
            codex: "in-memory mailbox",
            kernel: "JoinChildTerminal event",
            quality: MappingQuality::Unmapped,
            notes: "Mailbox is lost on restart. Kernel child completion is an event.",
        },
        Mapping {
            codex: "InitialHistory::Forked(Vec<RolloutItem>)",
            kernel: "Context snapshot + delta",
            quality: MappingQuality::Approximate,
            notes: "Codex copies sanitized parent rollout items into the child. Kernel shares content-addressed chunks.",
        },
        Mapping {
            codex: "exec-server ProcessId / resume_session_id",
            kernel: "Lease + executor_id",
            quality: MappingQuality::Approximate,
            notes: "Remote executor becomes a leased worker; it cannot define completion outside ProcessExited/commit.",
        },
        Mapping {
            codex: "ApprovalAction / Guardian",
            kernel: "CapabilityGranted + approval_evidence",
            quality: MappingQuality::Approximate,
            notes: "Approvals become durable capability decisions bound to an operation or agent.",
        },
        Mapping {
            codex: "AgentStatus::Interrupted",
            kernel: "AgentStatus::Interrupted (non-terminal)",
            quality: MappingQuality::Faithful,
            notes: "Preserved: interrupted is resumable, matching Codex is_final().",
        },
        Mapping {
            codex: "app-server CommandExecManager",
            kernel: "Operation (same log)",
            quality: MappingQuality::Unmapped,
            notes: "A second process manager must not exist. Client exec should create kernel operations.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_table_is_nonempty() {
        assert!(mappings().len() >= 10);
        assert!(mappings()
            .iter()
            .any(|m| m.codex.contains("wait_agent") && m.quality == MappingQuality::Unmapped));
    }
}
