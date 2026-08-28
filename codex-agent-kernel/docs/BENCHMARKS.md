# Benchmarks

How to reproduce:

```bash
cd codex-agent-kernel
cargo test --workspace
cargo run -p codex-kernel-cli -- bench --dir /tmp/cak-bench
cargo run -p codex-kernel-cli -- demo --dir /tmp/cak-demo
```

Host: Linux x86_64 cloud agent VM, 2026-08-27. Rust 1.95.0.

## Reliability (deterministic tests)

`cargo test --workspace` on this revision:

| Suite | Tests | Result |
|---|---:|---|
| lib (`reducer_tests` + compat) | 8 | pass |
| `tests/lifecycle.rs` | 11 | pass |
| `tests/property.rs` | 2 | pass |
| CLI unit | 0 | n/a |

Scenarios covered: single agent, two-agent join, child failure, restart hash, torn write, corrupted projection rebuild, stale lease, lost pid, echo/true/huge output, capability escalation, CAS fan-out.

Not covered as live OS tests (see FINDINGS): PTY loss, process tree kill, Windows/macOS executors, hanging process wait without timeout helper.

## Randomized

`random_completion_order_never_double_commits_join`: 1024 cases, 2–8 children, shuffled completion, optional injected failure.

`duplicate_delivery_of_recorded_events_is_idempotent`: 1024 cases, 1–12 agents, replay of every recorded event.

Runtime ~21s for the property crate on this VM. Zero failures.

**Not achieved:** 10,000 executions. Do not read 1024 as 10,000.

## Storage

Synthetic parent 262,144 unique bytes, 4096-byte chunks, 1028-byte child tail.

| n | naive bytes | measured unique | unique / naive |
|---:|---:|---:|---:|
| 1 | 525,316 | 263,172 | 0.50 |
| 10 | 2,893,864 | 272,424 | 0.094 |
| 100 | 26,579,344 | 364,944 | 0.014 |

Demo execution (3 agents, 2 `/bin/echo` ops, join, goal complete):

| Artifact | Bytes |
|---|---:|
| `events.cak` | 23,484 |
| `projection.sqlite` | 40,960 |

## Efficiency (kernel demo, no model)

The demo does not call a model. Reported zeros are **VERIFIED zeros**, not estimates:

| Metric | Demo |
|---|---|
| Model turns | 0 |
| Tool turns (kernel ops) | 2 |
| Polling turns | 0 (scheduler drive) |
| Wall clock | < 1 s |

Codex baseline token amplification under 100-agent fork was **not measured** (would need instrumented `codex-core` + API). **FUTURE WORK.**

## Correctness counters (this slice, tests + demo)

| Counter | Observed |
|---|---|
| Lost authoritative tasks | 0 in tests |
| Duplicate committed operations | 0 in tests |
| Invalid terminal transitions accepted | 0 (reducer rejects; tests assert) |
| Replay divergence | 0 in tests |
| Unrecoverable projection after rebuild | 0 in tests |

These counters are **test-scoped**, not a production SLO.
