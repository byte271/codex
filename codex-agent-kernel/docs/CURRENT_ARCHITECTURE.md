# Current Codex execution architecture

Audit date: 2026-08-28 (re-audit)

| Ref | SHA | When | Subject |
|---|---|---|---|
| Previous overlay tip | `4fea5234664ebc628b1a5322761cb132eaacc9e2` | 2026-08-27 | Share linked tool mention parsing in the TUI (#41218) |
| This overlay / `openai/codex` `main` | `41d3dc56a0e1de47e30a9585c1b49253c082f8f7` | 2026-08-28 | Surface model provider authentication recovery progress (#41239) |

Re-audit after #41219–#41239: history-notes sanitization, PowerShell sandbox/version, Guardian tests/token budgets, plugin cache/routing, `project/list` recency, model-provider auth recovery. **None add a durable joint lifecycle for goals ∪ subagents ∪ processes ∪ joins.** The hypothesis below still holds. Issue-shaped RCA: `docs/UPSTREAM.md`.

This document describes **live code**, not the kernel. Citations are paths under `/workspace/codex-rs` unless noted.

Contribution policy ([docs/contributing.md](../../docs/contributing.md)):

> **We do not accept external code contributions or pull requests.**

## Hypothesis under test

Codex contains multiple partially overlapping notions of execution state — thread, turn, goal, agent, process, tool-call, queue, rollout, history projection, remote-executor, approval, UI — without one authoritative durable execution model tying them together.

**Verdict: VERIFIED, with nuance.** Codex already has a clear canonical store for *session history* (rollout JSONL) and an explicit “SQLite is a rebuildable view” rule for paginated thread history. It does **not** have a single durable lifecycle that covers goals, subagents, processes, joins, and leases together. Those remain specialist subsystems with different recovery stories.

## 1. Canonical vs derived (session history)

**VERIFIED.** Rollout JSONL is canonical for session history.

`RolloutRecorder` is documented as writing canonical session rollout items (`codex-rs/rollout/src/recorder.rs`). Live writes flush JSONL first, then best-effort project to SQLite:

```335:346:codex-rs/thread-store/src/local/live_writer.rs
        // SQLite is a rebuildable view. The flush barrier must win before projection starts so it
        // can lag JSONL after failure, but can never get ahead of canonical history.
        durable_write(&recorder, write_op).await?;
        if let Err(err) = super::thread_history_materialization::materialize_to_sqlite(
```

Paginated lines carry ordinals (`codex-rs/history/src/lib.rs` `RolloutLine`). Incomplete trailing lines are left for the next materialize pass. SQLite corruption recovery moves one DB to `db-backups/` (`codex-rs/state/src/runtime/recovery.rs`).

**Not in the canonical log (VERIFIED):** many transient `EventMsg`s are filtered by `should_persist_event_msg` (`codex-rs/rollout/src/policy.rs`). Streaming deltas, some begin events, and live process handles are not session-resume state.

**Resume (VERIFIED):** reopen JSONL → `reconstruct_history_from_rollout` → `ContextManager`. Modern compaction uses `CompactedItem.replacement_history` as a checkpoint; legacy compaction without it is an approximate rebuild (`codex-rs/core/src/session/rollout_reconstruction.rs`).

**Runtime processes, MCP connections, in-memory mailboxes: not restored. VERIFIED.**

## 2. Goals

**Canonical store:** `thread_goals` in the state SQLite (`codex-rs/state/src/model/thread_goal.rs`).

```14:41:codex-rs/state/src/model/thread_goal.rs
pub enum ThreadGoalStatus {
    Active, Paused, Blocked, UsageLimited, BudgetLimited, Complete,
}
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::BudgetLimited | Self::Complete)
    }
```

**Completion authority: the model.** `update_goal` may set `complete` or `blocked`. Prompt text requires evidence (`codex-rs/ext/goal/src/spec.rs`), but **no code checks whether child agents or processes are still running**. Idle continuation (`GoalRuntime::continue_if_idle`) steers another turn while status is `Active`.

System-owned transitions: turn error → `Blocked`; usage limit → `UsageLimited`; token budget SQL → `BudgetLimited`; user API → pause/resume/clear.

**VERIFIED disagreement:** a goal may be `Complete` while descendants are `Running`. A goal may stay `Active` after every child `Completed`. `BudgetLimited` is terminal in `is_terminal()`, but the extension can keep the goal active and inject budget-limit steering.

Recent related work: subagent token usage rolls up toward root goals (`Account subagent token usage toward root goals`, #41183, 2026-08-27). Rollup is process-local until the root flushes.

Open issue class: [#41176](https://github.com/openai/codex/issues/41176) “Codex agents incorrectly stop or declare completion while tasks are still incomplete”.

## 3. Subagents

**Live status (VERIFIED)** is derived from turn events, not a durable agent FSM:

```6:30:codex-rs/core/src/agent/status.rs
pub(crate) fn agent_status_from_event(msg: &EventMsg) -> Option<AgentStatus> {
    match msg {
        EventMsg::TurnStarted(_) => Some(AgentStatus::Running),
        EventMsg::TurnComplete(ev) => Some(match &ev.error { ... Completed / Errored }),
        ...
pub(crate) fn is_final(status: &AgentStatus) -> bool {
    !matches!(status, PendingInit | Running | Interrupted)
}
```

`Interrupted` is **non-final**. v2 completion mail is not sent for non-final statuses.

**Spawn:** `AgentControl::spawn_agent_internal` may `InitialHistory::Forked(forked_rollout_items)` — a **copied, sanitized parent history** (`codex-rs/core/src/agent/control/spawn.rs` around the `fork_thread_with_source` call). Tests explicitly assert full-history forks copy parent context (`spawn_agent_can_fork_parent_thread_history_with_sanitized_items`).

**Durable membership:** spawn edges in `agent-graph-store` / `thread_spawn_edges`. **Live membership:** `AgentRegistry`. **Residency:** `V2Residency` LRU unload. These three can disagree after eviction.

**Wait / join:**

| API | Meaning | Authoritative join? |
|---|---|---|
| v1 `wait_agent` | Wait until named agents `is_final` | No; named poll |
| v2 `wait_agent` | Wait for **mailbox activity or steer** on this thread; does not return child content | No |

v2 tool description (`codex-rs/core/src/tools/handlers/multi_agents_spec.rs`):

> Wait for a mailbox update from any live agent… Does not return the content…

**Completion routing (v2): one hop to the immediate parent**, `trigger_turn: false`, into an **in-memory** mailbox (`codex-rs/core/src/session/input_queue.rs`). Restart loses pending completion mail. Grandparent learns only if the parent relays.

**VERIFIED: there is no authoritative JOIN primitive** tying goal completion to descendant terminality.

## 4. Process / tool execution

**VERIFIED: no single process handle.** Competing identities for one logical command:

| ID | Owner |
|---|---|
| Model `session_id` (i32) | Unified exec tool output (`process_id` renamed for the model) |
| Unified `process_id` | `UnifiedExecProcessManager` |
| Exec-server `ProcessId` | May be `{process_id}-{uuid}` on sandbox retry |
| Code-mode `cell_id` | JS cell wait loop |
| Code-mode host `session-{n}` | Host connection |
| Tool `call_id` / nested `exec-{uuid}` | Turn/tool dispatch |
| App-server `CommandExec` process id | **Separate** manager (`codex-rs/app-server/src/command_exec.rs`) |

Yield/resume: `exec_command` + `write_stdin` (unified exec) vs `exec` + `wait(cell_id)` (code mode). Nested `exec_command` inside a cell produces **two continuation loops** for one user-visible action.

Exit-code ownership is split:

1. OS / exec-server sets `ProcessState.exit_code`
2. Tool wrapper snapshots it after a yield
3. `spawn_exit_watcher` emits UI `ExecCommandEnd` independently

Approvals are layered, not one capability object: `ExecCommand`, `WriteStdin`, `Execve`, `McpToolCall`, `NetworkAccess`, `request_permissions`, plus remote executor-managed sandbox.

Recent related commits (same day as this audit): executor OS into turn environments (#41207), executor home into sandbox contexts (#41204), cached approvals (#41196).

## 5. Queue, persistent mode, Guardian

`ext/queue` is a durable **user-message** queue that competes with goals for `start_turn_if_idle` (queue is registered first). Mailbox pending mail can skip idle contributors.

Persistent mode (`ReasoningEffort::Persistent`) is a prompt fragment, orthogonal to goals, another “keep going” driver.

Guardian V2 is a review/approval path with its own persistence and parent-history access. It is not the execution kernel.

## 6. Context amplification on spawn

**VERIFIED.** Full-history fork copies sanitized parent `RolloutItem`s into `InitialHistory::Forked(Vec<RolloutItem>)`. Last-N windows drop items outside the window. Compacted replacement history is copied and re-sanitized. Images and tool payloads in the retained window are duplicated per child. There is no content-addressed structural sharing at spawn.

This is the baseline the kernel CAS benchmark compares against (synthetic; see BENCHMARKS.md). Measuring live `~/.codex/sessions` fan-out was **not** done in this environment (no user session corpus). **LIKELY** the same O(children × parent bytes) shape.

## 7. Ownership map (lifecycle transitions)

| Concern | Who owns truth today | Durable across process death? |
|---|---|---|
| Session transcript | Rollout JSONL | Yes |
| Paginated UI history | SQLite projection of JSONL | Rebuildable |
| Thread metadata / spawn edges | `state_*.sqlite` | Index; can lag |
| Goal status / budget | `goals` SQLite | Yes (not the rollout) |
| User queue | `queue_*.sqlite` | Yes |
| Agent live status | in-memory watch channel | No |
| Mailbox | in-memory deque | No |
| Unified exec processes | in-memory process manager | No (exec-server has a short detach TTL) |
| Code-mode cells | host/runtime | No |
| Approvals cache | session/turn memory + persisted profiles | Partial |
| UI “command finished” | event watcher | No |

## 8. Disagreement cases (found in source, not theorized)

1. Goal `Complete` vs child `Running`
2. Agent `Completed` vs goal `Active` vs unread mailbox
3. `Interrupted` non-final vs queue skipping idle vs no v2 parent mail
4. Live `list_agents` vs Open spawn edges vs residency eviction
5. Completion activity to initiator vs mail to parent
6. Model saw `exit_code` via `write_stdin` vs UI still waiting on `ExecCommandEnd`
7. Unified exec `session_id` vs exec-server `ProcessId` vs `cell_id`
8. SQLite history behind JSONL after a crash (by design) vs UI reading SQLite
9. App-server `CommandExec` vs agent unified exec for the “same” shell
10. Queue and goal both calling `start_turn_if_idle`

## 9. What Codex already got right

Do not duplicate these:

- JSONL-before-SQLite for thread history
- Writer lock per thread
- Compaction checkpoints (`replacement_history`) for model-context resume
- Paginated ordinals and incomplete-line handling
- Explicit persistence policy (`is_persisted_rollout_item`)
- Interrupted as resumable agent status
- Exec-server session resume TTL for detached transports

The kernel **copies the projection rule** and extends it to goals, agents, operations, and joins: the event log is canonical; SQLite is always rebuildable.

## 10. Recent upstream motion (context for this overlay)

Same-day work shows execution concerns spreading across crates rather than collapsing into one kernel: remote executor metadata (#41207, #41204, #40710), paginated history promotion (#40677, #40676, #40673), goal continuation hardening (#40628), Guardian compaction fail-closed (#41152), multi-agent v2 tool tracking (#40585). That pace is why a research overlay is more useful than a drive-by PR.
