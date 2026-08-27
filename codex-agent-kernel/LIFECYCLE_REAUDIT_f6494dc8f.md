# Codex durable execution lifecycle re-audit

- Audit date: 2026-08-27
- Repo: openai/codex (`codex-rs`) at `/workspace`
- Current HEAD: `f6494dc8f5969e8576a8a0945a674f2a15ac4de6` — Enable clock tools from model metadata (#41210)
- Prior audit points: `7f135e131` / `c4c51c56e`
- Method: fresh search of live sources only (no reliance on prior kernel docs)

## Verdict

**Hypothesis still holds, with nuance:** Codex has durable session history and several durable *partial* state machines (goals, spawn edges, queue, rollout/SQLite history, some approval/policy amendments). It does **not** have a single authoritative durable execution lifecycle that jointly owns goals, subagents, processes, and joins.

Closest host-owned gate is extension lifecycle contributors (`ThreadLifecycleContributor` / `TurnLifecycleContributor`) plus per-subsystem stores. That is an event bus / plugin surface, not a unified kernel.

---

## Current HEAD

```
f6494dc8f5969e8576a8a0945a674f2a15ac4de6
Enable clock tools from model metadata (#41210)
2026-08-27 21:46:20 +0000
```

## Commits since `7f135e131`

Only 4 commits on `main` after the prior audit SHA:

| SHA | Subject |
|-----|---------|
| `19321435b` | Propagate executor OS into turn environments (#41207) |
| `c4c51c56e` | Honor per-repository plugin configuration in catalog requests (#41208) |
| `34e74fda0` | Align deny-read matching with executor path semantics (#41209) |
| `f6494dc8f` | Enable clock tools from model metadata (#41210) |

None of these introduce a unified durable lifecycle.

### Lifecycle-adjacent commits after `7f135e131` (none) / recent ~2 weeks on main (selected)

Most important near prior audit:

- `4761851ff` Account subagent token usage toward root goals (#41183) — **accounting only**, no completion/join invariants
- `f1433fc71` Add developer instructions for persistent mode (#41050)
- `0cdb1f1c8` Harden goal continuation and remove duplicate prompt helpers (#40628)
- `d21794d6b` Reload Multi-Agent V2 children through their parent (#40477)
- `b705b6b07` Report completed sub-agent activity on parent turns (#40437)
- `eeb82a156` Dispatch queued messages written by other processes (#39034)
- `9341b3831` Add experimental thread queue APIs to app server (#38456)

---

## Topic findings (VERIFIED)

### 1. Goals (`ThreadGoalStatus`, `update_goal`, complete/blocked, child checks)

**VERIFIED:** Goals are durable in SQLite (`goals_1.sqlite`). Model tool `update_goal` may only set `complete` or `blocked`. There is **no** source-backed check that child agents/processes are idle before completion.

Statuses (state DB):

- File: `/workspace/codex-rs/state/src/model/thread_goal.rs`
- `ThreadGoalStatus::{Active,Paused,Blocked,UsageLimited,BudgetLimited,Complete}`
- Note: `is_terminal()` is only `BudgetLimited | Complete` (Blocked is *not* treated as terminal there)

`update_goal` handler (`GoalTools::handle_update`):

- File/function: `/workspace/codex-rs/ext/goal/src/tool.rs` :: `handle_update`
- Validates Complete|Blocked only, accounts progress, writes SQLite via `update_thread_goal`, emits `thread_goal_updated`
- No enumeration of children / processes / waiters

Since #41183 (`4761851ff`): descendant **token usage** rolls into root goal accounting (`GoalAccountingState::record_descendant_token_usage`). That is budget accounting, not a completion invariant.

Idle continuation: `GoalExtension::on_thread_idle` → `GoalRuntime::continue_if_idle` starts a turn when SQLite goal is `Active`.

### 2. Persistent mode / idle continuation

**VERIFIED:** Two different mechanisms:

1. **Persistent mode** = model-visible developer instructions fragment (`PersistentModeState`), not an execution kernel.
   - `/workspace/codex-rs/core/src/context/world_state/persistent_mode.rs`
2. **Idle continuation** = goal extension on host idle hook.
   - `/workspace/codex-rs/ext/goal/src/extension.rs` :: `on_thread_idle`
   - `/workspace/codex-rs/ext/goal/src/runtime.rs` :: `continue_if_idle`
   - Host emitter: `/workspace/codex-rs/core/src/tasks/lifecycle.rs` :: `emit_thread_idle_lifecycle_if_idle`

### 3. Multi-agent / subagent lifecycle

**VERIFIED:** Split across durable graph + in-memory live state.

| Concern | Durable? | Location |
|---------|----------|----------|
| Spawn parent/child edges Open/Closed | Yes (SQLite via AgentGraphStore) | `codex-rs/agent-graph-store/`, `state/src/runtime/threads.rs` |
| Live registry nicknames/slots | No (Mutex HashMap) | `AgentRegistry` in `core/src/agent/registry.rs` |
| `AgentStatus` | Live `watch::Sender`; partially reconstructed from rollout events on resume | `agent_status_from_event`, Session |
| Mailbox pending queue | In-memory `InputQueue` | `core/src/session/input_queue.rs` |
| Delivered inter-agent messages | Yes in rollout JSONL | `record_inter_agent_communication`, `RolloutItem::InterAgentCommunication` |
| `wait_agent` | Live poll/watch of status + mailbox activity | `multi_agents_v2/wait.rs` |
| Child completion notify | Event forwarding to parent on final status | `Session::maybe_forward_child_completion...` |

`agent_status_from_event` maps TurnStarted/Complete/Aborted/Error/ShutdownComplete → `AgentStatus`. It is a pure event→status mapper, not a durable store.

`restore_v2_agent_metadata` restores **identities/topology metadata** for open descendants without reopening runtimes.

### 4. App-server v2 thread/agent lifecycle APIs

**VERIFIED:** Lifecycle-related v2 surfaces exist but are API facades over the same split stores:

- Goals: `thread/goal/set|get|clear`, `ThreadGoalUpdatedNotification` — `app-server-protocol/src/protocol/v2/thread.rs`
- Resume/rollback/fork: `thread/resume`, deprecated `thread/rollback`
- Collab agent status on items: `CollabAgentStatus` in `v2/item.rs`
- Background terminals: `thread/backgroundTerminals/list|clean` (app-server + TUI)
- Queue: experimental thread queue APIs
- Turn: `turn/start`, steer, etc.

No v2 RPC exposes a unified “execution unit / join handle” spanning goal+agent+process.

### 5. Unified exec (process manager / PTY / IDs / yield)

**VERIFIED:**

- Manager: `UnifiedExecProcessManager` + in-memory `ProcessStore` (`HashMap<i32, ProcessEntry>`)
  - `/workspace/codex-rs/core/src/unified_exec/mod.rs`
  - `/workspace/codex-rs/core/src/unified_exec/process_manager.rs`
- Model-facing arg is **`session_id`** (trained name) mapped to internal **`process_id`**
  - `write_stdin.rs`: `process_id: args.session_id`
- **`call_id`**: tool-call id stored on `ProcessEntry` / `UnifiedExecContext` (approval/events/metrics), distinct from process_id
- Yield/resume: `exec_command` / `write_stdin` wait up to `yield_time_ms`; if process still alive, response returns `Some(process_id)`; if exited, `None`
- Wrapper complete vs process running: tool call can complete while process remains in `ProcessStore` as a background terminal (`store_process` before yield wait so interrupt does not drop last Arc)

Process table is **session-scoped memory**. New session constructs empty `UnifiedExecProcessManager::new(...)` — no rehydrate of live processes from rollout.

### 6. Code mode cells

**VERIFIED:** Separate host-generation-scoped cell IDs (`g{N}:...`), wait/yield/terminated outcomes in code-mode protocol. Not joined to goals/subagents/unified-exec process IDs.

- `/workspace/codex-rs/code-mode/src/remote_session/connection/driver/cell_ids.rs`

### 7. Background processes

**VERIFIED:** Same `ProcessStore` as unified exec. Listed via `list_processes` → `BackgroundTerminalInfo`. Ops: `CleanBackgroundTerminals`, app-server background terminal list/clean. Still in-memory; not durable across process restart of Codex.

### 8. Thread history: rollout JSONL vs SQLite

**VERIFIED:** Dual persistence:

- Rollout JSONL items filtered by `should_persist_event_msg` / `is_persisted_rollout_item`
  - `/workspace/codex-rs/rollout/src/policy.rs`
- SQLite: `state_5.sqlite`, `thread_history_1.sqlite`, `goals_1.sqlite`, `queue_1.sqlite`, etc.
  - `/workspace/codex-rs/state/src/sqlite.rs`

**NOT persisted** (selected from `should_persist_event_msg` false arm): `ExecCommandBegin/End`, `TerminalInteraction`, `ExecCommandOutputDelta`, approval request events, collab begin/end events, `ThreadQueueChanged`, MCP begin, streaming deltas, `RawResponseItem`, most realtime noise, etc.

Persisted: turn lifecycle markers (`TurnStarted/Complete/Aborted`), `ThreadGoalUpdated`, token counts, response items (most), inter-agent communications, compacted/world state/session meta, paginated TurnItems, etc.

### 9. Queue state

**VERIFIED:** User-message queue is durable SQLite (`queue_1.sqlite` / `QueuedItemService` + `QueueStore`). Separate from in-memory mailbox `InputQueue`.

- `/workspace/codex-rs/ext/queue/src/service.rs`
- `/workspace/codex-rs/state/src/runtime/queued_items.rs`
- Cross-process dispatch via SQLite data-version watch (~10s)

`ThreadQueueChanged` events are intentionally **not** rollout-persisted.

### 10. Remote executors and live process recovery

**VERIFIED:** Exec-server has **session reattach** (`resume_session_id`) with detached TTL (~30s) so a reconnect can keep processes alive for a short detach window.

- `/workspace/codex-rs/exec-server/src/server/session_registry.rs`
- Test: `exec_server_resumes_detached_session_without_killing_processes`

This is executor-session attachment recovery, **not** Codex-thread resume reattaching unified-exec process_ids into `ProcessStore`.

### 11. MCP connection restore

**VERIFIED:** On session construction from history, MCP runtime:

- restores resource-origin checkpoints from `RolloutItem::Compacted`
- observes events via `mcp_runtime.observe_event`
- starts with empty connection set; connections re-initialized after SessionConfigured (prewarm worker)

Not a full “restore live MCP sockets from durable kernel state”; reconnect/startup is runtime.

### 12. Approvals persistence

**VERIFIED:** Split:

- In-memory `ApprovalStore` (`HashMap` of serialized keys → `ReviewDecision`) for session-scoped tool approval cache
- Network policy amendments can persist to disk via `Session::persist_network_policy_amendment` (with atomic commit vs in-memory caches)
- Thread settings (`approval_policy`, `approvals_reviewer`) restored on `thread/resume` from persisted settings

Cached per-tool `ApprovedForSession` map itself is not a durable cross-process kernel.

### 13. Recovery: session resume / process reattach / sqlite recovery

**VERIFIED:**

- Session resume: reconstructs history via rollout / thread store; may set `AgentStatus::Interrupted` from last status-bearing event; restores V2 agent metadata from graph store; goal extension `restore_after_resume` re-marks active goals for idle continuation
- Process reattach: exec-server short TTL only; Codex `ProcessStore` empty on new Session
- SQLite recovery: corruption backup/recreate paths in `state/src/runtime/recovery.rs`

### 14. Context handling / compaction / ContextManager

**VERIFIED:** `ContextManager` owns in-memory model history envelopes, history_version, token_info, reference context, world_state baseline. Compaction rewrites history and can persist MCP origin checkpoints into compacted rollout items. This is context/history lifecycle, orthogonal to process/agent joins.

- `/workspace/codex-rs/core/src/context_manager/history.rs`

---

## Strongest lifecycle disagreements (source-backed)

1. **Goal Complete vs live children/processes**
   - `update_goal` can mark Complete with no child/process check (`ext/goal/src/tool.rs::handle_update`)
   - Meanwhile `AgentStatus`/`ProcessStore` may still show Running/Alive

2. **Durable spawn edges vs live AgentStatus**
   - Graph store persists Open/Closed edges
   - Status is `watch` channel derived from turn events; resume only special-cases Interrupted from rollout

3. **Tool wrapper completion vs process liveness**
   - `exec_command` returns while process may remain in `ProcessStore` (`process_id: Some`)
   - Rollout policy drops most exec begin/end/delta events → history does not retain live PTY state

4. **Mailbox durability split**
   - Pending mailbox: in-memory `InputQueue`
   - Delivered communications: rollout-persisted
   - User queue: separate SQLite store

5. **wait_agent is observation, not durable join**
   - Times out or returns current `AgentStatus` / mailbox activity
   - No durable join record / barrier object

6. **#41183 did not add completion invariants**
   - Only rolls descendant tokens into goal budgets

---

## Best MINIMAL observation-only adapter (1–2 hooks)

1. **Primary:** `/workspace/codex-rs/core/src/tasks/lifecycle.rs`
   - `Session::emit_thread_idle_lifecycle_if_idle`
   - `Session::emit_turn_start_lifecycle` / `emit_turn_stop_lifecycle` / `emit_turn_abort_lifecycle`
   - Already the host fan-out for extensions; observation can sit beside contributors without owning execution.

2. **Secondary (status + process boundaries):**
   - `/workspace/codex-rs/core/src/agent/status.rs` :: `agent_status_from_event` (and the Session sites that `send_replace` status / forward child completion)
   - `/workspace/codex-rs/core/src/unified_exec/process_manager.rs` :: `store_process` / `release_process_id` / `refresh_process_state`

These two paths cover turn/idle gates + agent finality + process alloc/release without inventing a new control plane.

---

## Existing duplicated state machinery a kernel could REPLACE (not add to)

| Duplicated concern | Live / partial owners today |
|--------------------|-----------------------------|
| Agent topology | `AgentGraphStore` (durable) + `AgentRegistry` (memory) + thread source metadata |
| Agent run status | `watch::Sender<AgentStatus>` + rollout event reconstruction + collab item snapshots |
| Process liveness | `ProcessStore` + exec-server `SessionRegistry` + background-terminal APIs |
| Goal progress | SQLite `thread_goals` + in-memory `GoalAccountingState` + turn extension stores |
| Pending work | SQLite user `QueueStore` + in-memory `InputQueue` mailbox + active turn pending_input |
| Approvals | in-memory `ApprovalStore` + persisted network/session policy amendments + thread settings |
| History | rollout JSONL policy + thread_history SQLite + `ContextManager` memory |

A kernel that *adds* another parallel store would worsen this. Replacement targets are the duplicated live/durable pairs above.

---

## Hypothesis status

**Still holds:** there is no single durable lifecycle authority for goals ∪ subagents ∪ processes ∪ joins.

**Nuance (do not overclaim):** Codex *does* have durable session history and durable subsystems (goals, spawn edges, queues, rollout/SQLite timeline, some policies). What is missing is a unified durable execution/join model across those subsystems; they remain loosely coupled via events, extension hooks, and in-memory managers.
