# Event log recovery

Canonical store: `events.cak`. Incomplete appends are recoverable. Completed-but-corrupt records are not.

## Layout

```
magic[4] = CAK1
schema[2] = u16 LE
reserved[2] = 0
records: { len u32 LE, crc32 u32 LE, payload[len] }*
```

`len` is the JSON payload size. `crc32` covers payload bytes only. `EventRecord.checksum` is blake3 of the record with `checksum=""`.

Maximum `len`: **16 MiB** (`MAX_RECORD_BYTES`). Larger or zero lengths are fatal *before allocation*.

## Open semantics

| On-disk situation | Result |
|---|---|
| EOF in length prefix (1–3 extra bytes) | Torn tail: truncate, load prior records |
| Length present, CRC truncated | Torn tail |
| Length+CRC present, payload shorter than `len` | Torn tail |
| Full frame, CRC mismatch (including last record) | **Fatal** `LogCorrupt(CrcMismatch)` |
| Full frame, JSON invalid | **Fatal** `LogCorrupt(InvalidJson)` |
| Full frame, blake3 mismatch | **Fatal** `LogCorrupt(ChecksumMismatch)` |
| `len == 0` or `len > MAX_RECORD_BYTES` | **Fatal** (do not treat as torn, do not allocate) |
| Valid records + incomplete garbage at EOF | Torn tail |
| CRC mismatch in the middle (not a short tail) | Fatal |

A complete record at EOF with a bad CRC is **not** a crash. Prefer "corruption detected" over silent discard.

## Tests

`crates/kernel/src/log_tests.rs` and `crates/kernel/tests/fault_injection.rs` (`hard_kill_during_append_leaves_openable_or_explicit_corruption`, `stale_worker_commit_is_rejected_across_processes`).
