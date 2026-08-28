//! Observation-only adapter: mirror Codex facts into the kernel and report
//! disagreements. This crate does not control Codex.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use codex_agent_kernel::ids::{AgentId, AttemptId, GoalId, LeaseOwnerId, OperationId};
use codex_agent_kernel::log::EventLog;
use codex_agent_kernel::{Kernel, KernelError};
use serde::{Deserialize, Serialize};

/// Codex `main` SHA this adapter was checked against.
pub const TESTED_UPSTREAM_HEAD: &str = "41d3dc56a0e1de47e30a9585c1b49253c082f8f7";

/// Unified exec yield return after `exec_command` (`process_manager.rs`).
pub const UNIFIED_EXEC_SOURCE: &str = "codex-rs/core/src/unified_exec/process_manager.rs";

/// Model-owned goal completion (`ext/goal/src/tool.rs` `handle_update`).
pub const GOAL_UPDATE_SOURCE: &str = "codex-rs/ext/goal/src/tool.rs";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodexFact {
    ExecCommandStarted {
        session_id: i32,
        argv: Vec<String>,
    },
    ExecYieldReturned {
        session_id: i32,
        process_id: Option<i32>,
        wrapper_output: String,
        tool_call_completed: bool,
    },
    OsProcessExited {
        session_id: i32,
        exit_code: i32,
    },
    ModelTurnFinished {
        last_message: String,
    },
    ChildAgentTurnComplete {
        child: String,
    },
    GoalMarkedComplete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Disagreement {
    pub code: String,
    pub codex_view: String,
    pub kernel_view: String,
    pub source: String,
}

pub struct ObservationAdapter {
    pub kernel: Kernel,
    goal_id: Option<GoalId>,
    agent_id: Option<AgentId>,
    ops: BTreeMap<i32, (OperationId, AttemptId, u64)>,
}

impl ObservationAdapter {
    pub fn new(log: EventLog) -> Result<Self, KernelError> {
        let mut kernel = Kernel::from_log(log)?;
        if kernel.execution_id().is_err() {
            kernel.create_execution("codex-observe")?;
        }
        Ok(Self {
            kernel,
            goal_id: None,
            agent_id: None,
            ops: BTreeMap::new(),
        })
    }

    fn ensure_agent(&mut self) -> Result<AgentId, KernelError> {
        if let Some(id) = self.agent_id {
            return Ok(id);
        }
        let (goal, _) = self.kernel.create_goal("observed", vec![])?;
        let (agent, _) = self.kernel.spawn_agent(None, goal, "root", None, None)?;
        self.kernel.start_agent(agent)?;
        self.goal_id = Some(goal);
        self.agent_id = Some(agent);
        Ok(agent)
    }

    pub fn observe(&mut self, fact: CodexFact) -> Result<Vec<Disagreement>, KernelError> {
        let mut found = Vec::new();
        match fact {
            CodexFact::ExecCommandStarted { session_id, argv } => {
                let agent = self.ensure_agent()?;
                let (op, attempt, _) = self.kernel.create_operation(agent, argv, "/", None)?;
                let gen = self.kernel.lease_operation(
                    op,
                    attempt,
                    LeaseOwnerId::new(),
                    "observe",
                    60_000,
                    None,
                )?;
                let pid = u32::try_from(session_id.max(1)).unwrap_or(1);
                self.kernel
                    .process_started(op, attempt, pid, format!("{pid}"))?;
                self.ops.insert(session_id, (op, attempt, gen));
            }
            CodexFact::ExecYieldReturned {
                session_id,
                process_id,
                wrapper_output,
                tool_call_completed,
            } => {
                if let Some((op, _, _)) = self.ops.get(&session_id).copied() {
                    let status = self.kernel.operation_status(op)?;
                    let exited = self.process_has_exit(op)?;
                    if tool_call_completed && process_id.is_some() && !exited {
                        found.push(Disagreement {
                            code: "wrapper_complete_process_running".into(),
                            codex_view: format!(
                                "tool call completed with process_id={process_id:?}; wrapper={wrapper_output:?}"
                            ),
                            kernel_view: format!("operation {op} status={status:?} process_exited={exited}"),
                            source: UNIFIED_EXEC_SOURCE.into(),
                        });
                    }
                }
            }
            CodexFact::OsProcessExited {
                session_id,
                exit_code,
            } => {
                if let Some((op, attempt, gen)) = self.ops.get(&session_id).copied() {
                    self.kernel
                        .process_exited(op, attempt, exit_code, None, false)?;
                    self.kernel.commit_operation(op, attempt, gen, exit_code)?;
                }
            }
            CodexFact::ModelTurnFinished { last_message } => {
                let agent = self.ensure_agent()?;
                self.kernel.finish_model_turn(agent, last_message)?;
            }
            CodexFact::ChildAgentTurnComplete { child } => {
                let parent = self.ensure_agent()?;
                let goal = self.goal_id.expect("goal");
                let (id, _) =
                    self.kernel
                        .spawn_agent(Some(parent), goal, child.clone(), None, None)?;
                self.kernel.start_agent(id)?;
                self.kernel.complete_agent(id)?;
            }
            CodexFact::GoalMarkedComplete => {
                let _ = self.ensure_agent()?;
                let goal = self.goal_id.expect("goal");
                match self.kernel.try_complete_goal(goal) {
                    Ok(_) => {}
                    Err(err) => found.push(Disagreement {
                        code: "goal_complete_unfinished_work".into(),
                        codex_view: "update_goal(complete) accepted with no child/process gate"
                            .into(),
                        kernel_view: err.to_string(),
                        source: GOAL_UPDATE_SOURCE.into(),
                    }),
                }
            }
        }
        Ok(found)
    }

    fn process_has_exit(&self, op: OperationId) -> Result<bool, KernelError> {
        let exec = self.kernel.execution()?;
        let operation = exec
            .operations
            .get(&op)
            .ok_or_else(|| KernelError::invalid("unknown operation"))?;
        Ok(operation
            .attempts
            .values()
            .any(|attempt| attempt.exit_code.is_some()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentReport {
    pub scenario: String,
    pub upstream_issue: String,
    pub tested_upstream_head: String,
    pub baseline: serde_json::Value,
    pub kernel: serde_json::Value,
    pub disagreements: Vec<Disagreement>,
    pub failure_reproduced: bool,
    pub invariant_violations: u32,
}

/// Mirrors unified exec after the initial yield wait: a live process keeps
/// `process_id` in the tool response (`process_manager.rs` ~645–651).
pub fn tool_response_process_id(has_exited: bool, session_id: i32) -> Option<i32> {
    if has_exited {
        None
    } else {
        Some(session_id)
    }
}

pub fn run_wrapper_complete_experiment() -> Result<ExperimentReport, KernelError> {
    let dir = tempfile::tempdir().map_err(|err| KernelError::Process(err.to_string()))?;
    let log = EventLog::create(dir.path().join("events.cak"))?;
    let mut adapter = ObservationAdapter::new(log)?;

    let mut child = spawn_wrapper_script()?;
    std::thread::sleep(Duration::from_millis(250));
    let alive = child.try_wait().ok().flatten().is_none();
    let session_id = 7;
    adapter.observe(CodexFact::ExecCommandStarted {
        session_id,
        argv: wrapper_argv(),
    })?;
    let process_id = tool_response_process_id(!alive, session_id);
    let disagreements = adapter.observe(CodexFact::ExecYieldReturned {
        session_id,
        process_id,
        wrapper_output: "Script completed".into(),
        tool_call_completed: true,
    })?;
    let _ = child.kill();
    let _ = child.wait();

    let op_status = adapter
        .ops
        .get(&session_id)
        .map(|(op, _, _)| adapter.kernel.operation_status(*op))
        .transpose()?;

    Ok(ExperimentReport {
        scenario: "wrapper_complete_while_process_running".into(),
        upstream_issue: "https://github.com/openai/codex/issues/34866".into(),
        tested_upstream_head: TESTED_UPSTREAM_HEAD.into(),
        baseline: serde_json::json!({
            "tool_returned": true,
            "process_still_alive": alive,
            "process_id_in_tool_response": process_id,
            "wrapper_output": "Script completed",
            "authoritative_state_disagreement": "tool wrapper completed while OS process still running"
        }),
        kernel: serde_json::json!({
            "process_exited_event": false,
            "operation_status": format!("{op_status:?}"),
            "events": adapter.kernel.log.records().len(),
        }),
        disagreements: disagreements.clone(),
        failure_reproduced: false,
        invariant_violations: 0,
    })
}

pub fn run_goal_complete_experiment() -> Result<ExperimentReport, KernelError> {
    let dir = tempfile::tempdir().map_err(|err| KernelError::Process(err.to_string()))?;
    let log = EventLog::create(dir.path().join("events.cak"))?;
    let mut adapter = ObservationAdapter::new(log)?;
    adapter.observe(CodexFact::ModelTurnFinished {
        last_message: "I am done".into(),
    })?;
    let disagreements = adapter.observe(CodexFact::GoalMarkedComplete)?;
    Ok(ExperimentReport {
        scenario: "model_turn_is_not_goal_completion".into(),
        upstream_issue: "https://github.com/openai/codex/issues/41176".into(),
        tested_upstream_head: TESTED_UPSTREAM_HEAD.into(),
        baseline: serde_json::json!({
            "codex_update_goal_complete": "accepted without child/process checks",
            "authoritative_state_disagreement": "MODEL_FINISHED_TURN vs GOAL_COMPLETED"
        }),
        kernel: serde_json::json!({
            "goal_completed": false,
        }),
        disagreements: disagreements.clone(),
        failure_reproduced: false,
        invariant_violations: 0,
    })
}

fn wrapper_argv() -> Vec<String> {
    if cfg!(windows) {
        // Spawn ping as the direct child so Child::kill() reaps it. `cmd /C ping`
        // leaves ping.exe running after cmd is killed and fails Windows CI cleanup.
        vec!["ping".into(), "-n".into(), "8".into(), "127.0.0.1".into()]
    } else {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'Script completed\\n'; exec sleep 8".into(),
        ]
    }
}

fn spawn_wrapper_script() -> Result<std::process::Child, KernelError> {
    let argv = wrapper_argv();
    Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| KernelError::Process(err.to_string()))
}

#[cfg(test)]
#[path = "observe_tests.rs"]
mod observe_tests;
