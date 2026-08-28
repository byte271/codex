# Invariants

These are enforced by `crates/kernel/src/reducer.rs` unless marked otherwise.

## Hard (reducer rejects the event)

1. **Monotonic seq.** Per execution, `seq` must be `last+1`. Duplicates of the same `event_id` are ignored (idempotent). A new `event_id` with a reused `idempotency_key` is `IdempotencyConflict`.
2. **Checksum.** Every record carries `blake3(canonical JSON with checksum="")`. Mismatch → `ChecksumMismatch`.
3. **Schema.** Only `schema_version = 1` is accepted.
4. **Committed operations cannot return to RUNNING.** `ProcessStarted` after `Succeeded`/`Failed`/`Cancelled` is illegal. **VERIFIED** by `committed_operation_cannot_return_to_running`.
5. **One active lease.** A new lease is allowed only from `Ready` or `Orphaned`, and generation must increase by exactly 1. First generation is 1.
6. **Stale workers cannot commit.** `OperationCommitted.lease_generation` must equal the unexpired lease generation. **VERIFIED** by `stale_lease_cannot_commit` (gen 1 expired, gen 2 issued, gen 1 commit rejected).
7. **Commit identifies the attempt.** The attempt must already have `ProcessExited`, and `exit_code` must match.
8. **ProcessExited is the completion fact for a process.** Tool wrappers, model turns, and UI traces are not events that can mark an operation succeeded.
9. **GOAL_COMPLETED is not MODEL_FINISHED_TURN.** Completion requires acceptance preconditions: required agents `Succeeded`, no non-terminal operations for the goal, required barriers satisfied. Empty required-agent list means **all** agents on that goal. **VERIFIED** by `model_turn_does_not_complete_goal`.
10. **Join outcome matches observations.** `JoinSatisfied` is rejected if children have not produced the claimed outcome. `join_all` + `WaitAll`: all children terminal; any `Failed` → `Failed`; else any `Cancelled` → `Cancelled`; else `Succeeded`. **VERIFIED** by `two_agent_join_all_waits_for_both` and `join_all_fails_if_child_fails`.
11. **Child capability cannot exceed parent.** Network `Allow` cannot be granted under `Deny`. Filesystem scope must be a prefix. Command allowlist must be a subset. **VERIFIED** by `capability_cannot_exceed_parent`.
12. **Terminal agent status is monotonic.** A `Succeeded` agent cannot become `Running`.
13. **Barrier satisfaction requires expected arrivals.**

## Log / projection (not the reducer, but tested)

14. **Incomplete (torn) tails are truncated.** Short length/CRC/payload at EOF is recovered. **A complete frame with CRC/JSON/checksum failure is fatal, including at EOF.** Huge or zero length prefixes are fatal before allocation. **VERIFIED** by `log_tests` and `torn_write_is_discarded_on_open`. See `docs/LOG.md`.
15. **SQLite is a projection.** Deleting `projection.sqlite` and rebuilding from the log yields the same `state_hash`. **VERIFIED** by `projection_rebuild_matches_reducer` (including after a corrupted JSON cell).
16. **Replay determinism.** `state_hash(reduce(prefix))` is stable across process restarts. **VERIFIED** by `replay_hash_is_deterministic`, `crash_restart_replays_same_hash`, and property tests.

## Scheduler / runtime

17. **UI state is not authoritative.** `codex-kernel view` and `replay` read reduced kernel state only.
18. **Lost processes after restart are not silent.** A `ProcessStarted` identity that is **dead** becomes `OperationFailed` with reason `lost-after-restart` and **no invented exit code**. A process that is still **alive** on Linux is polled until it dies, then failed the same way. When identity cannot be proven (macOS/Windows, or a pid-only key), the operation becomes `Unknown` via `OperationUnresolved`. Recovery never commits exit `-1`. **VERIFIED** by `lost_process_after_restart_is_not_silently_dropped` and `restart_surviving_process_reaches_terminal_state`.
19. **Waiting on a join does not consume an operation slot.** Join waiters are `WaitingJoin` agents, not `Running` operations.

## Explicitly not claimed

- Literal distributed exactly-once execution. Semantics are **at-least-once spawn + idempotent commit + lease fencing**.
- Reattaching a live OS process after parent death as a `std::process::Child`. Linux can prove identity with `pid:starttime` and poll liveness; it still cannot recover pipes or wait status. macOS/Windows preserve `Unknown` rather than guessing.
- 100,000 distinct randomized executions. Default property count is 1024 (override with `CAK_PROPTEST_CASES`). See FINDINGS.md.
