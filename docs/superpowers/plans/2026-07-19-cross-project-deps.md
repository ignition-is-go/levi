# Cross-project dependencies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `docs/superpowers/specs/2026-07-19-cross-project-deps-design.md`: upstream bug filing (`add --project`, foreign `comment`), cross-project dependencies with the sibling-checkout → hub-facts-cache → unknown resolution ladder, `via` annotations, and minted machine ids.

**Architecture:** levi-core stays pure — new Dependency/Claim fields plus a foreign-status hook in `rank_next`. The levi crate owns all state files (machine-id, checkout registry, foreign-status cache), the hub write-through, and the ladder. Hub unchanged. Dashboard gets a light dep rendering.

**Tech Stack:** unchanged (existing workspace).

## Global Constraints

- Conventional commits; branch `feat/elsewhere-closed` (user chose to extend PR #2).
- Entity fields must avoid `tx`/`createdAt` wire keys (CLAUDE.md gotcha).
- `ls`/`next` never touch the network; only sync legs do.
- All new fields `#[serde(default)]` for event back-compat.

### Task 1: levi-core — entity fields + foreign-aware ranking

Files: `levi-core/src/entities/{dependency,claim}.rs`, `levi-core/src/rank.rs`

- Dependency += `blocker_project_id: Option<String>`, `blocker_ref: Option<String>`, `via: Option<String>` (serde default). `dependency_id` for foreign blockers: `"{project_id}/{blocker}->{blocked}"` via new `foreign_dependency_id`.
- Claim += `machine_id: String` (serde default). `Identity` += `machine_id: String`. New `pub fn claim_is(claim: &Claim, me: &Identity) -> bool` in rank.rs: dev equal ∧ worktree equal ∧ (both machine_ids nonempty ⇒ ids equal; else display names equal). Replace inline mine-ness comparisons everywhere (rank eligibility, plus CLI call sites in Task 3).
- `rank_next` gains `foreign: &BTreeMap<String, Status>` keyed `"{project_id}/{task_id}"`; a dep with `blocker_project_id` is satisfied iff `foreign.get(key) == Some(Closed)` (missing ⇒ open ⇒ blocked). Same-project deps unchanged.
- Tests: foreign dep blocks until map says Closed; missing key blocks; claim_is legacy fallback matrix.

### Task 2: levi — machine id + checkout registry + foreign cache (state layer)

Files: `levi/src/state.rs` (new), `levi/src/ctx.rs`

- `state.rs`: `machine_id() -> String` (read/mint `~/.local/state/levi/machine-id`; `LEVI_STATE_DIR` env override for tests); `register_checkout(project_id, worktree)` (upsert path+last_used into `checkouts.toml`); `sibling_checkout(project_id) -> Option<PathBuf>` (verify exists, prune stale, most-recent first, skip self); `ForeignStatusCache` read/write of `.git/levi/foreign-status.toml` entries `{status, resolution, observed_at, title}` keyed `project_id/task_id`.
- `LeviCtx::load`: fill `identity.machine_id`, call `register_checkout` (best-effort) when a project exists.
- Tests: unit tests with temp state dir (mint idempotent; registry upsert/prune; cache round-trip).

### Task 3: levi — hub foreign ops + CLI surface

Files: `levi/src/hub_client.rs`, `levi/src/commands/{add,comment,dep}.rs`, `levi/src/cli.rs`, `levi/src/foreign.rs` (new: target parsing + ladder)

- `foreign.rs`: `parse_target("myko/lv-7c21[@ref]") -> ForeignTarget { project: NameOrId, task_prefix, refname }`; `resolve_project(session, name_or_id) -> (id, name)` (GetAllProjects + count marker; ambiguous name ⇒ error listing ids); `resolve_foreign_task(session, project_id, prefix) -> Task` (CountTasks(Partial{project_id}) + GetTasksByQuery, prefix match on ids, ambiguity errors).
- `hub_client.rs`: `push_events_verified(events: &[MEvent], project_id)` — wrap as LogEntries, send, verify via `GetLogEntrysByIds` count (reuse pattern), return event ids.
- `add --project NAME_OR_ID`: build foreign Task (uuid), push verified, print `name/lv-xxxx`. Claims/deps flags rejected with `--project` (v1 scope). `comment <proj>/<task> "text"`: same path. No hub configured ⇒ "cross-project operations need a hub".
- `dep add BLOCKED --on proj/task[@ref] [--via TEXT]`: foreign branch resolves project+task via hub (or accepts `<project-id>/<32-hex>` verbatim offline), stores Dependency with new fields; same-project path untouched. `dep rm` accepts the same form.
- CLI mine-ness call sites (`claim_ops`, `ls --mine`, `next`) switch to `claim_is`.

### Task 4: levi — resolution ladder + sync cache refresh + display

Files: `levi/src/foreign.rs`, `levi/src/sync.rs`, `levi/src/commands/{ls,show,next}.rs`, `levi/src/output.rs`

- `foreign.rs::resolve_ladder(world, cache) -> BTreeMap<String, (Status, Resolution, String /*asof*/, Option<String> /*title*/)>` for every distinct foreign blocker in world.deps: rung 1 sibling checkout (EventStore::discover at registry path, materialize, effective_status vs its HEAD via GixAncestors, read its task title) → rung 2 cache entry → rung 3 unknown/open/partial.
- `sync.rs` hub leg: after event exchange, for each foreign blocker fetch its StatusChanges (`CountStatusChanges(Partial{task_id}) + GetStatusChangesByQuery`), the foreign project's CommitFacts + RefFacts (ByQuery + count markers), resolve with `FactsAncestors` at `blocker_ref`/default branch (main, else most-recent RefFact), write cache with observed_at + title (`GetTasksByIds`).
- `ls/show/next`: statuses ladder map feeds `rank_next`; `show` renders `proj/lv-x — title [status, resolution, as of …] via: …`; `next` reason: blocked ⇒ `blocked by proj/lv-x (open on their main as of …); via: …`; just-unblocked (foreign Closed) ⇒ append `verify availability via: …` note when the gating dep has via.
- `onboard` instructions block: add the `--via` record/verify line (bump block content).

### Task 5: tests + dashboard + docs

**CI constraint: no real sibling checkouts exist.** The harness sets `LEVI_STATE_DIR` to a per-test temp dir for every invocation (alongside `LEVI_CONFIG`), so the developer's real registry is never read or written; sibling tests create both repos under temp and register the sibling by running levi inside it (which exercises auto-registration itself).

- Integration (`levi/tests/cross_project.rs`): two repos + in-process hub — file from A appears in B after sync; A's dep on B's task blocks through facts path until B closes + facts land; unblock reason carries via; ambiguous name error; no-hub error. Sibling path: registry env-pointed at B, no cache, exact resolution flips with B's checkout. Machine-id: same hostname+worktree, different `LEVI_STATE_DIR` ⇒ claims don't cross-own.
- Dashboard `browser.rs` drawer: render dep rows with `proj/short — via` when `blocker_project_id` set (status via hub entities + FactsAncestors, existing helpers).
- README: short "Cross-project" section. Close lv-3ab3 anchored; unblock note check on lv-ee52.
