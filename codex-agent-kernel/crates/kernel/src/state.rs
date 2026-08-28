use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::event::{
    AgentStatus, EventRecord, GoalStatus, JoinFailurePolicy, JoinKind, JoinOutcome, NetworkScope,
    OperationStatus,
};
use crate::ids::{
    AgentId, AttemptId, BarrierId, CapabilityId, ContentHash, EventId, ExecutionId, GoalId,
    IdempotencyKey, JoinId, LeaseOwnerId, OperationId, SnapshotId, StateHash, TaskId, TurnId,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelState {
    pub executions: BTreeMap<ExecutionId, ExecutionState>,
    pub seen_event_ids: BTreeSet<EventId>,
    pub idempotency_index: BTreeMap<IdempotencyKey, EventId>,
    pub last_seq: BTreeMap<ExecutionId, u64>,
}

impl KernelState {
    pub fn execution(&self, id: ExecutionId) -> Option<&ExecutionState> {
        self.executions.get(&id)
    }

    pub fn execution_mut(&mut self, id: ExecutionId) -> Option<&mut ExecutionState> {
        self.executions.get_mut(&id)
    }

    pub fn require_execution(
        &self,
        id: ExecutionId,
    ) -> Result<&ExecutionState, crate::error::KernelError> {
        self.executions
            .get(&id)
            .ok_or_else(|| crate::error::KernelError::ExecutionNotFound(id.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionState {
    pub id: ExecutionId,
    pub created_at_ms: i64,
    pub note: String,
    pub ancestor: Option<ForkAncestry>,
    pub goals: BTreeMap<GoalId, GoalState>,
    pub agents: BTreeMap<AgentId, AgentState>,
    pub tasks: BTreeMap<TaskId, TaskState>,
    pub operations: BTreeMap<OperationId, OperationState>,
    pub joins: BTreeMap<JoinId, JoinState>,
    pub barriers: BTreeMap<BarrierId, BarrierState>,
    pub capabilities: BTreeMap<CapabilityId, CapabilityState>,
    pub snapshots: BTreeMap<SnapshotId, SnapshotState>,
    pub model_turns: Vec<ModelTurnState>,
    pub checkpoints: Vec<CheckpointState>,
}

impl ExecutionState {
    pub fn new(id: ExecutionId, created_at_ms: i64, note: String) -> Self {
        Self {
            id,
            created_at_ms,
            note,
            ancestor: None,
            goals: BTreeMap::new(),
            agents: BTreeMap::new(),
            tasks: BTreeMap::new(),
            operations: BTreeMap::new(),
            joins: BTreeMap::new(),
            barriers: BTreeMap::new(),
            capabilities: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            model_turns: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    pub fn business_view(&self) -> BusinessStateView<'_> {
        BusinessStateView {
            goals: &self.goals,
            agents: &self.agents,
            tasks: &self.tasks,
            operations: &self.operations,
            joins: &self.joins,
            barriers: &self.barriers,
            capabilities: &self.capabilities,
            snapshots: &self.snapshots,
            model_turns: &self.model_turns,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkAncestry {
    pub source_execution_id: ExecutionId,
    pub from_event_id: EventId,
    pub from_seq: u64,
    pub source_business_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalState {
    pub id: GoalId,
    pub objective: String,
    pub status: GoalStatus,
    pub required_agent_ids: Vec<AgentId>,
    pub require_all_required_agents_terminal: bool,
    pub require_no_running_operations: bool,
    pub require_barriers: Vec<BarrierId>,
    pub model_turns_finished: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub goal_id: GoalId,
    pub task: String,
    pub status: AgentStatus,
    pub snapshot_id: Option<SnapshotId>,
    pub capability_id: Option<CapabilityId>,
    pub retained: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskState {
    pub id: TaskId,
    pub goal_id: GoalId,
    pub agent_id: AgentId,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseState {
    pub generation: u64,
    pub owner: LeaseOwnerId,
    pub expires_at_ms: i64,
    pub executor_id: String,
    pub capability_id: Option<CapabilityId>,
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptState {
    pub id: AttemptId,
    pub pid: Option<u32>,
    pub start_key: Option<String>,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub killed_externally: bool,
    pub output_chunks: Vec<OutputChunk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputChunk {
    pub stream: crate::event::StreamKind,
    pub offset: u64,
    pub byte_len: u64,
    pub chunk_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationState {
    pub id: OperationId,
    pub agent_id: AgentId,
    pub status: OperationStatus,
    pub argv: Vec<String>,
    pub cwd: String,
    pub executor_hint: Option<String>,
    pub attempts: BTreeMap<AttemptId, AttemptState>,
    pub active_attempt: Option<AttemptId>,
    pub lease: Option<LeaseState>,
    pub committed_attempt: Option<AttemptId>,
    pub committed_exit_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinState {
    pub id: JoinId,
    pub waiter_agent_id: AgentId,
    pub children: Vec<AgentId>,
    pub kind: JoinKind,
    pub failure_policy: JoinFailurePolicy,
    pub observed: BTreeMap<AgentId, AgentStatus>,
    pub outcome: Option<JoinOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarrierState {
    pub id: BarrierId,
    pub expected_arrivals: u32,
    pub arrivals: Vec<AgentId>,
    pub satisfied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityState {
    pub id: CapabilityId,
    pub parent_capability_id: Option<CapabilityId>,
    pub agent_id: AgentId,
    pub filesystem_scope: String,
    pub network: NetworkScope,
    pub command_allowlist: Vec<String>,
    pub deadline_ms: Option<i64>,
    pub approval_evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotState {
    pub id: SnapshotId,
    pub agent_id: AgentId,
    pub parent_snapshot_id: Option<SnapshotId>,
    pub chunk_hashes: Vec<ContentHash>,
    pub byte_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTurnState {
    pub turn_id: TurnId,
    pub agent_id: AgentId,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointState {
    pub seq: u64,
    pub state_hash: String,
}

#[derive(Serialize)]
pub struct BusinessStateView<'a> {
    pub goals: &'a BTreeMap<GoalId, GoalState>,
    pub agents: &'a BTreeMap<AgentId, AgentState>,
    pub tasks: &'a BTreeMap<TaskId, TaskState>,
    pub operations: &'a BTreeMap<OperationId, OperationState>,
    pub joins: &'a BTreeMap<JoinId, JoinState>,
    pub barriers: &'a BTreeMap<BarrierId, BarrierState>,
    pub capabilities: &'a BTreeMap<CapabilityId, CapabilityState>,
    pub snapshots: &'a BTreeMap<SnapshotId, SnapshotState>,
    pub model_turns: &'a Vec<ModelTurnState>,
}

pub fn state_hash(execution: &ExecutionState) -> Result<StateHash, crate::error::KernelError> {
    let bytes = serde_json::to_vec(execution)?;
    Ok(StateHash::of_bytes(&bytes))
}

pub fn business_hash(execution: &ExecutionState) -> Result<StateHash, crate::error::KernelError> {
    let bytes = serde_json::to_vec(&execution.business_view())?;
    Ok(StateHash::of_bytes(&bytes))
}

pub fn prefix_hashes(
    records: &[EventRecord],
) -> Result<Vec<(EventId, StateHash, StateHash)>, crate::error::KernelError> {
    let mut state = KernelState::default();
    let mut out = Vec::with_capacity(records.len());
    for record in records {
        crate::reducer::reduce(&mut state, record)?;
        if let Some(exec) = state.execution(record.execution_id) {
            out.push((record.event_id, state_hash(exec)?, business_hash(exec)?));
        }
    }
    Ok(out)
}
