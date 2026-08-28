# Security

Security is a lifecycle property: **who may run what, with which evidence, under which lease.**

## Trust boundaries

| Principal | May |
|---|---|
| Model | Propose semantic work (spawn, argv, “I think we’re done”) |
| Kernel reducer | Accept or reject lifecycle events |
| Executor / OS | Run a leased argv inside a capability |
| UI / TUI | Display projections; never complete an operation |
| Remote executor | Hold a lease; cannot define completion except by reporting `ProcessExited` which still needs a matching generation to commit |

## Capabilities

`CapabilityGranted` records:

- filesystem_scope (prefix)
- network `Deny | Allow`
- command_allowlist (exact argv0, or exact basename for a bare allow entry; `/tmp/malicious-echo` is not authorized by `echo`)
- deadline
- approval_evidence (string; in production this would be a signed or user-attributed decision id)
- parent_capability_id

A child capability that is not a subset of its parent is rejected at `AgentSpawned`. Granting a wider cap does **not** silently attach to the parent agent (no auto-escalation).

**VERIFIED:** network Allow under Deny rejected.

**FUTURE WORK:** parse and compare real sandbox policies / Guardian scores; wire `approval_evidence` to Codex `ApprovalAction`.

## Inheritance

Subagents do not gain authority by default. If the parent has no capability, a child may be spawned without one (local prototype). Production adapter should require an explicit grant at spawn, mapping Codex sandbox policy + approval cache into a capability.

## Remote executors

A remote worker receives `{operation_id, generation, capability, deadline}`. Stale generation commits are rejected even if the remote process exited 0. That is the fence against partitioned workers.

This environment did not run a second machine. The fence is tested locally with two `LeaseOwnerId`s.

## Model vs runtime authority

The model can emit `ModelTurnFinished`. It cannot emit a successful `GoalCompleted` unless the reducer preconditions hold. That is the whole point of the GOAL vs TURN split.
