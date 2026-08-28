# What OpenAI can actually use

**Grep `LIFECYCLE AUDIT` in this tree.** The markers sit on the live Codex paths so a Codex/Guardian audit of unified-exec, goals, or `wait_agent` cannot miss them.

`openai/codex` [does not accept external pull requests](https://github.com/openai/codex/blob/main/docs/contributing.md). A parallel kernel on a fork will not be reviewed. The contribution channel they document is **issue reports with reproduction and root-cause analysis**.

Checked against `openai/codex` `main` **`41d3dc56a0`** (2026-08-28, #41239). Commits since the previous overlay tip (`4fea52346`, #41218): #41219 remote registration retry, #41221 Guardian token budgets, #41223 `project/list` recency, #41226 Guardian test split, #41227 elevated PowerShell sandbox, #41230–#41232 plugins/PowerShell version, #41235 history-notes error sanitization, #41239 model-provider auth recovery progress. None add a durable joint lifecycle for goals ∪ subagents ∪ processes ∪ joins.

The three issues below are still open. Each section is meant to be pasted onto that issue. They do not propose this overlay.

---

## [#34866](https://github.com/openai/codex/issues/34866) — `Script completed` while the nested shell is still running

**Expected:** a tool return that prints `Script completed` means the OS child has exited, or the model still has a live `process_id` / `session_id` it can continue.

**Actual:** the wrapper/cell can report completion while unified-exec still owns a live process.

**Root cause (not the wrapper text alone):** `UnifiedExecProcessManager` already distinguishes alive vs exited. After `exec_command`, a still-running process yields with `process_id = Some(...)`:

https://github.com/openai/codex/blob/41d3dc56a0e1de47e30a9585c1b49253c082f8f7/codex-rs/core/src/unified_exec/process_manager.rs#L647-L651

```rust
let (response_process_id, exit_code) = if process_started_alive {
    match self.refresh_process_state(process_id).await {
        ProcessStatus::Alive {
            exit_code,
            process_id,
            ..
        } => (Some(process_id), exit_code),
```

That return is internally consistent: the process is not done. Completion is inferred from a *second* view (code-mode cell output, wrapper `Script completed`, model turn) that is not required to match `ProcessStatus`. Existing comments already cover discarded `session_id` in generated JS (#35613). The manager-side fact is the other half: **alive is already knowable; tool-complete does not consult it.**

**Reproduction (no Codex patch):** a child that prints `Script completed` and then sleeps is still running after the Codex-like yield predicate (`process_id = Some(session_id)`).

```bash
cd codex-agent-kernel
cargo run -p codex-kernel-cli -- experiment wrapper-complete
```

Disagreement code: `wrapper_complete_process_running`.

**Possible approach (for the Codex team, not a PR):** treat `ProcessStatus::Alive` / `process_id is Some` as not-complete in the wrapper and in code-mode cell completion. Do not print `Script completed` unless the status is `Exited`. Keep the existing `session_id` in the model-visible result when the process is alive.

---

## [#41176](https://github.com/openai/codex/issues/41176) — `update_goal(complete)` with no child/process gate

**Expected:** marking a goal complete fails while live children or processes remain.

**Actual:** `handle_update` accepts `Complete` or `Blocked` after argument parse and budget accounting. There is no query of live unified-exec processes or nested agents.

https://github.com/openai/codex/blob/41d3dc56a0e1de47e30a9585c1b49253c082f8f7/codex-rs/ext/goal/src/tool.rs#L234-L247

```rust
async fn handle_update(
    &self,
    invocation: ToolCall<'_>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let args: UpdateGoalArgs = parse_arguments(invocation.function_arguments()?)?;
    if !matches!(
        args.status,
        ThreadGoalStatus::Complete | ThreadGoalStatus::Blocked
    ) {
```

The rest of the function writes status through `update_thread_goal` with no process/child precondition.

**Reproduction:**

```bash
cd codex-agent-kernel
cargo run -p codex-kernel-cli -- experiment goal-complete
```

`MODEL_FINISHED_TURN` is not `GOAL_COMPLETED`. Disagreement code: `goal_complete_unfinished_work`.

**Possible approach:** before persisting `Complete`, refuse if the thread has a live process id or a non-terminal child. Same predicate as “do not print Script completed while Alive.”

---

## [#41142](https://github.com/openai/codex/issues/41142) — nested subagent completion vs mailbox

Same pattern, weaker unique evidence from this overlay: waiters observe `list_agents` / an in-memory mailbox, not a durable join over the child’s terminal status. A `join_all` that requires the child’s actual terminal agent status would close the split. This overlay does not claim a standalone repro beyond the mailbox vs spawn-graph reading already discussed on the issue.

---

## What this overlay is (optional, not the ask)

An isolated workspace under `codex-agent-kernel/` that records `ProcessExited` as the only process-completion fact and rejects `GOAL_COMPLETED` until preconditions hold. It does not patch `codex-core`. It is not a merge candidate.
