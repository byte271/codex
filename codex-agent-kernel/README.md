# Codex Agent Kernel

A research overlay on [openai/codex](https://github.com/openai/codex): an event-sourced **durable execution kernel** for Codex-style autonomous agents.

This is not an upstream pull request. Codex [does not accept external code contributions](../docs/contributing.md). OpenAI will not review this workspace. The artifact that matches their contribution policy is issue-shaped RCA: [docs/UPSTREAM.md](docs/UPSTREAM.md).

## What this is

A small Rust workspace that implements one authoritative lifecycle for:

- executions, goals, agents, tasks
- operations / process attempts
- leases and stale-worker rejection
- joins (`join_all` / `join_any`)
- content-addressed context snapshots
- deterministic replay and time-travel fork
- rebuildable SQLite projections

The central distinction, enforced by the reducer:

**`MODEL_FINISHED_TURN` is not `GOAL_COMPLETED`.**

## What this is not

- A replacement for `codex-core`
- A claim that Codex is “unreliable”
- A 10,000-execution production runtime (see [FINDINGS.md](docs/FINDINGS.md) for actual numbers)

Codex has evolved rapidly. Several independent subsystems now carry overlapping notions of “running” and “done”. This kernel asks whether a single durable execution model can sit underneath those subsystems without requiring a rewrite on day one.

## Quick start

Requires Rust 1.95 (the same channel Codex pins in `codex-rs/rust-toolchain.toml`).

```bash
cd codex-agent-kernel
cargo test --workspace
cargo run -p codex-kernel-cli -- experiment wrapper-complete
cargo run -p codex-kernel-cli -- experiment goal-complete
cargo run -p codex-kernel-cli -- demo --dir /tmp/cak-demo
```

Maintainer-facing summary: [docs/MAINTAINER.md](docs/MAINTAINER.md). Log recovery: [docs/LOG.md](docs/LOG.md). CI: `.github/workflows/codex-agent-kernel.yml`.

Replay answers, from the log alone: why an agent existed, who spawned it, which operation ran, which lease generation committed, and why a goal was allowed to complete.

```text
codex-kernel replay --dir <execution-dir>
codex-kernel replay --dir <execution-dir> --until <event-id>
codex-kernel fork --dir <execution-dir> --from <event-id> --dest <new-dir>
```

## Layout

```
codex-agent-kernel/
  crates/kernel/        core library (events, log, reducer, runtime)
  crates/observe/       observation-only Codex adapter + experiments
  crates/kernel-cli/    codex-kernel binary
  docs/                 architecture, specs, evidence
  docs/formal/          TLA+ for lease fencing
```

The crate is intentionally **outside** `codex-rs/` so it does not touch Bazel/Cargo workspace CI.

## Evidence, not slogans

Read in this order:

1. [MAINTAINER.md](docs/MAINTAINER.md) — 30-minute report
2. [STAGES.md](docs/STAGES.md) — reviewable decomposition of the overlay
3. [CURRENT_ARCHITECTURE.md](docs/CURRENT_ARCHITECTURE.md) — live Codex as of the audit HEAD
4. [FINDINGS.md](docs/FINDINGS.md) — hypothesis verdict + measured results
5. [LOG.md](docs/LOG.md) — torn vs corrupt
6. [INVARIANTS.md](docs/INVARIANTS.md) — what the reducer rejects

Status labels used everywhere: **VERIFIED**, **LIKELY**, **HYPOTHESIS**, **FUTURE WORK**.
