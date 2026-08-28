use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::context::ContentStore;
use crate::error::KernelError;
use crate::event::{Event, OperationStatus};
use crate::ids::{ExecutionId, IdempotencyKey, LeaseOwnerId, OperationId};
use crate::kernel::Kernel;
use crate::log::EventLog;
use crate::process::{
    process_liveness, LaunchSpec, ProcessLiveness, ProcessReport, ProcessSupervisor,
};
use crate::projection::Projection;
use crate::scheduler::{schedule, SchedulerConfig};
use crate::state::state_hash;

struct LiveOp {
    generation: u64,
}

pub struct Runtime {
    pub kernel: Kernel,
    pub supervisor: ProcessSupervisor,
    pub cas: ContentStore,
    pub projection: Projection,
    pub owner: LeaseOwnerId,
    pub executor_id: String,
    live: BTreeMap<OperationId, LiveOp>,
    cfg: SchedulerConfig,
    root: PathBuf,
}

impl Runtime {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, KernelError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let log = EventLog::open_or_create(root.join("events.cak"))?;
        let kernel = Kernel::from_log(log)?;
        let cas = ContentStore::open(root.join("cas"))?;
        let projection = Projection::open(root.join("projection.sqlite"))?;
        let mut rt = Self {
            kernel,
            supervisor: ProcessSupervisor::new(),
            cas,
            projection,
            owner: LeaseOwnerId::new(),
            executor_id: "local".to_string(),
            live: BTreeMap::new(),
            cfg: SchedulerConfig::default(),
            root,
        };
        rt.recover_live_processes()?;
        Ok(rt)
    }

    pub fn create(root: impl AsRef<Path>, note: &str) -> Result<Self, KernelError> {
        let mut rt = Self::open(root)?;
        if rt.kernel.state.executions.is_empty() {
            rt.kernel.create_execution(note)?;
        }
        rt.project()?;
        Ok(rt)
    }

    pub fn execution_id(&self) -> Result<ExecutionId, KernelError> {
        self.kernel.execution_id()
    }

    pub fn drive(&mut self, max_steps: usize) -> Result<usize, KernelError> {
        let mut steps = 0;
        for _ in 0..max_steps {
            let mut progress = false;
            while let Some(report) = self.supervisor.try_recv()? {
                self.apply_report(report)?;
                progress = true;
                steps += 1;
            }
            let decision = {
                let exec = self.kernel.execution()?;
                schedule(exec, &self.cfg)?
            };
            for agent_id in decision.runnable_agents {
                self.kernel.start_agent(agent_id)?;
                progress = true;
                steps += 1;
            }
            for operation_id in decision.runnable_operations {
                self.launch_operation(operation_id)?;
                progress = true;
                steps += 1;
            }
            if !progress {
                break;
            }
        }
        self.project()?;
        Ok(steps)
    }

    pub fn wait_idle(&mut self, timeout: Duration) -> Result<(), KernelError> {
        let start = std::time::Instant::now();
        loop {
            self.drive(32)?;
            let exec = self.kernel.execution()?;
            let live = exec.operations.values().any(|op| {
                matches!(
                    op.status,
                    OperationStatus::Ready
                        | OperationStatus::Leased
                        | OperationStatus::Running
                        | OperationStatus::WaitingExternal
                        | OperationStatus::Committing
                        | OperationStatus::Orphaned
                )
            });
            if !live && self.live.is_empty() {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(KernelError::Process("wait_idle timed out".into()));
            }
            if let Some(report) = self.supervisor.recv_timeout(Duration::from_millis(20))? {
                self.apply_report(report)?;
            }
        }
    }

    fn launch_operation(&mut self, operation_id: OperationId) -> Result<(), KernelError> {
        let (attempt_id, argv, cwd) = {
            let exec = self.kernel.execution()?;
            let op = exec
                .operations
                .get(&operation_id)
                .ok_or_else(|| KernelError::invalid("unknown operation"))?;
            let attempt = op
                .active_attempt
                .ok_or_else(|| KernelError::invalid("operation has no attempt"))?;
            (attempt, op.argv.clone(), op.cwd.clone())
        };
        let generation = self.kernel.lease_operation(
            operation_id,
            attempt_id,
            self.owner,
            self.executor_id.clone(),
            60_000,
            None,
        )?;
        if let Err(err) = self.supervisor.spawn(LaunchSpec {
            operation_id,
            attempt_id,
            argv,
            cwd,
        }) {
            self.kernel.append(
                Event::OperationFailed {
                    operation_id,
                    attempt_id,
                    lease_generation: Some(generation),
                    reason: err.to_string(),
                },
                IdempotencyKey::new("op_failed", operation_id),
            )?;
            return Err(err);
        }
        self.live.insert(operation_id, LiveOp { generation });
        Ok(())
    }

    fn apply_report(&mut self, report: ProcessReport) -> Result<(), KernelError> {
        match report {
            ProcessReport::Started {
                operation_id,
                attempt_id,
                pid,
                start_key,
            } => {
                self.kernel
                    .process_started(operation_id, attempt_id, pid, start_key)?;
            }
            ProcessReport::Output {
                operation_id,
                attempt_id,
                stream,
                offset,
                bytes,
            } => {
                let _ = self.cas.put_chunk(&bytes)?;
                self.kernel
                    .process_output(operation_id, attempt_id, stream, offset, &bytes)?;
            }
            ProcessReport::Exited {
                operation_id,
                attempt_id,
                exit_code,
                signal,
                killed_externally,
            } => {
                self.kernel.process_exited(
                    operation_id,
                    attempt_id,
                    exit_code,
                    signal,
                    killed_externally,
                )?;
                let generation = self
                    .live
                    .get(&operation_id)
                    .map(|live| live.generation)
                    .unwrap_or(1);
                self.kernel
                    .commit_operation(operation_id, attempt_id, generation, exit_code)?;
                self.live.remove(&operation_id);
            }
            ProcessReport::Failed {
                operation_id,
                attempt_id,
                reason,
            } => {
                if let Ok(status) = self.kernel.operation_status(operation_id) {
                    if status.is_terminal() {
                        self.live.remove(&operation_id);
                        return Ok(());
                    }
                }
                let generation = self.live.get(&operation_id).map(|live| live.generation);
                self.kernel.append(
                    Event::OperationFailed {
                        operation_id,
                        attempt_id,
                        lease_generation: generation,
                        reason,
                    },
                    IdempotencyKey::new("op_failed", operation_id),
                )?;
                self.live.remove(&operation_id);
            }
            ProcessReport::Unresolved {
                operation_id,
                attempt_id,
                reason,
            } => {
                if let Ok(status) = self.kernel.operation_status(operation_id) {
                    if status.is_terminal() || status == OperationStatus::Unknown {
                        self.live.remove(&operation_id);
                        return Ok(());
                    }
                }
                self.kernel.append(
                    Event::OperationUnresolved {
                        operation_id,
                        attempt_id,
                        reason,
                    },
                    IdempotencyKey::new("op_unresolved", operation_id),
                )?;
                self.live.remove(&operation_id);
            }
        }
        Ok(())
    }

    fn recover_live_processes(&mut self) -> Result<(), KernelError> {
        let exec = match self.kernel.execution() {
            Ok(exec) => exec.clone(),
            Err(_) => return Ok(()),
        };
        for op in exec.operations.values() {
            if op.status.is_terminal() {
                continue;
            }
            if self.live.contains_key(&op.id) {
                continue;
            }
            let Some(attempt_id) = op.active_attempt else {
                continue;
            };
            let Some(attempt) = op.attempts.get(&attempt_id) else {
                continue;
            };
            if attempt.exit_code.is_some() {
                continue;
            }
            let generation = op
                .lease
                .as_ref()
                .filter(|lease| !lease.expired)
                .map(|lease| lease.generation);
            let Some(start_key) = attempt.start_key.clone() else {
                if matches!(
                    op.status,
                    OperationStatus::Leased | OperationStatus::Running
                ) {
                    self.kernel.append(
                        Event::OperationFailed {
                            operation_id: op.id,
                            attempt_id,
                            lease_generation: generation,
                            reason: "incomplete-after-restart".into(),
                        },
                        IdempotencyKey::new("op_failed", op.id),
                    )?;
                }
                continue;
            };
            match process_liveness(&start_key) {
                ProcessLiveness::Alive => {
                    self.supervisor
                        .monitor_existing(op.id, attempt_id, start_key);
                    self.live.insert(
                        op.id,
                        LiveOp {
                            generation: generation.unwrap_or(1),
                        },
                    );
                }
                ProcessLiveness::Dead => {
                    // The process is gone. Do not invent exit -1; the wait
                    // status was never observed by this runtime.
                    self.kernel.append(
                        Event::OperationFailed {
                            operation_id: op.id,
                            attempt_id,
                            lease_generation: generation,
                            reason: "lost-after-restart".into(),
                        },
                        IdempotencyKey::new("op_failed", op.id),
                    )?;
                }
                ProcessLiveness::Unknown => {
                    self.kernel.append(
                        Event::OperationUnresolved {
                            operation_id: op.id,
                            attempt_id,
                            reason: "process-identity-uncertain-after-restart".into(),
                        },
                        IdempotencyKey::new("op_unresolved", op.id),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn project(&mut self) -> Result<(), KernelError> {
        let records = self.kernel.log.records().to_vec();
        self.projection.rebuild(&self.kernel.state, &records)
    }

    pub fn rebuild_projection_from_scratch(&mut self) -> Result<String, KernelError> {
        let path = self.root.join("projection.sqlite");
        let _ = std::fs::remove_file(&path);
        self.projection = Projection::open(&path)?;
        self.project()?;
        let exec = self.kernel.execution()?;
        Ok(state_hash(exec)?.to_hex())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
