# Architecture

## Question

What runtime would Codex need if an agent execution had to remain correct across thousands of tool calls, hundreds of subagents, process crashes, app restarts, executor failures, compaction, retries, and multi-machine execution?

This overlay’s answer: **an event-sourced kernel with a pure reducer**, a crash-safe append log, and rebuildable projections. The model decides semantic work. The kernel decides mechanical lifecycle.

## Hierarchy (implemented)

```
Execution
 └── Goal
      ├── Agent (parent)
      │    ├── Operation (attempt, lease, process)
      │    └── Join ── wait on children
      ├── Agent (child A)
      │    └── Operation
      └── Agent (child B)
           └── Operation
```

Names follow upstream where they help (`Goal`, `Agent`) and introduce `Operation`/`Lease`/`Join` where Codex currently has overlapping IDs.

## Data flow

```
Command (Kernel API)
    → validate against KernelState
    → EventRecord (checksum, seq, idempotency_key)
    → EventLog append + fsync
    → reducer (already applied on a clone; state swapped on success)
    → optional SQLite projection
    → optional ContentStore chunks for process output / context
```

Workers never write the log. They hold a **lease generation**. Commit with a stale generation is rejected. That is the fencing token.

## Why not “just use rollout JSONL”?

Rollout is the right canonical store for **conversation history**. It is the wrong grain for:

- process identity across `session_id` / `cell_id` / `ProcessId`
- join of subagents (mailbox is in-memory)
- goal completion vs model turn
- lease fencing for remote executors
- copy-on-write context sharing

The compatibility adapter treats rollout items as *mappable* to events, not as a substitute for the kernel log. See COMPATIBILITY.md.

## Component map

| Module | Role |
|---|---|
| `event.rs` | Tagged event protocol, schema v1 |
| `log.rs` | `CAK1` length+CRC records, torn-tail recovery |
| `reducer.rs` | Pure `(state, event) -> state` |
| `kernel.rs` | Command API that emits events |
| `process.rs` | OS supervisor; reports are **not** completion |
| `runtime.rs` | Drive scheduler + apply `ProcessExited` then `OperationCommitted` |
| `scheduler.rs` | Concurrency limits + capability allowlists |
| `context.rs` | Hash-addressed chunks + COW snapshots |
| `projection.rs` | SQLite view |
| `replay.rs` | Prefix replay + fork |
| `viewer.rs` | Text DAG from durable state |

## Time-travel fork

`codex-kernel fork --from <event-id>` copies the prefix into a new execution id (checksums recomputed), then appends `ExecutionForked` ancestry. The source log is not mutated. Business-state hash of the rewritten prefix must equal the source prefix (**VERIFIED** `fork_preserves_business_hash`).

This is Git-like branching for agent debugging, not git objects.

## Distributed executors (protocol only)

A lease carries `executor_id`, `generation`, optional `capability_id`, and deadline. The environment this overlay ran in is a single Linux VM. **FUTURE WORK:** real Windows/macOS workers. **VERIFIED here:** capability isolation and stale-generation rejection, which are the trust-boundary primitives those workers would use.

## Relationship to Codex evolution

Codex is not “wrong”; it accumulated specialists (goals, v2 agents, unified exec, code mode, exec-server, paginated history) that now need a common lifecycle. The kernel is a candidate for that common layer, behind an adapter, not a fork of `codex-core`.
