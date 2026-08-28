# Spec (kernel slice 1)

Normative behavior of the executable in this directory. If docs and code drift, code + tests win; this file should be updated in the same change.

## Identifiers

Typed UUID newtypes: `ExecutionId`, `EventId`, `GoalId`, `AgentId`, `TaskId`, `OperationId`, `AttemptId`, `JoinId`, `BarrierId`, `CapabilityId`, `SnapshotId`, `TurnId`, `LeaseOwnerId`, `ExecutorId`.

An operation keeps **one** `OperationId` from create through commit. Attempts are retries under that operation. Lease generations fence attempts.

## Commands vs events

Commands (`Kernel` methods) are not stored. They produce events. Failed commands do not append (reduce on a cloned state; append only on success).

## Process operations

Launch sequence:

1. `OperationCreated` + `OperationScheduled`
2. Scheduler selects READY ops under concurrency/capability limits
3. `OperationLeased` (generation N, owner, executor_id, expiry)
4. OS spawn → `ProcessStarted { pid, start_key }`
5. stdout/stderr → `ProcessOutputAvailable { stream, chunk_hash, offset, byte_len }` (bytes live in CAS). Output commands are keyed by stream, offset, length, and chunk hash so the two streams cannot collapse into one record.
6. `ProcessExited { exit_code, signal, killed_externally }`
7. `OperationCommitted { attempt_id, lease_generation, exit_code }`

If 7 uses the wrong generation, it is rejected and the log does not contain it.

## Crash

On `Runtime::open`:

- Replay the log (dropping a torn tail)
- Reconcile non-terminal ops that have a `start_key`:
  - **Alive** (Linux identity match): keep `Running` and start a poll watcher. When the process later disappears, append `OperationFailed` (`lost-after-restart`). Do **not** invent `ProcessExited` / exit `-1`.
  - **Dead**: `OperationFailed` (`lost-after-restart`), `committed_exit_code` stays unset.
  - **Unknown** (no identity, including every live PID on macOS/Windows): `OperationUnresolved` → `Unknown`. Do not treat `/proc` as existing outside Linux.

Silent loss of an authoritative operation is a bug. Inventing exit `-1` when the wait status was not observed is also a bug. **VERIFIED** for dead pids and for a surviving Linux process.

## Replay UX

`codex-kernel replay --dir DIR [--until EVENT]` prints tree + per-event trace + `state_hash`. Replaying the same prefix must match `prefix_hashes`. An unknown `--until` UUID returns `EventNotFound`; it must not silently replay the whole execution. **VERIFIED** by `unknown_replay_cutoff_returns_event_not_found`.

## Fork UX

`codex-kernel fork --dir DIR --from EVENT --dest NEWDIR` creates a new execution. Source is immutable.

## Out of scope for slice 1

PTY/write_stdin resume as a second public ID; code-mode cells; real multi-OS executors; compaction of the kernel log itself; 10k soak.
