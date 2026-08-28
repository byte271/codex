use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::ids::{
    AgentId, AttemptId, BarrierId, CapabilityId, ContentHash, CorrelationId, EventId, ExecutionId,
    GoalId, IdempotencyKey, JoinId, LeaseOwnerId, OperationId, SnapshotId, TaskId, TurnId,
};

pub const SCHEMA_VERSION: u16 = 1;
pub const LOG_MAGIC: &[u8; 4] = b"CAK1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Created,
    Runnable,
    Running,
    WaitingJoin,
    WaitingExternal,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
    Orphaned,
}

impl AgentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Created
                | Self::Runnable
                | Self::Running
                | Self::WaitingJoin
                | Self::WaitingExternal
                | Self::Interrupted
                | Self::Orphaned
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl GoalStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Created,
    Ready,
    Leased,
    Running,
    WaitingExternal,
    Committing,
    Succeeded,
    Failed,
    Cancelled,
    Orphaned,
    Unknown,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn can_become_running(self) -> bool {
        matches!(
            self,
            Self::Leased | Self::WaitingExternal | Self::Ready | Self::Created
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    All,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinFailurePolicy {
    WaitAll,
    FailFast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkScope {
    Deny,
    Allow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ExecutionCreated {
        created_at_ms: i64,
        note: String,
    },
    ExecutionForked {
        source_execution_id: ExecutionId,
        from_event_id: EventId,
        from_seq: u64,
        source_business_hash: String,
    },
    GoalCreated {
        goal_id: GoalId,
        objective: String,
        required_agent_ids: Vec<AgentId>,
    },
    GoalAcceptanceDefined {
        goal_id: GoalId,
        require_all_required_agents_terminal: bool,
        require_no_running_operations: bool,
        require_barriers: Vec<BarrierId>,
    },
    ModelTurnFinished {
        turn_id: TurnId,
        agent_id: AgentId,
        summary: String,
    },
    GoalCompleted {
        goal_id: GoalId,
    },
    GoalFailed {
        goal_id: GoalId,
        reason: String,
    },
    AgentSpawned {
        agent_id: AgentId,
        parent_agent_id: Option<AgentId>,
        goal_id: GoalId,
        task: String,
        snapshot_id: Option<SnapshotId>,
        capability_id: Option<CapabilityId>,
    },
    AgentStatusChanged {
        agent_id: AgentId,
        from: AgentStatus,
        to: AgentStatus,
        reason: String,
    },
    TaskCreated {
        task_id: TaskId,
        goal_id: GoalId,
        agent_id: AgentId,
        description: String,
    },
    JoinCreated {
        join_id: JoinId,
        waiter_agent_id: AgentId,
        children: Vec<AgentId>,
        kind: JoinKind,
        failure_policy: JoinFailurePolicy,
    },
    JoinChildTerminal {
        join_id: JoinId,
        child_id: AgentId,
        child_status: AgentStatus,
    },
    JoinSatisfied {
        join_id: JoinId,
        outcome: JoinOutcome,
    },
    OperationCreated {
        operation_id: OperationId,
        agent_id: AgentId,
        attempt_id: AttemptId,
        argv: Vec<String>,
        cwd: String,
        executor_hint: Option<String>,
    },
    OperationScheduled {
        operation_id: OperationId,
    },
    OperationLeased {
        operation_id: OperationId,
        attempt_id: AttemptId,
        generation: u64,
        owner: LeaseOwnerId,
        expires_at_ms: i64,
        capability_id: Option<CapabilityId>,
        executor_id: String,
    },
    ProcessStarted {
        operation_id: OperationId,
        attempt_id: AttemptId,
        pid: u32,
        start_key: String,
    },
    ProcessOutputAvailable {
        operation_id: OperationId,
        attempt_id: AttemptId,
        stream: StreamKind,
        offset: u64,
        byte_len: u64,
        chunk_hash: ContentHash,
    },
    ProcessExited {
        operation_id: OperationId,
        attempt_id: AttemptId,
        exit_code: i32,
        signal: Option<String>,
        killed_externally: bool,
    },
    OperationCommitted {
        operation_id: OperationId,
        attempt_id: AttemptId,
        lease_generation: u64,
        exit_code: i32,
    },
    OperationFailed {
        operation_id: OperationId,
        attempt_id: AttemptId,
        lease_generation: Option<u64>,
        reason: String,
    },
    OperationUnresolved {
        operation_id: OperationId,
        attempt_id: AttemptId,
        reason: String,
    },
    OperationCancelled {
        operation_id: OperationId,
        reason: String,
    },
    LeaseExpired {
        operation_id: OperationId,
        generation: u64,
    },
    CheckpointTaken {
        seq: u64,
        state_hash: String,
    },
    ContextSnapshotAttached {
        snapshot_id: SnapshotId,
        agent_id: AgentId,
        parent_snapshot_id: Option<SnapshotId>,
        chunk_hashes: Vec<ContentHash>,
        byte_len: u64,
    },
    CapabilityGranted {
        capability_id: CapabilityId,
        parent_capability_id: Option<CapabilityId>,
        agent_id: AgentId,
        filesystem_scope: String,
        network: NetworkScope,
        command_allowlist: Vec<String>,
        deadline_ms: Option<i64>,
        approval_evidence: String,
    },
    BarrierCreated {
        barrier_id: BarrierId,
        expected_arrivals: u32,
    },
    BarrierArrived {
        barrier_id: BarrierId,
        agent_id: AgentId,
    },
    BarrierSatisfied {
        barrier_id: BarrierId,
    },
    RetentionChanged {
        agent_id: AgentId,
        retained: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub schema_version: u16,
    pub execution_id: ExecutionId,
    pub event_id: EventId,
    pub seq: u64,
    pub idempotency_key: IdempotencyKey,
    pub causation_id: Option<EventId>,
    pub correlation_id: Option<CorrelationId>,
    pub parent_event_id: Option<EventId>,
    pub occurred_at_ms: i64,
    pub checksum: String,
    pub payload: Event,
}

impl EventRecord {
    pub fn canonical_payload_bytes(&self) -> Result<Vec<u8>, KernelError> {
        let mut clone = self.clone();
        clone.checksum = String::new();
        Ok(serde_json::to_vec(&clone)?)
    }

    pub fn compute_checksum(&self) -> Result<String, KernelError> {
        let bytes = self.canonical_payload_bytes()?;
        Ok(ContentHash::of_bytes(&bytes).to_hex())
    }

    pub fn with_checksum(mut self) -> Result<Self, KernelError> {
        self.checksum = self.compute_checksum()?;
        Ok(self)
    }

    pub fn verify_checksum(&self) -> Result<(), KernelError> {
        let expected = self.compute_checksum()?;
        if expected != self.checksum {
            return Err(KernelError::ChecksumMismatch);
        }
        Ok(())
    }
}
