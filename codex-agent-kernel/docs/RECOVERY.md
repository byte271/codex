# Recovery

## What is canonical

The `events.cak` log. Everything else is a cache.

## Open path

1. Read header magic/schema
2. Read records until EOF
3. If the last record is **incomplete** (short length, CRC, or payload), `set_len` back to the last good boundary. A *complete* last record with CRC/JSON/checksum failure is a hard error (`LogCorrupt`), not a truncate.
4. `reduce_all`
5. Reconcile live processes:
   - Linux: `pid:starttime` identity. Matching processes are **polled** (not `Child`-reattached). Dead processes become `OperationFailed` with **no invented exit code**.
   - macOS / Windows: PID existence is not identity. A live PID becomes `OperationUnresolved` / `Unknown`. A missing PID becomes `OperationFailed` without exit `-1`. `/proc` is never consulted outside Linux.
6. Rebuild SQLite if requested / on `Runtime::project`

## Faults tested

| Fault | Result | Evidence |
|---|---|---|
| Process restart after committed work | Same `state_hash` | `crash_restart_replays_same_hash` |
| Incomplete JSON at tail | Record dropped, prior events load | `torn_write_is_discarded_on_open`, `log_tests` |
| Complete CRC mismatch at EOF | Fatal, not truncated | `complete_crc_mismatch_at_eof_is_fatal` |
| SIGKILL / TerminateProcess during append | Reopen succeeds or `LogCorrupt` | `hard_kill_during_append_leaves_openable_or_explicit_corruption` |
| Stale lease commit from another process | Rejected | `stale_worker_commit_is_rejected_across_processes` |
| SQLite payload corrupted | Delete DB, rebuild, hash matches | `projection_rebuild_matches_reducer` |
| Stale lease commit after restart-quality expire | `StaleLease` | `stale_lease_cannot_commit` |
| Pid from `ProcessStarted` gone on reopen | `OperationFailed` (`lost-after-restart`), no exit `-1` | `lost_process_after_restart_is_not_silently_dropped` |
| Live Linux process after restart | Polled until death, then failed without inventing `-1` | `restart_surviving_process_reaches_terminal_state` |
| Non-Linux live PID after restart | `Unknown`, not committed `-1` | `restart_surviving_process_reaches_terminal_state`, `non_linux_does_not_assume_proc_and_does_not_invent_death` |
| Duplicate event replay | No state change | `duplicate_event_id_is_idempotent`, property test |

## Faults not injected in this environment

| Fault | Status |
|---|---|
| Kill `app-server` / `codex` binary mid-turn | FUTURE WORK (would need a Codex integration harness) |
| Network partition to a remote exec-server | FUTURE WORK |
| Reordered delivery | N/A: single writer; reducer forbids non-monotonic seq |
| Mid-record bitrot not at EOF | Code path returns `LogIntegrity`; no randomized flip test yet |
| Live pid reattach (process still running after parent death) | Linux: identity via `pid:starttime` plus a poll watcher (no `Child` reattach, no invented exit). macOS/Windows: `Unknown` because PID existence is not identity. |

## Codex comparison

Codex resume restores **model history** from JSONL and catches up SQLite. It does not restore unified-exec processes or mailboxes. The kernel treats those as first-class events so restart has something true to reconcile.
