# levi v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the full levi v1 design (`docs/superpowers/specs/2026-07-18-levi-design.md`): a git-aware, agent-first, distributed issue tracker — CLI, hub, and Leptos dashboard — on the myko framework.

**Architecture:** Cargo workspace with four crates. `levi-core` holds myko entities plus pure functions (event materialization, per-checkout status resolution, ranking). `levi` (CLI) stores content-addressed CBOR event blobs under `refs/levi/events` via gix with CAS append, and materializes state per invocation. `levi-hub` wraps a myko `CellServer` (optional Postgres) behind a token-checking axum front door that also serves the dashboard. `levi-dash` is a Leptos 0.8 CSR app using `myko-leptos` live queries.

**Tech Stack:** Rust edition 2024, myko 4.24 (path deps to `../myko`), gix 0.85, clap 4 (derive), ciborium 0.2, chrono, uuid, tokio 1, axum 0.7 + tokio-tungstenite (proxy), leptos 0.8 (csr) + trunk, assert_cmd + tempfile for tests.

## Global Constraints

- Never include AI/Claude attribution or references in commit messages or PRs (CLAUDE.md).
- Spec is authoritative: `docs/superpowers/specs/2026-07-18-levi-design.md`. Deviations recorded in the "Spec deviations" section below must be preserved in code comments where they apply.
- Every read command supports `--json` with stable, versioned schemas (`"schema": "levi.<cmd>/1"`).
- Status is never a field on Task; it is always derived per-checkout from StatusChange records.
- Event log is append-only; events are immutable; union merge must always be conflict-free.
- myko deps are **path deps** to the local checkout: `../myko/libs/myko/core` (`myko`), `../myko/libs/myko/server` (`myko-server`), `../myko/libs/myko/leptos` (`myko-leptos`). Version `4.24`. Edition 2024 everywhere.
- Work happens on branch `levi-impl`; commit after every green task.

## Spec deviations (agreed rationale, encode as comments where relevant)

1. **Content addressing** — myko events are not content-addressed (Postgres BIGSERIAL). levi's event id = the git blob OID (SHA-1 hex) of the event's canonical CBOR bytes. Git *is* the content-address space; tree sharding = OID prefix sharding.
2. **LWW order** — myko applies events in arrival order. levi guarantees the spec's `(created_at, event id)` LWW by sorting all events with that key before replay in `materialize()`. Deterministic on every node.
3. **CLI "embeds a myko node"** — `CellServerCtx` requires nine wired subsystems; a one-shot CLI instead uses `levi_core::materialize()` (pure fold with identical replay semantics). The hub embeds the real `CellServer`.
4. **Hub event exchange** — levi events travel to/from the hub wrapped as immutable `LogEntry` entities (`id` = event OID, payload = base64 CBOR). Set difference over `LogEntry` ids implements "what are you missing". A hub-side saga unwraps each LogEntry and applies the inner event so dashboards query real entities.
5. **Hub auth + static serving** — dropped after review: the hub is a plain `CellServer` on the public bind with no auth (myko has no auth hooks; put a proxy in front if needed), and the dashboard is a standalone CSR app (`trunk serve` in dev) connecting straight to `/myko`. The spec's bearer token is deferred beyond v1.
6. **CommitFact/RefFact scope** — facts are published to the hub only (sync leg 3), not appended to the git ref (nodes with the repo don't need them; keeps the ref lean). They remain ordinary myko entities so they replicate hub-side like everything else.
7. **Git transport** — fetch/push of `refs/levi/*` shells out to the `git` binary (gix push support is immature). Everything else uses gix in-process.
8. **`levi watch`** — requires a configured hub (it is the live-subscription exception). Without a hub it exits with guidance.
9. **Config location** — repo-level config moved from `git config levi.*` to a committed `.levi/config.toml` (written by `levi onboard --hub`), so a clone is fully configured; user-global `~/.config/levi/config.toml` remains the fallback.
10. **`levi onboard` merged into `levi init`** — `levi onboard` is now a hidden alias; `levi init` adopts an existing remote project instead of minting a fork, and `next`/`ls` auto-fetch the events ref on a fresh clone (spec: 2026-07-20-next-sync-recovery-design.md).

## File Structure

```
Cargo.toml                      # workspace: levi-core, levi, levi-hub, levi-dash
levi-core/
  src/lib.rs                    # pub mod entities, materialize, resolve, rank, ids
  src/entities/mod.rs           # one file per entity, glob re-export
  src/entities/{project,task,status_change,dependency,claim,comment,commit_fact,ref_fact,log_entry}.rs
  src/ids.rs                    # short-id display + prefix matching
  src/materialize.rs            # EventRecord, World, materialize()
  src/resolve.rs                # AncestorSet, Ancestry, ResolvedStatus, effective_status(), FactsAncestors
  src/rank.rs                   # eligibility, ordering, reason strings
levi/
  src/main.rs                   # clap dispatch only
  src/cli.rs                    # clap derive types (full surface)
  src/ctx.rs                    # LeviCtx: repo, identity, config, world loading
  src/store.rs                  # gix event store: read_events, append_events (CAS), merge_heads
  src/ancestors.rs              # GixAncestors (exact mode), orphan-anchor detection
  src/config.rs                 # levi.hub git config + ~/.config/levi/config.toml
  src/output.rs                 # human + --json rendering, schema constants
  src/commands/{init,add,ls,show,next,claim_ops,close,dep,comment,edit,sync,watch}.rs
  src/hub_client.rs             # MykoClient + TokenTransport, one-shot query/report helpers
  src/facts.rs                  # facts leg: ancestor walk, depth cap, published-cache
  tests/common/mod.rs           # temp-repo harness
  tests/{cli_basic,cli_git_state,cli_concurrency,cli_next,convergence}.rs
levi-hub/
  src/main.rs                   # clap `serve`; CellServer (internal) + axum front door
  src/front_door.rs             # token check, static files, WS byte-pipe proxy
  src/unwrap_saga.rs            # LogEntry -> apply inner MEvent
levi-dash/
  Cargo.toml, Trunk.toml, index.html
  src/main.rs                   # mount App
  src/app.rs                    # router, provide_myko, token cookie handling
  src/pages/{overview,in_flight,browser}.rs
  src/resolve_client.rs         # FactsAncestors reuse for branch-selector resolution
```

---

### Task 1: Workspace scaffold + levi-core entities

**Files:**
- Create: `Cargo.toml`, `levi-core/Cargo.toml`, `levi-core/src/lib.rs`, `levi-core/src/entities/*.rs` (9 files + mod.rs), `.gitignore`
- Test: entity round-trip test in `levi-core/src/entities/mod.rs`

**Interfaces (produces):** all entity types + generated `GetAll*`/`*Id`/`Partial*`; `levi_core::link()`; `Priority`, `StatusKind` subtypes.

- [ ] **Step 1: Branch + workspace files**

```bash
git checkout -b levi-impl
```

Root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["levi-core", "levi", "levi-hub", "levi-dash"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
myko = { version = "4.24", path = "../myko/libs/myko/core" }
myko-server = { version = "4.24", path = "../myko/libs/myko/server" }
myko-leptos = { version = "4.24", path = "../myko/libs/myko/leptos" }
levi-core = { path = "levi-core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ciborium = "0.2"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
anyhow = "1"
base64 = "0.22"
tokio = { version = "1", features = ["full"] }
log = "0.4"
env_logger = "0.11"
```

`.gitignore`: `target/`, `levi-dash/dist/`.

Start `levi-dash` as a stub lib crate (empty `lib.rs`, no leptos deps yet) so the workspace builds; it becomes real in Task 14.

- [ ] **Step 2: Entities** (levi-core/Cargo.toml deps: myko, serde, serde_json, chrono, uuid, anyhow, base64, workspace lints)

`entities/task.rs` — pattern for all:

```rust
use myko::prelude::*;

#[myko_subtype(derive(Default, Eq, Hash, PartialOrd, Ord, Copy))]
pub enum Priority { P0, P1, #[default] P2, P3 }

#[myko_item]
pub struct Task {
    pub project_id: String,
    #[searchable] pub title: String,
    #[serde(default)] #[searchable] pub body: String,
    #[serde(default)] pub priority: Priority,
    #[serde(default)] pub labels: Vec<String>,
    pub created_by_dev: String,
    pub created_by_machine: String,
    pub created_at: String,        // RFC3339
}
```

Remaining entities (each in its own file, same shape):

```rust
#[myko_item] pub struct Project { pub name: String, pub created_at: String }
// id = project UUID (simple hex)

#[myko_subtype(derive(Eq, Copy))] pub enum StatusKind { Closed, Reopened }
#[myko_item] pub struct StatusChange {
    pub project_id: String, pub task_id: String, pub to_status: StatusKind,
    #[serde(default)] pub anchor_commit: Option<String>,   // full sha hex; None = applies everywhere
    pub at: String, pub by_dev: String, pub by_machine: String,
}   // id = uuid; append-only, never edited

#[myko_item] pub struct Dependency { pub project_id: String, pub blocker_task_id: String, pub blocked_task_id: String }
// id = format!("{blocker}->{blocked}") — deterministic, so `dep add` is idempotent and `dep rm` is a DEL

#[myko_item] pub struct Claim {
    pub project_id: String, pub task_id: String, pub dev: String, pub machine: String,
    pub worktree: String, pub at: String, pub ttl_secs: u64,
}   // id = task_id — SET overwrite means newest-wins per task by LWW replay order

#[myko_item] pub struct Comment { pub project_id: String, pub task_id: String, pub body: String, pub by_dev: String, pub at: String }
// id = uuid; append-only

#[myko_item] pub struct CommitFact { pub project_id: String, pub parents: Vec<String> }
// id = commit sha hex — content-addressed, immutable

#[myko_item] pub struct RefFact { pub project_id: String, pub branch: String, pub head: String, pub observed_at: String }
// id = format!("{project_id}:{branch}") — LWW newest observation wins

#[myko_item] pub struct LogEntry { pub project_id: String, pub cbor_b64: String, pub created_at: String }
// id = event OID (git blob sha of CBOR bytes); payload = base64(CBOR(MEvent)); hub transport wrapper
```

`entities/mod.rs`: `pub use` globs. `lib.rs`:

```rust
pub mod entities; pub mod ids; pub mod materialize; pub mod resolve; pub mod rank;
pub use entities::*;
```

(`ids`, `materialize`, `resolve`, `rank` start as empty modules; filled in Tasks 2–4.)

- [ ] **Step 3: Round-trip test** in `entities/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use myko::wire::{MEvent, MEventType};
    #[test]
    fn task_event_roundtrip() {
        let t = Task { id: "abc123".into(), project_id: "p".into(), title: "T".into(),
            body: String::new(), priority: Priority::P1, labels: vec!["x".into()],
            created_by_dev: "d@e".into(), created_by_machine: "m".into(),
            created_at: "2026-07-18T00:00:00Z".into() };
        let ev = MEvent::from_item(&t, MEventType::SET, "m");
        let mut buf = Vec::new();
        ciborium::into_writer(&ev, &mut buf).unwrap();
        let back: MEvent = ciborium::from_reader(buf.as_slice()).unwrap();
        let t2: Task = serde_json::from_value(back.item.clone()).unwrap();
        assert_eq!(t, t2);
        assert_eq!(back.item_type, "Task");
    }
}
```

- [ ] **Step 4: `cargo test -p levi-core` green; fix myko API drift by reading the macro source if the report was stale.**
- [ ] **Step 5: Commit** `feat: workspace scaffold and levi-core entities`

---

### Task 2: Materialization (`World`)

**Files:** `levi-core/src/materialize.rs`

**Interfaces (produces):**

```rust
pub struct EventRecord { pub id: String, pub event: MEvent }        // id = blob OID hex
pub struct World {
    pub project: Option<Project>,
    pub tasks: BTreeMap<String, Task>,                // key = entity id
    pub status_changes: Vec<StatusChange>,            // sorted (at, id)
    pub deps: BTreeMap<String, Dependency>,
    pub claims: BTreeMap<String, Claim>,              // key = task_id
    pub comments: Vec<Comment>,                       // sorted (at, id)
    pub commit_facts: BTreeMap<String, CommitFact>,
    pub ref_facts: BTreeMap<String, RefFact>,
}
pub fn materialize(records: Vec<EventRecord>) -> World
impl World { pub fn changes_for(&self, task_id: &str) -> Vec<&StatusChange>;
             pub fn live_claim(&self, task_id: &str, now: DateTime<Utc>) -> Option<&Claim>; }
```

- [ ] **Step 1: Failing tests** — LWW by created_at across two SETs of the same Task id arriving out of order; tie on created_at broken by record id (lexicographic OID); DEL removes a Dependency; claim expiry (`at + ttl_secs <= now` ⇒ `live_claim` None); Project captured from first Project SET.
- [ ] **Step 2: Implement** — sort records by `(event.created_at.clone(), id.clone())`; dispatch on `event.item_type` strings (`"Task"`, `"StatusChange"`, …) deserializing `event.item` with `serde_json::from_value`; SET inserts/overwrites, DEL removes; unknown item_type ignored (forward compat). Sort `status_changes`/`comments` by `(at, id)` at the end.
- [ ] **Step 3: Tests green. Commit** `feat: deterministic event materialization`

---

### Task 3: Status resolution

**Files:** `levi-core/src/resolve.rs`

**Interfaces (produces):**

```rust
pub enum Ancestry { Yes, No, Unknown }
pub trait AncestorSet { fn contains(&mut self, sha: &str) -> Ancestry }
#[derive(Clone, Copy, PartialEq)] pub enum Status { Open, Closed }
#[derive(Clone, Copy, PartialEq)] pub enum Resolution { Exact, Facts, Partial }
pub struct ResolvedStatus { pub status: Status, pub resolution: Resolution }
pub fn effective_status(changes: &[&StatusChange], anc: &mut impl AncestorSet, base: Resolution) -> ResolvedStatus
pub struct FactsAncestors { /* built from &BTreeMap<String, CommitFact> + head sha */ }
impl FactsAncestors { pub fn new(facts: &BTreeMap<String, CommitFact>, head: &str) -> Self }
impl AncestorSet for FactsAncestors { ... }
pub struct MapAncestors(pub HashSet<String>);   // test helper: Yes iff contained, else No
pub struct PartialMapAncestors { pub yes: HashSet<String>, pub unknown: HashSet<String> }
```

Semantics (spec §Status resolution): fold changes in `(at, id)` order; a change applies iff `anchor_commit` is `None` or ancestry is `Yes`; any `Unknown` encountered downgrades resolution to `Partial` (task flagged, unknown close treated as open). No applicable change ⇒ `Open`. `base` is `Exact` (gix) or `Facts` (fact graph).

`FactsAncestors`: BFS closure from `head` over `CommitFact.parents`, memoized; if the walk ever needs a sha with no fact, closure is incomplete ⇒ non-members answer `Unknown` instead of `No` (head itself missing ⇒ everything `Unknown`).

- [ ] **Step 1: Failing tests** — table-driven over synthetic DAGs:
  - close anchored on ancestor ⇒ Closed/Exact; on non-ancestor branch ⇒ Open.
  - close then reopen (both ancestors) ⇒ Open; reopen only on other branch ⇒ Closed there-not-here.
  - unanchored close ⇒ Closed everywhere; `--no-anchor` reopen after anchored close ⇒ Open everywhere.
  - LWW tie on `at` broken by change id.
  - unknown anchor ⇒ Open + Partial; unknown anchor on a *reopen* with a later solid close ⇒ Closed + Partial.
  - FactsAncestors: linear chain, diamond merge, incomplete graph (missing parent fact ⇒ Unknown for absent shas, Yes still exact for reached shas).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: per-checkout status resolution`

---

### Task 4: Ranking (`levi next` core)

**Files:** `levi-core/src/rank.rs`, `levi-core/src/ids.rs`

**Interfaces (produces):**

```rust
pub struct Identity { pub dev: String, pub machine: String, pub worktree: String }
pub struct RankedTask { pub task_id: String, pub priority: Priority, pub unblocks: usize,
                        pub created_at: String, pub reason: String }
pub fn rank_next(world: &World, statuses: &BTreeMap<String, ResolvedStatus>,
                 now: DateTime<Utc>, me: &Identity) -> Vec<RankedTask>
// ids.rs
pub fn short_id(id: &str) -> String                       // "lv-" + first 4 hex (extend to 6/8 on collision within world)
pub fn resolve_prefix<'a>(world: &'a World, input: &str) -> Result<&'a Task, PrefixError>
pub enum PrefixError { NotFound(String), Ambiguous(String, Vec<String>) }
```

Rules: eligible = status Open ∧ every blocker's status Closed ∧ no live claim by a different `(dev, machine, worktree)`. Order: priority asc (P0 first) → transitive-unblock count desc (count of *open* tasks transitively blocked, cycle-safe via visited set) → `created_at` asc → task id asc. `reason` e.g. `"P0; unblocks 3 open tasks; oldest eligible"`. `resolve_prefix` accepts with/without `lv-` prefix, matches on id hex prefix.

- [ ] **Step 1: Failing tests** — priority beats unblock count; unblock count beats age; blocked task ineligible until blocker closed *on this checkout* (reuse MapAncestors DAG); own claim doesn't exclude, foreign live claim does, expired foreign claim doesn't; dep cycle doesn't loop; prefix resolution exact/ambiguous/missing.
- [ ] **Step 2: Implement. Tests green. Commit** `feat: deterministic next-task ranking`

---

### Task 5: Git event store (gix)

**Files:** `levi/Cargo.toml` (deps: levi-core, myko, gix 0.85, clap 4 derive, ciborium, serde_json, chrono, uuid, anyhow, hostname 0.4, base64, tokio [rt, macros for hub client later], log, env_logger; dev-deps: assert_cmd 2, predicates 3, tempfile 3), `levi/src/store.rs`, `levi/src/main.rs` (stub)

**Interfaces (produces):**

```rust
pub const EVENTS_REF: &str = "refs/levi/events";
pub struct EventStore { repo: gix::Repository }
impl EventStore {
    pub fn discover(dir: &Path) -> anyhow::Result<Self>;             // gix::discover, worktree-aware
    pub fn read(&self) -> anyhow::Result<Vec<EventRecord>>;          // absent ref => Ok(vec![])
    pub fn append(&self, events: Vec<MEvent>) -> anyhow::Result<Vec<String>>;  // returns new event ids; CAS+retry(5)
    pub fn merge_remote(&self, remote_ref: &str) -> anyhow::Result<usize>;     // tree union + merge commit; new-event count
    pub fn encode(event: &MEvent) -> Vec<u8>;                        // ciborium
}
```

Append algorithm (spec §Storage): read current ref head (or none) → write each event as a blob (`repo.write_blob`) → build updated tree sharded `id[..2]/id[2..]` (load existing shard trees, insert entries, write subtrees + root via `gix::objs::Tree` and `repo.write_object`) → write commit (`gix::objs::Commit` with tree, parent = old head, committer `levi <levi@localhost>` static signature, message `levi: N event(s)`) → ref edit `Change::Update { expected: PreviousValue::MustExistAndMatch(old) | MustNotExist, new }` → on transaction rejection, re-read and retry (blobs already written are reused), max 5 then error. `merge_remote`: union both trees (per-shard entry union), commit with two parents, same CAS loop. **The gix 0.85 API names here are best-effort — verify against docs.rs/gix at implementation time and adapt; the algorithm is fixed, the calls aren't.**

- [ ] **Step 1: Failing test** (`levi/tests/store.rs`, harness inline for now): temp dir, `git init`, append 3 events → read returns 3 with matching OIDs (`git cat-file` cross-check one blob); append 2 more → 5; ref log shows 2 commits; working tree remains clean (`git status --porcelain` empty).
- [ ] **Step 2: Implement; test green.**
- [ ] **Step 3: CAS race test**: two `EventStore` handles on the same repo; interleave appends from 8 threads × 5 events; final read = 40 distinct events.
- [ ] **Step 4: Commit** `feat: content-addressed event store on refs/levi/events`

---

### Task 6: CLI context + `init`/`add`/`ls`/`show`

**Files:** `levi/src/{cli,ctx,config,output}.rs`, `levi/src/commands/{init,add,ls,show}.rs`, `levi/tests/common/mod.rs`, `levi/tests/cli_basic.rs`

**Interfaces (produces):**

```rust
pub struct LeviCtx { pub store: EventStore, pub world: World, pub identity: Identity,
                     pub project_id: String, pub config: LeviConfig }
impl LeviCtx { pub fn load(no_sync: bool) -> anyhow::Result<Self>;   // discover, read, materialize
               pub fn append_and_sync(&self, events: Vec<MEvent>) -> anyhow::Result<Vec<String>>; }
pub fn mevent<T: Eventable + Serialize>(item: &T, machine: &str) -> MEvent  // SET, created_at = now RFC3339
pub struct LeviConfig { pub hub: Option<String>, pub token: Option<String>, pub claim_ttl_secs: u64 /*86400*/ }
```

Identity: dev = `git config user.email` (error with guidance if unset), machine = hostname, worktree = canonicalized workdir. Config: `git config levi.hub` / `levi.token` override `~/.config/levi/config.toml` (`[hub] address/token`, `[claim] ttl_secs`). `append_and_sync`: append, then unless `--no-sync`/no hub, best-effort opportunistic hub push (wired in Task 11; no-op until then).

Command behavior:
- `init`: refuse if project events exist (`init refuses if project events already exist`); mint UUID (32-hex), append `Project` SET, print id. `--name` optional (default repo dir name).
- `add "title" [-p p0..p3] [-b body] [-l label]... [--dep ID]...`: create Task (uuid 32-hex id) + Dependency events for each `--dep` (prefix-resolved); prints `lv-xxxx <id>`.
- `ls [--json] [--all|--closed] [-l label] [--mine] [--branch X]`: resolve every task against HEAD (exact; Task 7 supplies GixAncestors — until then use `MapAncestors` of all repo commits? No: Task 7 lands before ls grows `--branch`; in this task `ls` only handles unanchored statuses via `effective_status` with a `NoRepoAncestors` stub answering `Unknown`). Default: open only. `--mine`: claimed by me. JSON: `{"schema":"levi.ls/1","resolution_mode":"exact","tasks":[{"id","short","title","status","resolution","priority","labels","claim":{...}|null,"created_at"}]}`.
- `show ID [--json]`: detail + comments + deps (each side with resolved status) + live claim + full status history with per-change `applies` bool. Schema `levi.show/1`.
- Fresh clone/no ref: read commands print guidance (`levi sync` or `git fetch origin 'refs/levi/*:refs/levi/*'`) and exit 0 with empty list; mutating commands other than `init` error clearly.

Test harness (`tests/common/mod.rs`): `TestRepo::new()` → tempdir, `git init -b main`, `git config user.email agent@test`, `user.name`, helper `commit(msg) -> sha`, `branch/checkout/merge` helpers, `levi(args) -> assert_cmd::Command` with cwd set.

- [ ] **Step 1: Failing tests**: init prints id; double-init errors; add then `ls --json` shows open task with short id; `show` by prefix works; ambiguous prefix errors listing candidates; `ls` in repo without ref prints fetch guidance.
- [ ] **Step 2: Implement cli.rs (clap derive, full command enum from the spec's CLI surface), ctx, config, output, the four commands.**
- [ ] **Step 3: Tests green. Commit** `feat: levi init/add/ls/show`

---

### Task 7: Anchored close/reopen + exact ancestry

**Files:** `levi/src/ancestors.rs`, `levi/src/commands/close.rs`, wire into `ls`/`show`, `levi/tests/cli_git_state.rs`

**Interfaces (produces):**

```rust
pub struct GixAncestors<'r> { /* repo, head Option<ObjectId>, cache HashMap<String, Ancestry> */ }
impl GixAncestors<'_> { pub fn new(repo: &gix::Repository) -> Self;              // head = HEAD commit (None if unborn)
                        pub fn at(repo: &gix::Repository, head: gix::ObjectId) -> Self; }
impl AncestorSet for GixAncestors<'_> { /* Yes iff merge_base(head, anchor) == anchor; sha missing from odb => Unknown */ }
```

- `close ID [--anchor SHA | --no-anchor]`: StatusChange{Closed, anchor = HEAD | SHA | None}. `reopen` identical with Reopened. Both refuse redundant transitions (already closed/open here) with a clear message unless `--force`.
- `ls --branch X`: resolve against branch head instead of HEAD (`GixAncestors::at`).

This is the heart of the spec — test it hard:

- [ ] **Step 1: Failing tests** (`cli_git_state.rs`, using TestRepo):
  1. close on feature branch → `ls` on feature: closed/gone; `ls` on main: still open; merge feature → main: closed on main.
  2. close at main, branch created *before* the close commit: open on that branch.
  3. `--no-anchor` close: closed on every branch.
  4. reopen on main after merge: open on main, closed on feature (reopen anchor not in feature ancestry).
  5. `--branch` flag matches checking out that branch.
  6. worktree: `git worktree add ../wt branchX` → running levi in the worktree resolves against branchX HEAD and shares the same event ref.
  7. `--json` carries `"resolution":"exact"`.
- [ ] **Step 2: Implement. Tests green. Commit** `feat: git-ancestry status resolution and anchored close/reopen`

---

### Task 8: dep / comment / edit

**Files:** `levi/src/commands/{dep,comment,edit}.rs`, tests appended to `cli_basic.rs`

- `dep add BLOCKED --on BLOCKER` (SET Dependency, deterministic id; reject self-dep and duplicates silently-idempotent), `dep rm BLOCKED --on BLOCKER` (DEL). Warn (not fail) when adding creates a cycle.
- `comment ID "text"` (append Comment).
- `edit ID [-p P] [--title T] [-l +label] [-l -label]`: read task from world, apply field changes, SET full Task (LWW handles concurrent edits; ties by event id per spec).

- [ ] **Step 1: Failing tests**: dep gates `next` eligibility (placeholder: assert `show` lists dep both directions); comment shows in `show` ordered by time; edit priority reflected in `ls --json`; concurrent edits from two clones settle identically after ref union (simulate: two clones of a bare repo, edit both, push/fetch/merge via Task 9's sync? — defer the two-clone variant to Task 10's convergence test; here test LWW within one repo via two appends with controlled created_at).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: dependencies, comments, edit`

---

### Task 9: Claims + `next`/`start`/`steal`/`drop`

**Files:** `levi/src/commands/{next,claim_ops}.rs`, `levi/tests/cli_next.rs`

- `next [-n N] [--json]`: rank via `rank_next`, print top N (default 1) with reason. Schema `levi.next/1` includes `reason` and `resolution`.
- `next --claim`: append Claim event **before** printing (CAS makes parallel agents on one machine serialize; loser's retry re-reads, sees claim, picks next task) — implement as: loop { rank; build claim for top task; append (CAS may retry internally which is fine — what matters is rank re-check after any lost race): after append, re-read world; if our claim is the live one for that task, print it; else continue loop }.
- `start ID` = claim explicitly; `steal ID` = claim ignoring existing live claim (newest wins); `drop ID` = DEL own claim (error if not ours).
- TTL from config (`default 24h, configurable`).

- [ ] **Step 1: Failing tests**: ranking order end-to-end (P0 first; unblocker beats older); blocked task appears in `next` only after `close` of blocker *on this branch*; `--claim` then second `next` returns a different task; `steal` moves the claim; `drop` frees it; expired ttl (set `levi.claim.ttlSecs` git config? — config file field `claim_ttl_secs`, test writes `~` override via `LEVI_CONFIG` env pointing at a temp toml) frees it. Parallel `next --claim` ×4 processes → 4 distinct tasks (the concurrency test from the spec).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: advisory claims and levi next`

---

### Task 10: Sync git leg + orphaned anchors

**Files:** `levi/src/commands/sync.rs` (git leg), `levi/src/ancestors.rs` (orphan detection), `levi/tests/cli_git_sync.rs`

- `sync [--no-git]` git leg: `git fetch <remote> +refs/levi/events:refs/levi/remotes/<remote>/events` (subprocess), then `EventStore::merge_remote`, then `git push <remote> refs/levi/events:refs/levi/events` (tolerate non-fast-forward by fetch-merge-retry ×3). Remote = `levi.remote` config or `origin`.
- Orphan detection (spec §Anchoring): for each *closed-here-relevant* anchor: if sha exists but is not an ancestor of any local branch/tag tip, print warning `anchor <sha> for lv-xxxx unreachable from any ref — likely rebased away; re-close at the new HEAD` during `ls`/`show`. Keep it cheap: only check anchors of tasks being displayed; cache per invocation.
- [ ] **Step 1: Failing tests**: two clones of a bare repo; close in A, sync A (push), sync B (fetch) → closed in B once B merges the fixing commit / immediately if B's HEAD contains anchor. Divergent appends in both → both sync → both `ls --json --all` byte-identical. Rebase scenario: close at sha, `git reset --hard` + new commit (orphan the anchor) → `ls` warns.
- [ ] **Step 2: Implement. Tests green. Commit** `feat: git-leg sync and orphaned-anchor warnings`

---

### Task 11: Hub server (`levi-hub`)

**Files:** `levi-hub/Cargo.toml` (levi-core, myko, myko-server, tokio, axum 0.7, tokio-tungstenite 0.24, tower-http fs, clap, anyhow, log, env_logger), `levi-hub/src/{main,front_door,unwrap_saga}.rs`

- `levi-hub serve --bind 0.0.0.0:7377 [--internal-port 7378] [--dash-dir DIR]`.
- Internal: `CellServer::builder().with_bind_addr(127.0.0.1:internal).with_postgres(PostgresConfig::from_env()?)` — `MYKO_POSTGRES_URL` unset ⇒ in-memory with a logged warning (spec wants Postgres persistence; env-gated).
- `unwrap_saga.rs`: `#[myko_saga]` on LogEntry SETs → decode base64+CBOR to `MEvent` → `ctx.apply_event`, so Task/StatusChange/etc. are queryable hub-side. (If `myko_saga` proves awkward, equivalent fallback: a `watch`-style subscription on the LogEntry store inside `after_init`.) Idempotent: applying the same entry twice converges (SET overwrite).
- Front door (axum on public bind): `GET /myko` with `Upgrade: websocket` → check token (`?token=` or `levi_token` cookie; token from `LEVI_HUB_TOKEN` env / `--token`; **no token configured ⇒ open hub, log warning**) → `tokio_tungstenite` connect to internal, bidirectional byte pipe. All other paths: static files from `--dash-dir` (404 JSON if unset). Wrong token ⇒ 401.

- [ ] **Step 1: Failing test** (`levi-hub/tests/hub.rs`): start hub in-process (no postgres) on random ports with token `t`; raw WS client without token → 401/refused; `MykoClient`-style WS with `?token=t` → connects; push a LogEntry SET wrapping a Task event → query `GetAllTasks` returns the task (saga unwrapped it).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: levi-hub CellServer with token front door`

---

### Task 12: Hub leg + facts leg + opportunistic sync

**Files:** `levi/src/hub_client.rs`, `levi/src/facts.rs`, extend `levi/src/commands/sync.rs`, `levi/tests/convergence.rs`

- `hub_client.rs`: `TokenTransport` wrapping `autosocket::AutoReconnectSocket` — `set_addr` appends `?token=<t>` before delegating (add `autosocket` dep). `MykoClient::with_transport(...)`, `set_address(hub)`, wait-connected with 5s timeout. One-shot helpers: `report<R,O>(...)`, `query_snapshot_via_watch<Q>(...)` (subscribe, first emission, cancel), `send_events(Vec<MEvent>)`.
- Hub leg: (a) fetch hub's LogEntry ids for this project — add to levi-core a `#[myko_report]` `GetLogEntryIdsByProject { project_id } -> LogEntryIdList { ids: Vec<String> }` (register handler hub-side; if custom reports resist, fall back to `GetLogEntrysByQuery(PartialLogEntry{project_id})` snapshot and accept the bandwidth, per spec "Merkle-style comparison is a later optimization"); (b) push local-not-hub as LogEntry SET batch; (c) pull hub-not-local: fetch entries by ids, decode, verify OID = blob hash, append raw CBOR blobs into the ref (skip OID mismatches with a warning).
- Facts leg: after close/reopen and during sync: collect anchors of local StatusChanges + all branch heads; walk ancestors via gix (depth cap 2000 per tip); publish `CommitFact` + `RefFact` SETs directly to hub (not into the ref — deviation 6); dedupe with `.git/levi/facts-published` (one sha per line, append-only cache).
- Opportunistic sync: `append_and_sync` now spawns the hub push (and facts) best-effort with a 3s budget, silent on failure (`--no-sync` skips; `sync` command reports errors loudly).
- `sync --no-git`/`--no-hub` flags per spec.

- [ ] **Step 1: Failing convergence test** (spec §Testing): repos A and B (no shared git remote), in-process hub; A adds+closes tasks, B adds+comments offline; `sync --no-git` both twice (push/pull rounds); assert both repos' event-id sets identical and `ls --json --all` output byte-identical; assert hub `GetAllTasks` sees both projects' tasks with `resolution: "facts"` semantics available (RefFact for main exists).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: hub sync leg and commit-graph facts`

---

### Task 13: `levi watch`

**Files:** `levi/src/commands/watch.rs`

- Requires configured hub (deviation 8; else exit 2 with guidance). Connect, `watch_query(GetLogEntrysByQuery(project))`, on each new entry decode and print one JSON line `{"schema":"levi.watch/1","id","item_type","change_type","created_at","item"}` (`--json` is the only mode; human mode prints `item_type id summary`). Ctrl-C clean exit.
- [ ] **Step 1: Test**: with hub running, `watch` process sees an `add` from another repo handle within 5s (integration test with timeout; mark `#[ignore]`-not — keep it, it's in-process).
- [ ] **Step 2: Implement. Tests green. Commit** `feat: levi watch live stream`

---

### Task 14: Dashboard (`levi-dash`)

**Files:** `levi-dash/{Cargo.toml,Trunk.toml,index.html}`, `levi-dash/src/{main,app}.rs`, `levi-dash/src/pages/{overview,in_flight,browser}.rs`, `levi-dash/src/resolve_client.rs`

- Leptos 0.8 `csr`, `myko-leptos`, `levi-core` (entities shared verbatim), `console_error_panic_hook`, `wasm-bindgen`. Trunk build; **exclude levi-dash from the default workspace build** (`default-members` = the other three) so `cargo test` stays native; build dash via `trunk build` in CI/manually.
- `app.rs`: read `?token=` from URL → set `levi_token` cookie via `web_sys` → `provide_myko(host)` (same origin, so WS carries the cookie through the front door). Router with 3 routes.
- **Overview**: `live_query(GetAllProjects)` + per-project `live_report(CountTasks)` open/closed counts (open count computed client-side from tasks + status fold — counts need resolution, so: `live_query(GetAllTasks)` + `live_query(GetAllStatusChanges)` + `live_query(GetAllRefFacts)`, resolve against each project's default branch RefFact using `FactsAncestors` from levi-core in WASM); P0 alerts; activity feed = `live_query(GetAllLogEntrys)` tail sorted desc, rendered as event descriptions.
- **In flight**: `live_query(GetAllClaims)` grouped dev → machine → worktree, expired claims greyed.
- **Project browser**: task list with `ls` filters (label/status/mine) + branch selector fed by RefFacts; resolution via `FactsAncestors` (this is `resolve_client.rs` — thin wrapper choosing head from selected RefFact); task drawer: comments, deps, status history via `Get*sByQuery` live queries filtered to the task.
- [ ] **Step 1: `trunk build` compiles; `cargo test -p levi-core` still green (resolution logic reused, no WASM e2e per spec).**
- [ ] **Step 2: Manual smoke via `levi-hub serve --dash-dir levi-dash/dist` + one synced repo; fix what's broken.**
- [ ] **Step 3: Commit** `feat: leptos dashboard`

---

### Task 15: Polish + full-suite verification

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets` clean (allow documented exceptions).
- [ ] `cargo test --workspace` green; run the parallel-claim and convergence tests 3× for flake check.
- [ ] README.md: install, quickstart (init → add → close → next), sync setup (git remote + hub), hub deployment (env vars incl. `MYKO_POSTGRES_URL`, `LEVI_HUB_TOKEN`), dashboard build. No license file (spec: deferred).
- [ ] Update CLAUDE.md Status section: implemented; plan reference.
- [ ] Commit `docs: README and status`, then use superpowers:finishing-a-development-branch.

## Self-review notes

- Spec coverage: storage/CAS (T5), status resolution incl. unknown/facts (T3/T7), anchoring rules + orphan warnings (T7/T10), all CLI commands (T6–T13: init/add/ls/show T6, close/reopen T7, dep/comment/edit T8, next/start/steal/drop T9, sync T10/T12, watch T13, hub serve T11), ranking (T4/T9), identity (T6), edge cases: concurrent appends (T5/T9), fresh clone (T6), clock skew (T2), unknown ancestry (T3), large repos partially (depth-capped facts T12; gix commit-graph acceleration left to gix defaults — acceptable), dashboard 3 pages (T14), all four test categories (T2–T4 unit, T6–T10 CLI integration, T12 convergence, T14 core-level only).
- Known risks called out where they live: gix 0.85 exact API names (T5), `myko_saga` ergonomics (T11 fallback), custom report registration (T12 fallback), `set_address` query-param stripping (T12 TokenTransport is the mitigation; verify against autosocket source).
