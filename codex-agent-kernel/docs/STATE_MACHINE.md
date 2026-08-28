# State machine

## Operation

```
CREATED → READY → LEASED → RUNNING ⇄ WAITING_EXTERNAL
                         → COMMITTING → SUCCEEDED | FAILED
any non-terminal → CANCELLED
LEASED | RUNNING | WAITING_EXTERNAL → ORPHANED (lease expired)
ORPHANED → READY (new generation) | FAILED | CANCELLED
non-terminal + unproven process identity after restart → UNKNOWN (`OperationUnresolved`)
UNKNOWN is not scheduled. It does not carry an invented exit code.
```

`UNKNOWN` is emitted when recovery cannot prove process identity (macOS/Windows, or a pid-only `start_key`). Dead processes after restart become `FAILED` without `ProcessExited`.

Illegal examples (reducer):

- `SUCCEEDED → RUNNING`
- two unexpired leases
- `OperationCommitted` without `ProcessExited` on that attempt
- commit generation ≠ lease generation

## Agent

```
CREATED → RUNNABLE → RUNNING ⇄ WAITING_JOIN ⇄ WAITING_EXTERNAL
RUNNING → SUCCEEDED | FAILED | CANCELLED | INTERRUPTED | ORPHANED
INTERRUPTED → RUNNABLE | RUNNING | CANCELLED
```

`Interrupted` is non-terminal, matching Codex `is_final()`.

## Goal

```
ACTIVE → COMPLETED | FAILED | CANCELLED | BLOCKED
```

`COMPLETED` only via `GoalCompleted` after preconditions. A pile of `ModelTurnFinished` events never flips the flag.

## Join

Created with `kind ∈ {All, Any}` and `failure_policy ∈ {WaitAll, FailFast}`.

`join_all` implemented semantics (**VERIFIED**):

| Situation | WaitAll | FailFast |
|---|---|---|
| All children Succeeded | Join Succeeded; waiter Succeeded | same |
| Any Failed, others still running | wait | Join Failed as soon as failure is observed |
| Any Failed, all terminal | Join Failed | Join Failed |
| Any Cancelled, none Failed, all terminal | Join Cancelled | Join Cancelled |

Completed children remain addressable (`retained: true` by default). Waiting does not hold an operation lease.

`join_any`: first observed terminal child decides the outcome.

## Goal completion preconditions

Implemented in `goal_completion_preconditions_met`:

1. Goal not already terminal
2. Every required agent `Succeeded` (if the required list is empty, every agent on the goal)
3. No non-terminal operation whose agent belongs to the goal (except `Created`)
4. Every required barrier `satisfied`

There is no “model said so” clause.
