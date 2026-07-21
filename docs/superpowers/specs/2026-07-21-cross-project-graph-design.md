# Cross-project issue visibility

*Design spec, 2026-07-21. Status: approved pending review.*

## Problem

levi has no way to see issues in other projects. `levi ls` is scoped to the
local repo's project; the dashboard's Browser page shows one project at a
time behind a selector; Overview shows per-project rollups.

Note what already works: you can *write* across projects today — `levi add
--project myko "title"` files a bug there, and `levi comment myko/lv-a544
"..."` lands a comment, both through the hub. What is missing is purely
**discovery**. Two concrete failures:

1. **An agent cannot assign a cross-project blocker.** `levi dep add <id>
   --on <project>/lv-xxxx` requires the foreign task's id, and nothing in the
   CLI lists foreign tasks. Today you get the id from a human or the
   dashboard.
2. **Blocking relationships are invisible.** Cross-project deps exist
   (levi/lv-39ad and levi/lv-5f11 both wait on myko/lv-a544) and can only be
   seen by opening each task individually.

## The shape of the data

This drives the visual design: **~200 tasks across 3 projects, 3 dependency
edges.** A conventional node-link graph of all tasks would be ~195
unconnected dots with three lines through them — literally "everything and
the blocking relationships", practically unreadable.

So the graph shows only tasks that participate in a dependency, and
everything else lives in a list beside it. The picture stays true as the
backlog grows, because it never draws the parts that have no structure.

## Goals

- The CLI can list another project's issues, in a form directly usable as
  `levi dep add --on <ref>`.
- One dashboard page showing every project's issues with blocking drawn.
- Status reported at a precision each surface can afford, and labelled
  honestly with which precision that is.

## Non-goals

- Cross-project ranked work selection (`levi next` hub-wide).
- New foreign-write capability. `add --project` and `comment` already cover
  it, and assigning a blocker is a *local* write.
- Exact per-branch status for foreign projects in the CLI (see Precision).
- Pagination or virtualized rendering.

## Precision, and why the CLI does less

Resolving a project's task statuses exactly requires walking its CommitFact
graph — 27,596 facts for myko today. `foreign::refresh_cache` already pays
that per blocker, and a cross-project `ls` doing it per project per
invocation would make listing cost megabytes of ancestry.

It is also more precision than the job needs. When choosing a blocker to
depend on, you need to know the task exists, its id, its title, and roughly
whether it is done. So the CLI listing resolves status from **StatusChanges
alone, with no CommitFact fetch**:

- no status change → **open**, definitively, no ancestry required
- any close event → **closed somewhere**, reported as levi's existing
  `resolution: partial` — we know a close exists, not which branches contain
  it

Exact resolution still happens where it matters and is already paid for:
once a dependency exists, `foreign::refresh_cache` resolves that specific
blocker properly against its branch, and `levi next` reports the real unblock
with the `--via` verification note. Precision arrives when it is actionable,
not while browsing.

The dashboard keeps exact resolution because it already subscribes to every
CommitFact for other reasons — same core module, different cost profile.

## Shared layer: `levi-core`

Both surfaces fold hub entities into per-project answers. That logic lives in
levi-core, not in either consumer: levi-dash is wasm-only, excluded from
`default-members`, and has no test suite, so logic placed there cannot be
tested in CI. levi-core already owns `resolve`, `materialize`, and `rank`, is
tested natively, and this is the same kind of pure fold. Today
`levi-dash/src/resolve_client.rs` holds a per-project resolution helper the
CLI would otherwise duplicate; it moves to core and the dash keeps a thin
caller.

### `levi_core::crossproject`

```rust
/// Status from StatusChanges alone — no ancestry. Open when a task has no
/// close event; Closed/Partial when it does. What the CLI listing uses.
pub fn statuses_unanchored(
    tasks: &[Task],
    changes: &[StatusChange],
    project_id: &str,
) -> BTreeMap<String, ResolvedStatus>;

/// The branch a project resolves against when none is named: its `main`
/// RefFact if present, else the most recently observed.
pub fn default_head(refs: &[RefFact], project_id: &str) -> Option<String>;

/// Exact per-branch resolution over the fact graph. What the dashboard uses
/// (moved from levi-dash/src/resolve_client.rs).
pub fn statuses_for_project(
    tasks: &[Task],
    changes: &[StatusChange],
    facts: &[CommitFact],
    project_id: &str,
    head: Option<&str>,
) -> BTreeMap<String, ResolvedStatus>;
```

### `levi_core::graph`

```rust
pub struct GraphNode {
    pub task_id: String,
    pub project_id: String,
    pub title: String,
    pub priority: Priority,
    pub status: Status,
    pub layer: usize,        // 0 = blocks something, blocked by nothing
}

pub struct GraphEdge {
    pub blocker_task_id: String,
    pub blocked_task_id: String,
    pub blocker_project_id: Option<String>,  // None = same project
    pub via: Option<String>,
    pub resolved: bool,      // blocker is closed: the unblock is actionable
}

pub struct IssueGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub unconnected: Vec<String>,
    pub broken_cycles: Vec<(String, String)>,
}

pub fn build(
    tasks: &BTreeMap<String, Task>,
    deps: &BTreeMap<String, Dependency>,
    statuses: &BTreeMap<String, ResolvedStatus>,
) -> IssueGraph;
```

Layering is longest-path from the roots: a node's layer is one past its
deepest blocker. Deterministic — a sparse DAG lays out identically every
render, so nodes don't jitter as live data arrives. Cycles are broken by
dropping the edge that closes them, recorded in `broken_cycles`; nothing in
levi prevents filing a mutual block. Nodes sort within a layer by (project,
priority, created) so render order is stable too.

## Surface 1: CLI

Two mutually exclusive flags on `levi ls`:

- `--project <name|id>` — one foreign project, resolved through the existing
  hub registry lookup (which already handles name/id and reports ambiguity).
- `--all-projects` — every project on the hub, including the local one.

Both require a hub; without one they fail with the established message
style: "cross-project listing needs a hub: run `levi init --hub <host:port>`".
Plain `levi ls` is unchanged and stays fully offline.

Cost is flat: both fetch Tasks and StatusChanges only, hub-wide roughly 200
and 600 rows respectively. `--all-projects` is no more expensive than one
project.

`--json`, `-l/--label`, `--closed` and `--all` apply as usual. `--branch` is
rejected with these flags — it names a branch in *this* repo, which is
meaningless across projects, and the listing does not resolve per-branch
anyway.

### Output

```
myko/lv-a544  P2 open               hub ingest needs bounded queues
myko/lv-a2ae  P1 closed (somewhere) autosocket frame duplication
```

"closed (somewhere)" is deliberate: it states what is known and implies what
is not. A bare "closed" would claim branch-level truth this path never
established.

JSON adds three fields to the existing task schema:

- `project` — the project's name
- `project_id` — its id
- `ref` — the `<project>/lv-xxxx` string, **exactly the form `levi dep add
  --on` accepts**

`ref` is the point of the feature: discovery to assignment becomes one pipe,
with no id reconstruction by the caller.

```
$ levi ls --project myko --json | jq -r '.tasks[] | select(.title|test("queue")) | .ref'
myko/lv-a544
$ levi dep add lv-39ad --on myko/lv-a544 --via "cargo: crates.io myko >=4.24.4"
```

Rows carry `resolution: "partial"` for anything closed, so a caller can tell
this listing did not establish branch-level truth. Local rows under
`--all-projects` come from the real repo and are `exact`.

## Surface 2: dashboard

A new **Issues** page alongside Overview / Browser / In-Flight. Browser keeps
its per-project job and branch selector; this answers a different question.

Data comes from live queries the dashboard already issues — `GetAllProjects`,
`GetAllTasks`, `GetAllStatusChanges`, `GetAllCommitFacts`, `GetAllRefFacts`,
`GetAllDependencys`. No new hub queries.

Layout: graph pane left, backlog pane right, one shared filter row above
(status, free text, project multi-select). The task drawer is extracted from
Browser into a shared component so both pages open the same detail view.

Each project resolves exactly, against its own default branch, stated in the
UI — `main` in levi is unrelated to `main` in myko, and a silently assumed
basis would mislead.

### Visual design

Follows the dataviz skill's palette and rules.

- **Node color = project identity**, from the validated categorical palette
  in fixed order, never cycled. The project name is on the node too, so
  identity is never color-alone.
- **Layout left→right by layer**, blockers on the left.
- **Resolved edges (blocker closed) are dashed and muted, not hidden.** A
  blocker that just closed is the most actionable state in the system — "go
  verify the fix reached you, then start" — and hiding it destroys that
  signal.
- **Edge hover shows `via`.** For cross-project deps that annotation is the
  point: "cargo: crates.io myko >=4.24.4" says what must happen before the
  unblock is real.
- Status colors come from the reserved status palette and ship with a label.

## Error and empty states

- **No hub configured**: clear error naming `levi init --hub`.
- **Unknown or ambiguous project name**: the registry lookup's existing
  errors, which list candidate ids.
- **No dependencies anywhere** (dashboard): the graph pane says "nothing is
  blocked" and the backlog takes full width. This is what a fresh hub looks
  like; it must not read as a broken chart.
- **A dependency referencing an unknown task**: render a stub node labelled
  with its short id and project, visually distinct. Dropping the edge would
  silently hide a real blocker.
- **Cycles**: flagged from `broken_cycles`, naming the dropped edge.

## Testing

`levi-core` unit tests (native — the reason the logic lives here):

- graph: linear chain layers 0,1,2; diamond; cycle broken and recorded
  without hanging; cross-project edges carry `blocker_project_id` and `via`;
  a closed blocker marks `resolved`; unconnected tasks excluded from `nodes`;
  the no-dependencies case
- crossproject: `statuses_unanchored` reports open with no changes and
  partial-closed with a close event, ignoring anchors entirely; `default_head`
  prefers `main`, falls back to most-recently observed, and yields `None`
  with no RefFacts

CLI integration tests, against the in-process hub already used by
`cross_project.rs`:

- `--project <name>` lists the foreign project's tasks with `ref` in
  `<project>/lv-xxxx` form
- a `ref` from that output is accepted verbatim by `levi dep add --on`
- `--all-projects` includes both local and foreign tasks; local rows resolve
  `exact`, foreign closed rows `partial`
- listing fetches no CommitFacts (assert against a project whose facts are
  absent from the hub: it must still list)
- both flags without a hub fail with the hub-required message
- `--branch` with either flag is rejected

Dashboard rendering stays untested, as today.

## Sequencing

Two implementation plans, in order:

1. **Core layer + CLI** — `crossproject` and `graph` modules, `resolve_client`
   moved out of the dash, the two `ls` flags. Delivers the blocker-assignment
   workflow on its own.
2. **Dashboard Issues page** — consumes the same core modules.
