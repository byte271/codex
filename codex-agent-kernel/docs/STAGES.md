# Staged review decomposition

The first overlay commit is larger than the repository's reviewability limit. Do **not** rewrite published history to hide that. Review and land follow-up work in these stages so each increment is independently checkable.

Dependencies flow downward: a later stage may use earlier crates/modules; an earlier stage must not need a later one.

## Stage 1 — IDs, events, state, reducer

Pure lifecycle model. No OS processes, no disk log, no scheduler.

**Files**

- `crates/kernel/src/ids.rs`
- `crates/kernel/src/error.rs`
- `crates/kernel/src/event.rs`
- `crates/kernel/src/state.rs`
- `crates/kernel/src/reducer.rs`
- `crates/kernel/src/kernel.rs` (command API that emits events)
- `crates/kernel/src/reducer_tests.rs`
- `crates/kernel/tests/property.rs`
- `docs/EVENTS.md`
- `docs/STATE_MACHINE.md`
- `docs/INVARIANTS.md` (reducer-enforced items)

**Depends on:** crate dependencies only (`serde`, `uuid`, `blake3`, …).

**Done when:** reducer tests and property tests pass; `JoinChildTerminal` cannot forge child status; process-output idempotency keys include stream + hash.

## Stage 2 — Durable log, replay, projection

Crash-safe persistence and time-travel. Still no live processes.

**Files**

- `crates/kernel/src/log.rs`
- `crates/kernel/src/log_tests.rs`
- `crates/kernel/src/replay.rs`
- `crates/kernel/src/projection.rs`
- `crates/kernel/src/viewer.rs`
- `crates/kernel/src/context.rs` (CAS used by projection/output hashes)
- `docs/LOG.md`

**Depends on:** Stage 1.

**Done when:** torn tails truncate, complete CRC/JSON/checksum failures are fatal, `replay --until <unknown>` returns `EventNotFound`, fork preserves business hash, projection rebuild matches `state_hash`.

## Stage 3 — Process runtime, scheduler, leases

OS supervisor, allowlists, restart reconciliation.

**Files**

- `crates/kernel/src/process.rs`
- `crates/kernel/src/process_tests.rs`
- `crates/kernel/src/scheduler.rs`
- `crates/kernel/src/scheduler_tests.rs`
- `crates/kernel/src/runtime.rs`
- `crates/kernel/tests/lifecycle.rs`
- `crates/kernel/tests/fault_injection.rs`
- `crates/kernel-cli/src/main.rs` (`init` / `replay` / `fork` / `view` / `demo`)
- `docs/RECOVERY.md`
- `docs/SCHEDULER.md`
- `docs/SECURITY.md`

**Depends on:** Stages 1–2.

**Done when:** stdout/stderr drain concurrently; spawn failure leaves the operation failed, not leased; restart of a live Linux process is polled to a terminal state without inventing exit `-1`; macOS/Windows preserve `Unknown` when identity cannot be proven; allowlist matches executable boundaries only.

## Stage 4 — Observation adapter / Codex experiments

Evidence against live Codex paths. Not kernel mechanism.

**Files**

- `crates/observe/src/lib.rs`
- `crates/observe/src/observe_tests.rs`
- `crates/kernel-cli` `experiment` subcommand
- `docs/CURRENT_ARCHITECTURE.md`
- `docs/FINDINGS.md`
- `docs/UPSTREAM.md`
- `docs/COMPATIBILITY.md`
- `docs/MAINTAINER.md`
- LIFECYCLE AUDIT comments in `codex-rs/` (observation only)

**Depends on:** Stages 1–3 for kernel comparisons; `codex-rs` sources are read, not rewritten for behavior.

**Done when:** wrapper-complete vs still-running process is a machine-readable disagreement (`#34866` class).

## Future commits

Put new kernel work on the matching stage. Do not mix Stage 4 Codex comments with Stage 1 reducer changes in the same commit unless a single bug fix requires both.
