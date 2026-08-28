//! Fault-injection helper process. Hidden from the public CLI.
//!
//! Subcommands:
//!   write-loop PATH
//!   setup-lease PATH
//!   expire-commit PATH
//!   stale-commit PATH

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use codex_agent_kernel::ids::{AttemptId, LeaseOwnerId, OperationId};
use codex_agent_kernel::log::EventLog;
use codex_agent_kernel::Kernel;
use serde_json::json;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        eprintln!("usage: cak-fault <write-loop|setup-lease|expire-commit|stale-commit> PATH");
        return ExitCode::from(2);
    };
    let Some(path) = args.next() else {
        eprintln!("missing path");
        return ExitCode::from(2);
    };
    match run(&cmd, Path::new(&path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run(cmd: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        "write-loop" => {
            let log = EventLog::open_or_create(path)?;
            let mut k = Kernel::from_log(log)?;
            if k.execution_id().is_err() {
                k.create_execution("fault")?;
            }
            loop {
                k.checkpoint()?;
            }
        }
        "setup-lease" => {
            let log = EventLog::create(path)?;
            let mut k = Kernel::from_log(log)?;
            k.create_execution("lease-race")?;
            let (goal, _) = k.create_goal("g", vec![])?;
            let (agent, _) = k.spawn_agent(None, goal, "w", None, None)?;
            k.start_agent(agent)?;
            let (op, attempt, _) =
                k.create_operation(agent, vec!["/bin/true".into()], "/", None)?;
            let gen = k.lease_operation(op, attempt, LeaseOwnerId::new(), "a", 1, None)?;
            k.process_started(op, attempt, 1, "1:0")?;
            let sidecar = path.with_extension("lease.json");
            fs::write(
                sidecar,
                json!({
                    "operation_id": op.to_string(),
                    "attempt_id": attempt.to_string(),
                    "generation": gen,
                })
                .to_string(),
            )?;
        }
        "expire-commit" => {
            let sidecar: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path.with_extension("lease.json"))?)?;
            let op: OperationId = sidecar["operation_id"].as_str().unwrap().parse()?;
            let attempt: AttemptId = sidecar["attempt_id"].as_str().unwrap().parse()?;
            let gen = sidecar["generation"].as_u64().unwrap();
            let log = EventLog::open(path)?;
            let mut k = Kernel::from_log(log)?;
            k.expire_lease(op, gen)?;
            let attempt2 = AttemptId::new();
            let gen2 = k.lease_operation(op, attempt2, LeaseOwnerId::new(), "b", 1000, None)?;
            k.process_started(op, attempt2, 2, "2:0")?;
            k.process_exited(op, attempt2, 0, None, false)?;
            k.commit_operation(op, attempt2, gen2, 0)?;
            fs::write(
                path.with_extension("lease.json"),
                json!({
                    "operation_id": op.to_string(),
                    "attempt_id": attempt.to_string(),
                    "generation": gen,
                    "winner_generation": gen2,
                })
                .to_string(),
            )?;
        }
        "stale-commit" => {
            let sidecar: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path.with_extension("lease.json"))?)?;
            let op: OperationId = sidecar["operation_id"].as_str().unwrap().parse()?;
            let attempt: AttemptId = sidecar["attempt_id"].as_str().unwrap().parse()?;
            let gen = sidecar["generation"].as_u64().unwrap();
            let log = EventLog::open(path)?;
            let mut k = Kernel::from_log(log)?;
            k.commit_operation(op, attempt, gen, 0)?;
        }
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}
