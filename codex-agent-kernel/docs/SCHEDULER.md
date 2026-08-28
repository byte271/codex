# Scheduler

The model chooses *what* to do (spawn agent, specify argv). The scheduler chooses *whether it may run now*.

## Inputs

From reduced state:

- operations in `Ready` or `Orphaned`
- agents in `Runnable`
- active leases (`Leased`/`Running`/`WaitingExternal`) counting against `max_concurrent_operations` (default 8)
- running agents against `max_concurrent_agents` (default 16)
- capability command allowlists (`command_allowed`: exact path, or exact basename when the allow entry is a bare name — never `ends_with`)

## Outputs

`ScheduleDecision { runnable_agents, runnable_operations, blocked_operations }`

Blocked ops stay READY; they are not failed. No LLM polling is required to notice a free slot: `Runtime::drive` re-evaluates after each process report.

## What the scheduler must not do

- Mark a goal complete
- Invent argv
- Bypass capability allowlists (suffix matching is rejected; see `allowlist_matches_executable_boundary_only`)
- Issue a second unexpired lease (reducer would reject anyway)

## Timeouts and budgets

Lease `expires_at_ms` is recorded on `OperationLeased`. Expiry is an explicit `LeaseExpired` command (tests call it directly). **FUTURE WORK:** a clock thread that emits expiry events.

Token budgets are **not** implemented in the kernel. Codex goals already account tokens in SQLite; mapping that into kernel events is FUTURE WORK so we do not fork accounting logic badly.
