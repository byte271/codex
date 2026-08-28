//! Codex Agent Kernel
//!
//! Event-sourced durable execution for goals, agents, operations, leases,
//! joins, and content-addressed context. This crate is a research overlay:
//! it does not replace Codex and is not an upstream contribution.

pub mod compat;
pub mod context;
pub mod error;
pub mod event;
pub mod ids;
pub mod kernel;
pub mod log;
pub mod process;
pub mod projection;
pub mod reducer;
pub mod replay;
pub mod runtime;
pub mod scheduler;
pub mod state;
pub mod viewer;

pub use compat::{mappings, Mapping, MappingQuality};
pub use context::{naive_child_storage_bytes, shared_child_storage_bytes, ContentStore, Snapshot};
pub use error::{KernelError, LogCorruptKind};
pub use event::{
    AgentStatus, Event, EventRecord, GoalStatus, JoinFailurePolicy, JoinKind, JoinOutcome,
    NetworkScope, OperationStatus, StreamKind, SCHEMA_VERSION,
};
pub use ids::*;
pub use kernel::Kernel;
pub use log::{EventLog, MAX_RECORD_BYTES};
pub use process::{
    kill_pid, pid_still_matches, process_liveness, LaunchSpec, ProcessLiveness, ProcessReport,
    ProcessSupervisor,
};
pub use projection::Projection;
pub use reducer::{goal_completion_preconditions_met, join_is_satisfied, reduce, reduce_all};
pub use replay::{fork_from, replay_until, ReplayReport};
pub use runtime::Runtime;
pub use scheduler::{command_allowed, schedule, ScheduleDecision, SchedulerConfig};
pub use state::{business_hash, prefix_hashes, state_hash, ExecutionState, KernelState};
pub use viewer::{render_trace, render_tree};

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod reducer_tests;
