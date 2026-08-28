# Compatibility with Codex

The kernel does not depend on `codex-core`. Mappings are the contract an upstream engineer would implement as an adapter.

Quality: `Faithful` | `Approximate` | `ProjectionOnly` | `Unmapped` (see `compat.rs`; tests lock the table).

| Codex | Kernel | Quality | Imperfect because |
|---|---|---|---|
| Thread / `SessionId` | Execution | Approximate | Thread also means UI + live runtime |
| `ThreadGoal` | Goal | Approximate | Codex completes via model tool; kernel via acceptance |
| Subagent thread | Agent | Approximate | Codex status is event-derived and in-memory |
| v2 `wait_agent` | Join | Unmapped | Mailbox wake ≠ join |
| Unified exec / tool call | Operation + Attempt | Approximate | Many public IDs today |
| Rollout JSONL item | Event or event-derived | Approximate | Rollout is history, not leases/joins |
| `thread_history_*.sqlite` | Projection | Faithful | Same rebuild rule |
| `state_*.sqlite` metadata | Projection | ProjectionOnly | Can lag JSONL |
| `queue_*.sqlite` | Task READY | Approximate | User input vs work node |
| In-memory mailbox | `JoinChildTerminal` | Unmapped | Mailbox dies on restart |
| `InitialHistory::Forked` | Snapshot + delta | Approximate | Codex copies vectors |
| exec-server process | Lease + executor_id | Approximate | Transport session vs op id |
| `ApprovalAction` / Guardian | `CapabilityGranted` | Approximate | Layered caches vs one evidence object |
| `AgentStatus::Interrupted` | Interrupted (non-terminal) | Faithful | Same resumable meaning |
| App-server `CommandExecManager` | same Operation log | Unmapped | Second manager must not exist in a kernel world |

## Phase 2 observation adapter

`crates/observe` mirrors Codex-shaped facts. It does not write to `codex-core`.

| Codex source | event/function | current authority | kernel mapping | adapter | mismatch |
|---|---|---|---|---|---|
| `core/src/unified_exec/process_manager.rs` | yield return `Some(process_id)` while `!has_exited` | in-memory process manager + tool wrapper | `ProcessStarted` until `ProcessExited` | observation | wrapper can complete while kernel `Running` |
| `ext/goal/src/tool.rs` `handle_update` | `ThreadGoalStatus::Complete` | model + goals SQLite | `try_complete_goal` | observation | no child/process gate in Codex |
| `core/src/agent/status.rs` `agent_status_from_event` | `TurnComplete` → `Completed` | in-memory watch | `AgentStatusChanged` | not wired | mailbox vs join |
| `core/src/session/input_queue.rs` | v2 completion mail | in-memory deque | `JoinChildTerminal` | not wired | lost on restart |

Reproduction: `codex-kernel experiment wrapper-complete` / `goal-complete`.

1. On `exec_command`, allocate a kernel `OperationId` and treat model `session_id` as a **projection** of it.
2. On `ProcessState.has_exited`, emit `ProcessExited` then commit with the current lease.
3. On v2 child `TurnComplete`, emit `JoinChildTerminal` instead of (or in addition to) mailbox.
4. On `update_goal(complete)`, translate to `try_complete_goal` and **return an error to the model** if preconditions fail.
5. On spawn fork, put parent rollout bytes in CAS and attach `snapshot_id` to the child.

No Codex source files were modified. That is deliberate: contribution policy plus “resist adding to codex-core”.
