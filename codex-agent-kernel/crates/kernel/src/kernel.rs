use crate::error::KernelError;
use crate::event::{
    AgentStatus, Event, EventRecord, JoinFailurePolicy, JoinKind, JoinOutcome, NetworkScope,
    OperationStatus, StreamKind, SCHEMA_VERSION,
};
use crate::ids::{
    AgentId, AttemptId, BarrierId, CapabilityId, ContentHash, CorrelationId, EventId, ExecutionId,
    GoalId, IdempotencyKey, JoinId, LeaseOwnerId, OperationId, SnapshotId, TaskId, TurnId,
};
use crate::log::EventLog;
use crate::reducer::{goal_completion_preconditions_met, join_is_satisfied, reduce};
use crate::state::{business_hash, state_hash, ExecutionState, KernelState};

pub struct Kernel {
    pub log: EventLog,
    pub state: KernelState,
    clock_ms: i64,
}

impl Kernel {
    pub fn from_log(log: EventLog) -> Result<Self, KernelError> {
        let state = crate::reducer::reduce_all(log.records())?;
        Ok(Self {
            log,
            state,
            clock_ms: 0,
        })
    }

    pub fn set_clock_ms(&mut self, now: i64) {
        self.clock_ms = now;
    }

    pub fn now_ms(&self) -> i64 {
        self.clock_ms
    }

    pub fn tick(&mut self, delta_ms: i64) {
        self.clock_ms = self.clock_ms.saturating_add(delta_ms);
    }

    pub fn execution_id(&self) -> Result<ExecutionId, KernelError> {
        self.state
            .executions
            .keys()
            .next()
            .copied()
            .ok_or_else(|| KernelError::invalid("kernel has no execution"))
    }

    pub fn execution(&self) -> Result<&ExecutionState, KernelError> {
        let id = self.execution_id()?;
        self.state.require_execution(id)
    }

    pub fn append(
        &mut self,
        payload: Event,
        key: IdempotencyKey,
    ) -> Result<EventRecord, KernelError> {
        self.append_caused(payload, key, None)
    }

    pub fn append_caused(
        &mut self,
        payload: Event,
        key: IdempotencyKey,
        causation_id: Option<EventId>,
    ) -> Result<EventRecord, KernelError> {
        if let Some(existing) = self.state.idempotency_index.get(&key).copied() {
            if let Some(record) = self.log.records().iter().find(|r| r.event_id == existing) {
                return Ok(record.clone());
            }
        }
        let execution_id = match &payload {
            Event::ExecutionCreated { .. } => ExecutionId::new(),
            _ => self.execution_id()?,
        };
        let seq = self.log.next_seq();
        let parent_event_id = self.log.last_event_id();
        let record = EventRecord {
            schema_version: SCHEMA_VERSION,
            execution_id,
            event_id: EventId::new(),
            seq,
            idempotency_key: key,
            causation_id,
            correlation_id: Some(CorrelationId::from_uuid(execution_id.as_uuid())),
            parent_event_id,
            occurred_at_ms: self.clock_ms,
            checksum: String::new(),
            payload,
        };
        let prepared = record.with_checksum()?;
        let mut next = self.state.clone();
        reduce(&mut next, &prepared)?;
        let stored = self.log.append(prepared)?;
        self.state = next;
        Ok(stored)
    }

    pub fn create_execution(
        &mut self,
        note: impl Into<String>,
    ) -> Result<EventRecord, KernelError> {
        let note = note.into();
        self.append(
            Event::ExecutionCreated {
                created_at_ms: self.clock_ms,
                note: note.clone(),
            },
            IdempotencyKey::new("execution_created", note),
        )
    }

    pub fn create_goal(
        &mut self,
        objective: impl Into<String>,
        required_agent_ids: Vec<AgentId>,
    ) -> Result<(GoalId, EventRecord), KernelError> {
        let goal_id = GoalId::new();
        let objective = objective.into();
        let record = self.append(
            Event::GoalCreated {
                goal_id,
                objective: objective.clone(),
                required_agent_ids,
            },
            IdempotencyKey::new("goal_created", goal_id),
        )?;
        Ok((goal_id, record))
    }

    pub fn define_acceptance(
        &mut self,
        goal_id: GoalId,
        require_barriers: Vec<BarrierId>,
    ) -> Result<EventRecord, KernelError> {
        self.append(
            Event::GoalAcceptanceDefined {
                goal_id,
                require_all_required_agents_terminal: true,
                require_no_running_operations: true,
                require_barriers,
            },
            IdempotencyKey::new("goal_acceptance", goal_id),
        )
    }

    pub fn grant_capability(
        &mut self,
        agent_id: AgentId,
        parent: Option<CapabilityId>,
        filesystem_scope: impl Into<String>,
        network: NetworkScope,
        command_allowlist: Vec<String>,
        approval_evidence: impl Into<String>,
    ) -> Result<(CapabilityId, EventRecord), KernelError> {
        let capability_id = CapabilityId::new();
        let record = self.append(
            Event::CapabilityGranted {
                capability_id,
                parent_capability_id: parent,
                agent_id,
                filesystem_scope: filesystem_scope.into(),
                network,
                command_allowlist,
                deadline_ms: None,
                approval_evidence: approval_evidence.into(),
            },
            IdempotencyKey::new("capability", capability_id),
        )?;
        Ok((capability_id, record))
    }

    pub fn spawn_agent(
        &mut self,
        parent_agent_id: Option<AgentId>,
        goal_id: GoalId,
        task: impl Into<String>,
        snapshot_id: Option<SnapshotId>,
        capability_id: Option<CapabilityId>,
    ) -> Result<(AgentId, EventRecord), KernelError> {
        let agent_id = AgentId::new();
        let record = self.append(
            Event::AgentSpawned {
                agent_id,
                parent_agent_id,
                goal_id,
                task: task.into(),
                snapshot_id,
                capability_id,
            },
            IdempotencyKey::new("agent_spawned", agent_id),
        )?;
        self.append(
            Event::AgentStatusChanged {
                agent_id,
                from: AgentStatus::Created,
                to: AgentStatus::Runnable,
                reason: "spawned".to_string(),
            },
            IdempotencyKey::new("agent_runnable", agent_id),
        )?;
        Ok((agent_id, record))
    }

    pub fn create_task(
        &mut self,
        goal_id: GoalId,
        agent_id: AgentId,
        description: impl Into<String>,
    ) -> Result<TaskId, KernelError> {
        let task_id = TaskId::new();
        self.append(
            Event::TaskCreated {
                task_id,
                goal_id,
                agent_id,
                description: description.into(),
            },
            IdempotencyKey::new("task", task_id),
        )?;
        Ok(task_id)
    }

    pub fn start_agent(&mut self, agent_id: AgentId) -> Result<EventRecord, KernelError> {
        let from = self.agent_status(agent_id)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id,
                from,
                to: AgentStatus::Running,
                reason: "scheduler".to_string(),
            },
            IdempotencyKey::new("agent_running", format!("{agent_id}:{from:?}")),
        )
    }

    pub fn finish_model_turn(
        &mut self,
        agent_id: AgentId,
        summary: impl Into<String>,
    ) -> Result<EventRecord, KernelError> {
        let turn_id = TurnId::new();
        self.append(
            Event::ModelTurnFinished {
                turn_id,
                agent_id,
                summary: summary.into(),
            },
            IdempotencyKey::new("model_turn", turn_id),
        )
    }

    pub fn complete_agent(&mut self, agent_id: AgentId) -> Result<EventRecord, KernelError> {
        let from = self.agent_status(agent_id)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id,
                from,
                to: AgentStatus::Succeeded,
                reason: "agent work finished".to_string(),
            },
            IdempotencyKey::new("agent_succeeded", agent_id),
        )
    }

    pub fn fail_agent(
        &mut self,
        agent_id: AgentId,
        reason: impl Into<String>,
    ) -> Result<EventRecord, KernelError> {
        let from = self.agent_status(agent_id)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id,
                from,
                to: AgentStatus::Failed,
                reason: reason.into(),
            },
            IdempotencyKey::new("agent_failed", agent_id),
        )
    }

    pub fn cancel_agent(&mut self, agent_id: AgentId) -> Result<EventRecord, KernelError> {
        let from = self.agent_status(agent_id)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id,
                from,
                to: AgentStatus::Cancelled,
                reason: "cancel".to_string(),
            },
            IdempotencyKey::new("agent_cancelled", agent_id),
        )
    }

    pub fn create_join(
        &mut self,
        waiter_agent_id: AgentId,
        children: Vec<AgentId>,
        kind: JoinKind,
        failure_policy: JoinFailurePolicy,
    ) -> Result<(JoinId, EventRecord), KernelError> {
        let join_id = JoinId::new();
        let record = self.append(
            Event::JoinCreated {
                join_id,
                waiter_agent_id,
                children,
                kind,
                failure_policy,
            },
            IdempotencyKey::new("join_created", join_id),
        )?;
        let from = self.agent_status(waiter_agent_id)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id: waiter_agent_id,
                from,
                to: AgentStatus::WaitingJoin,
                reason: format!("join {join_id}"),
            },
            IdempotencyKey::new("join_wait", join_id),
        )?;
        Ok((join_id, record))
    }

    pub fn observe_join_child(
        &mut self,
        join_id: JoinId,
        child_id: AgentId,
    ) -> Result<(), KernelError> {
        let exec = self.execution()?;
        let child = exec
            .agents
            .get(&child_id)
            .ok_or_else(|| KernelError::invalid("unknown child"))?;
        if !child.status.is_terminal() {
            return Ok(());
        }
        let child_status = child.status;
        self.append(
            Event::JoinChildTerminal {
                join_id,
                child_id,
                child_status,
            },
            IdempotencyKey::new("join_child", format!("{join_id}:{child_id}")),
        )?;
        self.maybe_satisfy_join(join_id)?;
        Ok(())
    }

    pub fn maybe_satisfy_join(
        &mut self,
        join_id: JoinId,
    ) -> Result<Option<JoinOutcome>, KernelError> {
        let exec = self.execution()?;
        let Some(outcome) = join_is_satisfied(exec, join_id)? else {
            return Ok(None);
        };
        let waiter = exec
            .joins
            .get(&join_id)
            .map(|j| j.waiter_agent_id)
            .ok_or_else(|| KernelError::invalid("unknown join"))?;
        self.append(
            Event::JoinSatisfied { join_id, outcome },
            IdempotencyKey::new("join_satisfied", join_id),
        )?;
        let to = match outcome {
            JoinOutcome::Succeeded => AgentStatus::Succeeded,
            JoinOutcome::Failed => AgentStatus::Failed,
            JoinOutcome::Cancelled => AgentStatus::Cancelled,
        };
        let from = self.agent_status(waiter)?;
        self.append(
            Event::AgentStatusChanged {
                agent_id: waiter,
                from,
                to,
                reason: format!("join {join_id} {outcome:?}"),
            },
            IdempotencyKey::new("join_waiter_done", join_id),
        )?;
        Ok(Some(outcome))
    }

    pub fn try_complete_goal(&mut self, goal_id: GoalId) -> Result<EventRecord, KernelError> {
        let exec = self.execution()?;
        if !goal_completion_preconditions_met(exec, goal_id)? {
            return Err(KernelError::invalid(
                "GOAL_COMPLETED preconditions not met; MODEL_FINISHED_TURN is not sufficient",
            ));
        }
        self.append(
            Event::GoalCompleted { goal_id },
            IdempotencyKey::new("goal_completed", goal_id),
        )
    }

    pub fn create_operation(
        &mut self,
        agent_id: AgentId,
        argv: Vec<String>,
        cwd: impl Into<String>,
        executor_hint: Option<String>,
    ) -> Result<(OperationId, AttemptId, EventRecord), KernelError> {
        let operation_id = OperationId::new();
        let attempt_id = AttemptId::new();
        let record = self.append(
            Event::OperationCreated {
                operation_id,
                agent_id,
                attempt_id,
                argv,
                cwd: cwd.into(),
                executor_hint,
            },
            IdempotencyKey::new("op_created", operation_id),
        )?;
        self.append(
            Event::OperationScheduled { operation_id },
            IdempotencyKey::new("op_scheduled", operation_id),
        )?;
        Ok((operation_id, attempt_id, record))
    }

    pub fn lease_operation(
        &mut self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        owner: LeaseOwnerId,
        executor_id: impl Into<String>,
        ttl_ms: i64,
        capability_id: Option<CapabilityId>,
    ) -> Result<u64, KernelError> {
        let exec = self.execution()?;
        let op = exec
            .operations
            .get(&operation_id)
            .ok_or_else(|| KernelError::invalid("unknown operation"))?;
        let generation = op.lease.as_ref().map(|l| l.generation + 1).unwrap_or(1);
        self.append(
            Event::OperationLeased {
                operation_id,
                attempt_id,
                generation,
                owner,
                expires_at_ms: self.clock_ms.saturating_add(ttl_ms),
                capability_id,
                executor_id: executor_id.into(),
            },
            IdempotencyKey::new("op_leased", format!("{operation_id}:{generation}")),
        )?;
        Ok(generation)
    }

    pub fn process_started(
        &mut self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        pid: u32,
        start_key: impl Into<String>,
    ) -> Result<EventRecord, KernelError> {
        self.append(
            Event::ProcessStarted {
                operation_id,
                attempt_id,
                pid,
                start_key: start_key.into(),
            },
            IdempotencyKey::new("proc_started", format!("{operation_id}:{attempt_id}")),
        )
    }

    pub fn process_output(
        &mut self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        stream: StreamKind,
        offset: u64,
        bytes: &[u8],
    ) -> Result<EventRecord, KernelError> {
        let chunk_hash = ContentHash::of_bytes(bytes);
        let stream_name = match stream {
            StreamKind::Stdout => "stdout",
            StreamKind::Stderr => "stderr",
        };
        self.append(
            Event::ProcessOutputAvailable {
                operation_id,
                attempt_id,
                stream,
                offset,
                byte_len: bytes.len() as u64,
                chunk_hash,
            },
            IdempotencyKey::new(
                "proc_out",
                format!(
                    "{operation_id}:{attempt_id}:{stream_name}:{offset}:{}:{chunk_hash}",
                    bytes.len()
                ),
            ),
        )
    }

    pub fn process_exited(
        &mut self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        exit_code: i32,
        signal: Option<String>,
        killed_externally: bool,
    ) -> Result<EventRecord, KernelError> {
        self.append(
            Event::ProcessExited {
                operation_id,
                attempt_id,
                exit_code,
                signal,
                killed_externally,
            },
            IdempotencyKey::new("proc_exited", format!("{operation_id}:{attempt_id}")),
        )
    }

    pub fn commit_operation(
        &mut self,
        operation_id: OperationId,
        attempt_id: AttemptId,
        lease_generation: u64,
        exit_code: i32,
    ) -> Result<EventRecord, KernelError> {
        self.append(
            Event::OperationCommitted {
                operation_id,
                attempt_id,
                lease_generation,
                exit_code,
            },
            IdempotencyKey::new("op_committed", format!("{operation_id}:{attempt_id}")),
        )
    }

    pub fn expire_lease(
        &mut self,
        operation_id: OperationId,
        generation: u64,
    ) -> Result<EventRecord, KernelError> {
        self.append(
            Event::LeaseExpired {
                operation_id,
                generation,
            },
            IdempotencyKey::new("lease_expired", format!("{operation_id}:{generation}")),
        )
    }

    pub fn checkpoint(&mut self) -> Result<EventRecord, KernelError> {
        let exec = self.execution()?;
        let hash = state_hash(exec)?;
        let seq = self.log.next_seq();
        self.append(
            Event::CheckpointTaken {
                seq,
                state_hash: hash.to_hex(),
            },
            IdempotencyKey::new("checkpoint", seq),
        )
    }

    pub fn create_barrier(
        &mut self,
        expected_arrivals: u32,
    ) -> Result<(BarrierId, EventRecord), KernelError> {
        let barrier_id = BarrierId::new();
        let record = self.append(
            Event::BarrierCreated {
                barrier_id,
                expected_arrivals,
            },
            IdempotencyKey::new("barrier", barrier_id),
        )?;
        Ok((barrier_id, record))
    }

    pub fn arrive_barrier(
        &mut self,
        barrier_id: BarrierId,
        agent_id: AgentId,
    ) -> Result<(), KernelError> {
        self.append(
            Event::BarrierArrived {
                barrier_id,
                agent_id,
            },
            IdempotencyKey::new("barrier_arrive", format!("{barrier_id}:{agent_id}")),
        )?;
        let exec = self.execution()?;
        let barrier = exec
            .barriers
            .get(&barrier_id)
            .ok_or_else(|| KernelError::invalid("unknown barrier"))?;
        if barrier.arrivals.len() as u32 >= barrier.expected_arrivals && !barrier.satisfied {
            self.append(
                Event::BarrierSatisfied { barrier_id },
                IdempotencyKey::new("barrier_satisfied", barrier_id),
            )?;
        }
        Ok(())
    }

    pub fn attach_snapshot(
        &mut self,
        agent_id: AgentId,
        parent_snapshot_id: Option<SnapshotId>,
        chunk_hashes: Vec<ContentHash>,
        byte_len: u64,
    ) -> Result<SnapshotId, KernelError> {
        let snapshot_id = SnapshotId::new();
        self.append(
            Event::ContextSnapshotAttached {
                snapshot_id,
                agent_id,
                parent_snapshot_id,
                chunk_hashes,
                byte_len,
            },
            IdempotencyKey::new("snapshot", snapshot_id),
        )?;
        Ok(snapshot_id)
    }

    pub fn hashes(&self) -> Result<(String, String), KernelError> {
        let exec = self.execution()?;
        Ok((state_hash(exec)?.to_hex(), business_hash(exec)?.to_hex()))
    }

    fn agent_status(&self, agent_id: AgentId) -> Result<AgentStatus, KernelError> {
        let exec = self.execution()?;
        exec.agents
            .get(&agent_id)
            .map(|a| a.status)
            .ok_or_else(|| KernelError::invalid(format!("unknown agent {agent_id}")))
    }

    pub fn operation_status(
        &self,
        operation_id: OperationId,
    ) -> Result<OperationStatus, KernelError> {
        let exec = self.execution()?;
        exec.operations
            .get(&operation_id)
            .map(|o| o.status)
            .ok_or_else(|| KernelError::invalid(format!("unknown operation {operation_id}")))
    }
}
