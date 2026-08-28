# Event protocol

Schema version: **1**. Envelope fields are stable; new payload variants may be added with a version bump if they change reducer meaning.

## On-disk record

File header: magic `CAK1` (4) + `u16` schema LE + `u16` reserved.

Each record:

| Field | Size | Meaning |
|---|---|---|
| `len` | u32 LE | JSON payload bytes |
| `crc32` | u32 LE | CRC of payload (torn-write detection) |
| payload | `len` | `EventRecord` JSON |

`EventRecord.checksum` is **blake3** of the record serialized with `checksum=""` so the log can detect bitrot independent of CRC.

## Envelope

```
schema_version, execution_id, event_id, seq,
idempotency_key, causation_id, correlation_id, parent_event_id,
occurred_at_ms, checksum, payload
```

- `seq` is monotonic per execution, starting at 1.
- `event_id` is a UUID; duplicates are idempotent no-ops.
- `idempotency_key` is command-level. Examples: `op_leased:{op}:{generation}`, `proc_out:{op}:{attempt}:{stdout|stderr}:{offset}:{len}:{chunk_hash}`. Stream and content hash are part of the output key so equal-sized first chunks on stdout and stderr are recorded separately.
- `parent_event_id` is the previous log record (linear causal spine). Joins add semantic edges in the payload (`children`, `waiter_agent_id`).
- `causation_id` is used by forks (`from_event_id`).

## Payload catalog

| Event | Meaning |
|---|---|
| `ExecutionCreated` | Must be seq 1 |
| `ExecutionForked` | Ancestry; does not rewrite history |
| `GoalCreated` / `GoalAcceptanceDefined` | Objective + completion rules |
| `ModelTurnFinished` | **Not** goal completion |
| `GoalCompleted` / `GoalFailed` | Acceptance-checked |
| `AgentSpawned` / `AgentStatusChanged` | Agent lifecycle |
| `TaskCreated` | DAG node bound to an agent |
| `JoinCreated` / `JoinChildTerminal` / `JoinSatisfied` | Runtime join |
| `OperationCreated` / `Scheduled` / `Leased` | Mechanical work |
| `ProcessStarted` / `ProcessOutputAvailable` / `ProcessExited` | Process facts |
| `OperationCommitted` / `Failed` / `Cancelled` | Terminal op |
| `OperationUnresolved` | Identity could not be proven after restart; status `Unknown`; no exit code |
| `LeaseExpired` | Orphan; generation stays fenced |
| `CheckpointTaken` | Hash of reduced state at seq |
| `ContextSnapshotAttached` | CAS pointer, not inline bytes |
| `CapabilityGranted` | Durable approval evidence |
| `BarrierCreated` / `Arrived` / `Satisfied` | Named barrier |
| `RetentionChanged` | Keep agent addressable after terminal |

## Ordering model

Single writer per execution log. No permitted reordering. Duplicate *delivery* of the same record is ignored by `event_id`. Duplicate *commands* reuse `idempotency_key` and return the stored record without appending.

**FUTURE WORK:** multi-writer via per-execution consensus. Not implemented; do not pretend.

## Migration

Unknown payload tags fail serde on read (fail closed). Additive optional fields can be `#[serde(default)]` in a v1.1 reducer. Breaking changes bump `SCHEMA_VERSION` and require an offline rewrite tool (**FUTURE WORK**).
