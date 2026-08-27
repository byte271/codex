# Ranked GitHub issues: lifecycle disagreement (openai/codex)

- Survey date: 2026-08-27
- Method: `gh` GraphQL (open issues by label) + targeted `gh issue view` / timeline; REST search is unreliable here (`is:open` search often returns 0 / PR noise)
- Scope: manifestations of **multiple sources of truth**, not one-off model mistakes
- Companion audits: `LIFECYCLE_REAUDIT_f6494dc8f.md`, research overlay `docs/CURRENT_ARCHITECTURE.md` / `docs/INVARIANTS.md`

## Ranking criteria

1. Clear multi-SoT disagreement (not only “model said done”)
2. Still **OPEN** with recent activity / independent reproductions
3. Reproducibility without live LLM when possible
4. Maintainer/community attention (cross-refs, repeated reproductions, mechanism docs)
5. Maps to a kernel invariant that would prevent the class

---

## Ranked candidates (strongest first)

### 1. [#34866](https://github.com/openai/codex/issues/34866) — `"Script completed" while nested shell still running`

| Field | Value |
|---|---|
| Status | **OPEN** |
| Labels | `bug`, `CLI`, `tool-calls` |
| Created / updated | 2026-07-23 / **2026-08-27** |
| Comments | 6 (+ reactions); fresh Windows Desktop re-repro 2026-08-27 |
| URL | https://github.com/openai/codex/issues/34866 |

**Why multi-SoT:** One user-visible command has at least two lifecycles: code-mode **cell/wrapper** (`Script completed`) vs unified-exec **process** (`session_id` still live). Related closed slice [#35613](https://github.com/openai/codex/issues/35613) (lost model-visible handle while manager still owns the process). Cross-refs: [#33816](https://github.com/openai/codex/issues/33816), [#35482](https://github.com/openai/codex/issues/35482), [#40041](https://github.com/openai/codex/issues/40041).

**Reproduce without network/LLM (best of this set):** Synthetic / unit-level with mocked model tool sequence:

1. Mock responses: `functions.exec` JS that calls nested `tools.exec_command` with `sleep 30` (or `dd`/`yes` redirect), then returns only `.output`.
2. Assert tool output contains “Script completed” / cell success while `UnifiedExecProcessManager` still has a live entry (or OS pid still running).
3. Optional: assert no model-visible `session_id` after wrapper return (lost-handle case from #35613).

Existing suite hooks: `codex-rs/core/tests/suite/unified_exec.rs`, code-mode wait specs. **Doable in this environment** with `test_codex` + `mount_sse*`.

**Kernel invariant that would prevent it:**

- **I8** ProcessExited is the completion fact for a process (wrappers/UI cannot mark Succeeded).
- Committed operations cannot return to RUNNING; one operation ID across wrapper + nested process.

---

### 2. [#38972](https://github.com/openai/codex/issues/38972) — Background turn `completed`/`interrupted` while JSONL still grows

| Field | Value |
|---|---|
| Status | **OPEN** |
| Labels | `bug`, `app`, `app-server` |
| Created / updated | 2026-08-17 / **2026-08-27** |
| Comments | 5; multiple independent Remote SSH reproductions with conflicting first-party APIs |
| URL | https://github.com/openai/codex/issues/38972 |

**Why multi-SoT:** `wait_threads(reason=turnCompleted)` vs `read_thread(status=interrupted, completedAt=null)` vs rollout JSONL still appending (no `final_answer`/`task_complete`). Later comments show four-way disagreement across wait/read/list/projection on Remote SSH. Sibling: [#40515](https://github.com/openai/codex/issues/40515) (Desktop shows interrupted; remote JSONL has `task_complete`).

**Reproduce without live LLM:** Partial — app-server integration with mocked model can drive turn start + synthetic event stream, then assert wait/read/JSONL agreement. Full Desktop/Remote SSH path **needs networked remote host / Desktop** (not available as product UI here). Core disagreement (wait wake vs turn terminality vs JSONL) is testable with `TestAppServer` + mocked SSE.

**Kernel invariant:**

- UI / wait projections are not authoritative (**I18**).
- Terminal turn/goal state requires reducing the canonical event log; wake only on terminal commit.

---

### 3. [#41060](https://github.com/openai/codex/issues/41060) — `codex queue` vs exec-thread turn state (message never delivered / silently destroyed)

| Field | Value |
|---|---|
| Status | **OPEN** |
| Labels | `bug`, `exec`, `CLI`, `session` |
| Created / updated | 2026-08-27 / 2026-08-27 |
| Comments | 1 |
| URL | https://github.com/openai/codex/issues/41060 |

**Why multi-SoT:** Durable `queue_*.sqlite` accepts the message; live `codex exec` turn pops then aborts (`turn_aborted` / interrupted) as the process exits; message sometimes never lands in rollout. Queue SoT vs thread/turn SoT vs process lifetime.

**Reproduce without LLM:** **Yes, CLI-level** (issue gives exact steps). Needs a live model OR a mocked long turn for full fidelity; race can be stressed with `sleep`-heavy mocked tool turns. Honest: author used a real model; harness with mocked slow turn should reproduce the pop-then-abort race.

**Kernel invariant:**

- Queue pop + turn start must be transactional against process/session exit.
- Competing idle drivers (queue vs goal) need one scheduler owner (`CURRENT_ARCHITECTURE` §5 / disagreement #10).

---

### 4. [#41142](https://github.com/openai/codex/issues/41142) — Nested subagent completion routing / retention (docs + observed runtime)

| Field | Value |
|---|---|
| Status | **OPEN** |
| Labels | `documentation`, `subagent` |
| Created / updated | 2026-08-27 / 2026-08-27 |
| Comments | 0 |
| URL | https://github.com/openai/codex/issues/41142 |

**Why multi-SoT:** UI/`list_agents` show grandchild `Completed` with final payload, but root `wait_agent` mailbox never receives one-hop-only completion mail. Second boundary: completed agent reclaimed when parent spawns a sibling (registry/residency vs graph edges). Matches source audit: v2 wait is mailbox activity, completion is one-hop in-memory mail, no JOIN.

Related open symptom issues: [#40299](https://github.com/openai/codex/issues/40299) (premature close vs busy-wait polling), [#39854](https://github.com/openai/codex/issues/39854) (wait_agent polling token burn), [#39469](https://github.com/openai/codex/issues/39469) (fork amplification — adjacent spawn SoT, not completion).

**Reproduce without LLM:** **Yes at unit/integration level** with mocked multi-agent tools: spawn root→parent→child, complete child, assert root mailbox empty while `list_agents` shows Completed; spawn sibling and assert retention eviction. Needs live model only for “model behavior” framing; contract bug is harness-testable.

**Kernel invariant:**

- **I10** Join outcome matches observations (`join_all` / WaitAll).
- Durable mailbox = JoinChildTerminal persistence (FINDINGS next-slice #2).
- Terminal agent status monotonic (**I12**); retention policy explicit.

---

### 5. [#41176](https://github.com/openai/codex/issues/41176) — Agents stop / declare completion while work incomplete

| Field | Value |
|---|---|
| Status | **OPEN** |
| Labels | `bug`, `model-behavior`, `CLI` |
| Created / updated | 2026-08-27 / 2026-08-27 |
| Comments | 1 (dedupe bot → #40938, #40139, #39948, #40646, #40560) |
| URL | https://github.com/openai/codex/issues/41176 |

**Why multi-SoT (partial):** Product symptom of **model-owned goal/task completion** with no host check that children/processes/acceptance gates are terminal. Source-backed: `update_goal(complete)` does not inspect descendants (`LIFECYCLE_REAUDIT` §1). Cluster mates are mostly model-behavior reports (#40938, #40139, #40646, #40560); #39948 is closed Terra-refusal.

**Reproduce without LLM:** **Cannot reproduce the full user story** without a live model + long engineering task. **Can** reproduce the *structural* hole: unit/integration test that marks a goal Complete while a child agent/process is Running and assert current code allows it; kernel test already encodes the opposite (`model_turn_does_not_complete_goal`).

**Kernel invariant:**

- **I9** `GOAL_COMPLETED` ≠ `MODEL_FINISHED_TURN` (required agents Succeeded, no non-terminal ops).

---

## Strong runners-up (same class, slightly weaker for this survey)

| # | Title | Why ranked lower |
|---|---|---|
| [#38495](https://github.com/openai/codex/issues/38495) | Code-mode `exec` → full-context `wait(cell_id)` polling after yield | Clear yield/resume SoT split; expensive; needs model for full burn, mechanism verified on main by commenter |
| [#33816](https://github.com/openai/codex/issues/33816) | Terra abandons yielded `exec_command`, starts duplicate | Model mishandling after yield; process still owned — pairs with #34866 |
| [#35482](https://github.com/openai/codex/issues/35482) | Sandboxed exec loses running child; deleted log fills disk | Lost process tracking after “done”; older update (2026-08-17) |
| [#38792](https://github.com/openai/codex/issues/38792) | Resume at first turn: desynced `thread_history` cursors | History projection vs JSONL SoT; reproducible from corrupted DB; adjacent to lifecycle |
| [#40014](https://github.com/openai/codex/issues/40014) | UI shows completed child; `read_thread` returns `items: []` | UI vs API projection; Windows Desktop |
| [#41140](https://github.com/openai/codex/issues/41140) | Blocking question marked completed + completion hooks | Turn-complete vs awaiting-user SoT |
| [#25606](https://github.com/openai/codex/issues/25606) | Mobile loses goal/task state; stop does not pause goal | Client vs goal SQLite; needs mobile/Desktop pair |
| [#41078](https://github.com/openai/codex/issues/41078) | Python SDK drops early `turn/completed` | Client router SoT; **reproducible with synthetic server** (author has pytest) |

---

## Recent upstream PRs / commits that touched lifecycle (not fixes for the above)

Same-day / recent motion spreads concerns rather than collapsing them:

| PR / commit | Note |
|---|---|
| [#41183](https://github.com/openai/codex/pull/41183) `4761851ff` Account subagent token usage toward root goals | Accounting only — no join/completion gate |
| [#41202](https://github.com/openai/codex/pull/41202) Extensions process MCP tool results | Tool lifecycle extension surface |
| [#40449](https://github.com/openai/codex/pull/40449) / [#40437](https://github.com/openai/codex/pull/40437) Peer/sub-agent completion activity on parent turns | Event routing, not durable join |
| [#40628](https://github.com/openai/codex/pull/40628) Harden goal continuation | Idle continuation, not child-terminal check |
| [#40024](https://github.com/openai/codex/pull/40024) Granular sandbox approvals in unified exec | Approvals layering |
| Older: [#3644](https://github.com/openai/codex/pull/3644) race, [#4992](https://github.com/openai/codex/pull/4992) lagged buffer, [#3288](https://github.com/openai/codex/pull/3288) Unified execution | Historical unified-exec work |

No open PR found that asserts “goal Complete iff descendants terminal” or “wrapper complete ≠ process exit.”

---

## Honesty: what this environment can / cannot reproduce

| Issue | In this VM without live model / Desktop |
|---|---|
| #34866 / #35613 class | **Yes** — mocked tool turn + process manager assertions |
| #41060 | **Likely** — mocked long `codex exec` turn + `codex queue` race |
| #41142 join/mailbox | **Yes** — multi-agent integration harness |
| #38972 / #40515 | **Partial** — app-server wait/read vs JSONL; full Desktop/SSH no |
| #41176 and model-behavior cluster | **Structural hole only**; full Terra/Sol story needs live model |
| #38495 token burn | Mechanism inspectable in source; burn needs live model |
| #35482 disk orphan | Needs sandboxed Desktop + runaway child |

---

## Suggested maintainer-attention order

1. **#34866** — sharpest executable lifecycle bug; fresh multi-platform re-repro; kernel I8
2. **#38972** (+ #40515) — first-party APIs disagree while JSONL is canonical; breaks orchestrators
3. **#41060** — queue vs exec exit race with silent data loss; CLI-repro steps
4. **#41142** — documents missing JOIN; aligns with source-verified one-hop mailbox
5. **#41176** — highest-level product symptom; use as narrative for I9, not as a unit repro
