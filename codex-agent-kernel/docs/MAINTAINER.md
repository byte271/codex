# Maintainer report

Audience: an engineer who already maintains `openai/codex` and has 30 minutes.

**This overlay is not a merge candidate.** [docs/contributing.md](../../docs/contributing.md): OpenAI does not accept external pull requests. The usable artifact is issue-shaped RCA: [UPSTREAM.md](UPSTREAM.md).

Checked against `openai/codex` `main` **`41d3dc56a0`** (2026-08-28, #41239). Since the previous overlay tip (`4fea52346`, #41218): history-notes sanitization, PowerShell sandbox/version, Guardian review tests/token budgets, plugin cache/routing, `project/list` recency, model-provider auth recovery progress. Tried to disprove the lifecycle split. It still holds.

Review the overlay in [STAGES.md](STAGES.md) rather than as one 7k-line commit. Stage 1 is the reducer; Stage 3 is process recovery and allowlists; Stage 4 is the Codex observation adapter.

## Problem

Codex already has durable **session history** (rollout JSONL, SQLite as a rebuildable view). It does not have one durable lifecycle that jointly owns goals, subagents, processes, and joins.

Three open issues share that split. Full paste-ready notes are in [UPSTREAM.md](UPSTREAM.md).

| Issue | Symptom | What already knows the truth |
|---|---|---|
| [#34866](https://github.com/openai/codex/issues/34866) | Wrapper prints `Script completed` while the nested shell is still running | unified-exec `ProcessStatus::Alive` → `process_id = Some(...)` ([process_manager.rs#L647-L651](https://github.com/openai/codex/blob/41d3dc56a0e1de47e30a9585c1b49253c082f8f7/codex-rs/core/src/unified_exec/process_manager.rs#L647-L651)) |
| [#41176](https://github.com/openai/codex/issues/41176) | Agent declares completion while work remains | `update_goal(complete)` has no child/process gate ([tool.rs#L234](https://github.com/openai/codex/blob/41d3dc56a0e1de47e30a9585c1b49253c082f8f7/codex-rs/ext/goal/src/tool.rs#L234)) |
| [#41142](https://github.com/openai/codex/issues/41142) | Nested subagent completion does not reach the waiter | UI `list_agents` / in-memory mailbox vs spawn-graph edges |

Root pattern: **completion is inferred from a local view** (tool return, model turn, mailbox) rather than from the fact the manager already has.

## Evidence (this tree)

Observation-only adapter: `codex-agent-kernel/crates/observe`. It does not patch `codex-core`.

```bash
cd codex-agent-kernel
cargo test -p codex-kernel-observe
cargo run -p codex-kernel-cli -- experiment wrapper-complete
cargo run -p codex-kernel-cli -- experiment goal-complete
```

**Killer result:** a live OS child that prints `Script completed` and then sleeps is still running after a Codex-like yield return (`process_id = Some(session_id)`). The kernel operation stays `Running` with no `ProcessExited`. Machine-readable disagreement: `wrapper_complete_process_running`.

Goal path: kernel `try_complete_goal` after `MODEL_FINISHED_TURN` returns `GOAL_COMPLETED preconditions not met`.

## What not to send upstream

- This workspace
- Fork-only CI (skip Bazel/sdk on non-`openai/codex`, codespell `WRONLY`, stale `code-mode` features exception)
- A design for leases, time-travel fork, or a new entity model

Paste [UPSTREAM.md](UPSTREAM.md) onto the open issues if the goal is maintainer attention.

## Reproduction

```bash
cd codex-agent-kernel && cargo test --workspace
```
