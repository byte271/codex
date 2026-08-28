# Findings

## Upstream snapshot

- Overlay tested against `openai/codex` `main` `41d3dc56a0` (#41239).
- Since the previous overlay tip (`4fea52346`, #41218): #41219–#41239 (history-notes error sanitization, PowerShell sandbox/version, Guardian tests, plugins, `project/list` recency, model-provider auth recovery). None add a durable joint lifecycle.
- Contribution policy: no external PRs. Maintainer-facing RCA: [UPSTREAM.md](UPSTREAM.md).

## Hypothesis

> Codex has durable session-history semantics, but goals, child completion, process continuation, and several runtime control states do not share one authoritative durable lifecycle.

**Still VERIFIED after re-audit.** Session *history* already has canonical JSONL + rebuildable SQLite. Goals, subagents, processes, mailboxes, and approvals do not share that model. See CURRENT_ARCHITECTURE.md.

## Phase 2 (evidence, not more entities)

| Item | Status |
|---|---|
| Observation-only adapter | `crates/observe` — no `codex-core` patch |
| Real OS process + Codex-like yield | `wrapper_complete_process_running` vs [#34866](https://github.com/openai/codex/issues/34866) |
| Goal completion vs model turn | `goal_complete_unfinished_work` vs [#41176](https://github.com/openai/codex/issues/41176) |
| Event log: torn vs corrupt | complete CRC/JSON/checksum at EOF is fatal |
| Hard kill during append | `tests/fault_injection.rs` |
| Multi-process stale lease | setup / expire-commit / stale-commit helper processes |
| CI | `.github/workflows/codex-agent-kernel.yml` (Linux/macOS/Windows) |

What we still do not claim: live hook inside `UnifiedExecProcessManager`; 100k distinct state paths; Windows PID reattach.

## Kernel slice vs the success standard

| Milestone item | Status |
|---|---|
| Upstream architecture audit | VERIFIED (this doc set) |
| Event protocol + reducer | VERIFIED (executable) |
| Replay deterministic | VERIFIED (tests + CLI) |
| Operation lifecycle | VERIFIED (`/bin/echo`, `/bin/true`, 200kB `dd`) |
| Crash/restart | VERIFIED (hash equality; lost pid committed) |
| Corrupted projection rebuilt | VERIFIED |
| Stale lease cannot commit | VERIFIED |
| Two-agent join | VERIFIED |
| Randomized invariant tests | VERIFIED 1024+1024 cases, 0 fails |
| Benchmark baseline | VERIFIED synthetic CAS |
| Docs match executable | Intended; demo trace is the UX proof |

| Long-term target | Actual |
|---|---|
| 10,000+ randomized executions | 2,048 property cases + 19 deterministic tests |
| lost tasks / dup commits / invalid terminals / replay divergence / unrecoverable projection = 0 | 0 **in the tests we ran** |
| Significantly reduce duplicated context | 100-child unique storage 1.4% of naive copy (synthetic) |

## What we refused to fake

- “This solves Codex reliability.” It does not ship inside `codex-core`.
- Cross-OS executor demo. Protocol exists; workers were local Linux only.
- Live Codex session-byte amplification. No user rollout corpus in this VM.
- 10k soak numbers.

## Implementation differences vs the prompt’s dream hierarchy

- `Task` exists but the first slice’s runtime schedules **operations**, not a full DAG engine with arbitrary edges.
- `write_stdin` / PTY resume is not a second public ID; it is unimplemented (single-shot argv). That is stricter than Codex, not a wrapper soup.
- Distributed executor experiment is capability+lease, not four real operating systems.

Where the implementation is smaller than the prompt, the docs say so.

## Useful to upstream without a PR

1. Disagreement catalog (goal vs children vs mailbox vs process IDs).
2. Join semantics that `wait_agent` v2 does not provide.
3. `GOAL_COMPLETED` vs `MODEL_FINISHED_TURN` as a concrete reducer rule, relevant to issue #41176.
4. CAS numbers for spawn fan-out.
5. Lease fencing as the remote-executor commit rule.

## Next vertical slices (priority order)

1. Observation hook *inside* unified exec (still optional, default off) so yield-return cannot drift from the adapter predicate.
2. Durable mailbox = `JoinChildTerminal` persistence on real v2 child `TurnComplete`.
3. Soak: `CAK_PROPTEST_CASES=10000` via the kernel workflow `workflow_dispatch`.
