use clap::{Parser, Subcommand};
use std::path::PathBuf;

use codex_agent_kernel::context::{
    naive_child_storage_bytes, shared_child_storage_bytes, ContentStore,
};
use codex_agent_kernel::ids::EventId;
use codex_agent_kernel::replay::{fork_from, replay_until};
use codex_agent_kernel::runtime::Runtime;
use codex_agent_kernel::viewer::render_tree;
use std::str::FromStr;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "codex-kernel",
    about = "Codex Agent Kernel — durable execution research overlay"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a kernel directory and empty execution.
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "kernel-execution")]
        note: String,
    },
    /// Replay an execution log and print why the state is what it is.
    Replay {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        until: Option<String>,
    },
    /// Fork a new execution from a historical event without mutating the source.
    Fork {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        from: String,
        #[arg(long)]
        dest: PathBuf,
    },
    /// Print a textual DAG from durable kernel state.
    View {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Run the vertical-slice demo: one agent, one process, join of two agents.
    Demo {
        #[arg(long, default_value = "/tmp/codex-agent-kernel-demo")]
        dir: PathBuf,
    },
    /// Storage amplification benchmark (naive copy vs shared snapshot).
    Bench {
        #[arg(long, default_value = "/tmp/codex-agent-kernel-bench")]
        dir: PathBuf,
    },
    /// Observation-only baseline vs kernel experiment (machine-readable JSON).
    Experiment {
        #[arg(value_enum)]
        kind: ExperimentKind,
    },
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum ExperimentKind {
    WrapperComplete,
    GoalComplete,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { dir, note } => {
            let rt = Runtime::create(&dir, &note)?;
            println!("execution {}", rt.execution_id()?);
            println!("log {}", rt.root().join("events.cak").display());
        }
        Commands::Replay { dir, until } => {
            let until = until.map(|s| EventId::from_str(&s)).transpose()?;
            let report = replay_until(dir.join("events.cak"), until)?;
            println!("execution {}", report.execution_id);
            println!("events {}", report.events);
            println!("state_hash {}", report.state_hash);
            println!("business_hash {}", report.business_hash);
            println!();
            println!("{}", report.tree);
            println!("--- trace ---");
            print!("{}", report.trace);
            if let Some((event_id, hash)) = report.prefix_hashes.last() {
                println!("prefix_end {event_id} {hash}");
            }
        }
        Commands::Fork { dir, from, dest } => {
            let from = EventId::from_str(&from)?;
            let (id, src, fork) = fork_from(dir.join("events.cak"), dest, from)?;
            println!("forked_execution {id}");
            println!("source_business_hash {src}");
            println!("fork_business_hash {fork}");
        }
        Commands::View { dir } => {
            let rt = Runtime::open(&dir)?;
            println!("{}", render_tree(rt.kernel.execution()?));
        }
        Commands::Demo { dir } => {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let mut rt = Runtime::create(&dir, "demo")?;
            let (goal, _) = rt.kernel.create_goal("vertical slice", vec![])?;
            let (parent, _) = rt.kernel.spawn_agent(None, goal, "parent", None, None)?;
            let (a, _) = rt
                .kernel
                .spawn_agent(Some(parent), goal, "echo-a", None, None)?;
            let (b, _) = rt
                .kernel
                .spawn_agent(Some(parent), goal, "echo-b", None, None)?;
            rt.kernel.define_acceptance(goal, vec![])?;
            rt.kernel.start_agent(parent)?;
            let argv_a = vec!["/bin/echo".into(), "A".into()];
            let argv_b = vec!["/bin/echo".into(), "B".into()];
            rt.kernel
                .create_operation(a, argv_a, dir.to_string_lossy().into_owned(), None)?;
            rt.kernel
                .create_operation(b, argv_b, dir.to_string_lossy().into_owned(), None)?;
            rt.wait_idle(Duration::from_secs(8))?;
            rt.kernel.start_agent(a)?;
            rt.kernel.start_agent(b)?;
            rt.kernel.complete_agent(a)?;
            rt.kernel.complete_agent(b)?;
            let (join, _) = rt.kernel.create_join(
                parent,
                vec![a, b],
                codex_agent_kernel::JoinKind::All,
                codex_agent_kernel::JoinFailurePolicy::WaitAll,
            )?;
            rt.kernel.observe_join_child(join, a)?;
            rt.kernel.observe_join_child(join, b)?;
            match rt.kernel.try_complete_goal(goal) {
                Ok(_) => println!("GOAL_COMPLETED"),
                Err(err) => println!("goal not complete: {err}"),
            }
            let (sh, bh) = rt.kernel.hashes()?;
            println!("execution {}", rt.execution_id()?);
            println!("state_hash {sh}");
            println!("business_hash {bh}");
            println!("{}", render_tree(rt.kernel.execution()?));
            println!("replay with: codex-kernel replay --dir {}", dir.display());
        }
        Commands::Bench { dir } => {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let mut cas = ContentStore::open(&dir)?;
            let parent: Vec<u8> = (0..(256 * 1024)).map(|i| (i % 251) as u8).collect();
            let parent_snap = cas.snapshot(None, &parent, 4096)?;
            println!(
                "scenario,children,naive_bytes,shared_bytes,measured_unique_bytes,amplification"
            );
            for n in [1u64, 10, 100] {
                let mut measured_store = ContentStore::open(dir.join(format!("n{n}")))?;
                let p = measured_store.snapshot(None, &parent, 4096)?;
                for i in 0..n {
                    let mut child = parent.clone();
                    child.extend_from_slice(&(i as u32).to_le_bytes());
                    child.extend_from_slice(&[7u8; 1024]);
                    let _ = measured_store.snapshot_delta(&p, &child, 4096)?;
                }
                let unique = measured_store.unique_bytes()?;
                let naive = naive_child_storage_bytes(parent.len() as u64, 1028, n);
                let shared = shared_child_storage_bytes(parent.len() as u64, 1028, n);
                let amp = unique as f64 / parent.len() as f64;
                println!("{n}-children,{n},{naive},{shared},{unique},{amp:.3}");
            }
            let _ = parent_snap;
            println!("deep-tree and image-heavy rows use the same CAS; see BENCHMARKS.md");
        }
        Commands::Experiment { kind } => {
            let report = match kind {
                ExperimentKind::WrapperComplete => {
                    codex_kernel_observe::run_wrapper_complete_experiment()?
                }
                ExperimentKind::GoalComplete => {
                    codex_kernel_observe::run_goal_complete_experiment()?
                }
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
