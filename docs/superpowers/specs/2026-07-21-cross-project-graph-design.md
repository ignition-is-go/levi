# Cross-project issue visibility

*Design spec, 2026-07-21. Status: approved pending review.*

## Problem

levi has no view of issues across projects. `levi ls` is scoped to the local
repo's project; the dashboard's Browser page shows one project at a time
behind a selector; Overview shows per-project rollups. The hub already holds
every project's tasks, dependencies, and the CommitFacts/RefFacts needed to
resolve status — nothing surfaces them together.

Two concrete failures:

1. **An agent cannot assign a cross-project blocker.** `levi dep add <id>
   --on <project>/lv-xxxx` requires knowing the foreign task's id, and
   nothing in the CLI lists foreign tasks. Today you find the id by asking a
   human or opening the dashboard.
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
backlog grows, because it never tries to draw the parts that have no
structure.

## Goals

- The CLI can list another project's issues, in a form directly usable as
  `levi dep add --on <ref>`.
- One dashboard page showing every project's issues with blocking drawn.
- Correct per-project status resolution (git ancestry, not a flat bit), with
  honest reporting when it cannot be established.
- Readable and affordable at 3 tasks and at 3,000.

## Non-goals

- Cross-project ranked work selection (`levi next` hub-wide).
- Mutating foreign tasks beyond what `levi add --project` already does.
- A local cache of foreign facts (see Cost; a follow-up if listing is slow
  in practice).
- Pagination or virtualized rendering.

## Shared layer: `levi-core`

Both surfaces need the same thing: given entities pulled from the hub,
resolve each project's task statuses against that project's own history.
That logic lives in levi-core, not in either consumer.

levi-dash is wasm-only, excluded from `default-members`, and has no test
suite — logic placed there cannot be tested in CI. levi-core already owns
`resolve`, `materialize`, and `rank`, is tested natively, and this is the
same kind of pure fold. Today `levi-dash/src/resolve_client.rs` holds a
per-project resolution helper that the CLI would otherwise have to
duplicate; it moves to core and the dash keeps a thin caller.

### `levi_core::crossproject`

```rust
/// The branch a project resolves against when no branch is named:
/// its `main` RefFact if present, else the most recently observed.
pub fn default_head(refs: &[RefFact], project_id: &str) -> Option<String>;

/// task id -> resolved status for one project against one head.
/// `None` head yields Resolution::Partial for anchored changes rather
/// than guessing.
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
deepest blocker. Deterministic — a sparse DAG lays out the same way every
render, so nodes don't jitter as live data arrives. Cycles are broken by
dropping the edge that closes them, recorded in `broken_cycles`; nothing in
levi prevents filing a mutual block. Nodes sort within a layer by (project,
priority, created) so render order is stable too.

## Surface 1: CLI

Two new flags on `levi ls`, mutually exclusive:

- `--project <name|id>` — one foreign project (resolved through the existing
  hub registry lookup, which already handles name/id and ambiguity).
- `--all-projects` — every project on the hub, including the local one.

Both require a hub; without one, they fail with the existing message style:
"cross-project listing needs a hub: run `levi init --hub <host:port>`".
Plain `levi ls` is unchanged and stays fully offline.

Every existing filter (`--json`, `-l/--label`, `--closed`, `--all`) applies.
`--branch` is rejected with these flags: it names a branch in *this* repo,
which is meaningless across projects.

### Output

Text, project-qualified so it reads unambiguously:

```
levi/lv-5b89          P1 open  Hub access control: authenticate the WS endpoint
myko/lv-a544          P2 open  hub ingest needs bounded queues
pulse-deploy/lv-77c1  P2 open  converge drift on render-15
```

JSON adds three fields to the existing task schema:

- `project` — the project's name
- `project_id` — its id
- `ref` — the `<project>/lv-xxxx` string, **exactly the form `levi dep add
  --on` accepts**

That `ref` field is the point of the feature: discovering a blocker and
assigning it becomes one pipe, with no id reconstruction by the caller.

```
$ levi ls --project myko --json | jq -r '.tasks[] | select(.title|test("queue")) | .ref'
myko/lv-a544
$ levi dep add lv-39ad --on myko/lv-a544 --via "cargo: crates.io myko >=4.24.4"
```

### Resolution basis

The local project resolves **exactly** against the real git repo, as always.
Foreign projects resolve from hub facts against their default branch. The
existing `resolution` field already carries this distinction (`exact` |
`facts` | `partial`) and is populated per row, so a caller can tell how much
to trust each answer. The text output marks non-exact rows.

## Surface 2: dashboard

A new **Issues** page alongside Overview / Browser / In-Flight. Browser keeps
its per-project job and branch selector; this answers a different question.

Data comes from live queries the dashboard already issues — `GetAllProjects`,
`GetAllTasks`, `GetAllStatusChanges`, `GetAllCommitFacts`, `GetAllRefFacts`,
`GetAllDependencys`. No new hub queries.

Layout: graph pane left, backlog pane right, one shared filter row above
(status, free text, project multi-select). The task drawer is extracted from
Browser into a shared component so both pages open the same detail view.

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

## Cost, and how it degrades

Resolving a foreign project's statuses needs its CommitFact graph. myko has
**27,596** on the hub today. The existing `foreign::refresh_cache` already
pulls all of a project's facts per blocker, so this is a pre-existing cost,
but a cross-project `ls` would pay it per project on every invocation.

Three mitigations, in the order they apply:

1. **Skip projects that need no ancestry.** A task with no StatusChange is
   open by definition. Fetch a project's facts only if at least one listed
   task has an anchored status change.
2. **Bound the wait.** Fact fetches use the standard 10s leg timeout.
3. **Degrade honestly, never guess.** If a project's facts can't be fetched
   in time, its anchored tasks are reported with `resolution: "partial"` —
   levi's existing vocabulary for "this status is not established" — rather
   than defaulting to open or failing the whole listing. Other projects still
   list normally.

A local cache of foreign facts is the obvious next step if this proves slow;
it is deliberately out of scope until measured.

## Error and empty states

- **No hub configured**: clear error naming `levi init --hub`.
- **Unknown project name**: the registry lookup's existing error, which lists
  candidate ids on ambiguity.
- **No dependencies anywhere** (dashboard): graph pane says "nothing is
  blocked" and the backlog takes full width. This is what a fresh hub looks
  like; it must not read as a broken chart.
- **A dependency referencing an unknown task**: render a stub node labelled
  with its short id and project, visually distinct. Dropping the edge would
  silently hide a real blocker.
- **Cycles**: flagged from `broken_cycles`, naming the dropped edge.

## Testing

`levi-core` unit tests (native, the reason the logic lives here):

- graph: linear chain layers 0,1,2; diamond; cycle broken and recorded
  without hanging; cross-project edges carry `blocker_project_id` and `via`;
  closed blocker marks `resolved`; unconnected tasks excluded from `nodes`;
  the no-dependencies case
- crossproject: `default_head` prefers `main`, falls back to most-recently
  observed, and yields `None` for a project with no RefFacts; anchored
  changes with a `None` head resolve `Partial`

CLI integration tests, against the in-process hub already used by
`cross_project.rs`:

- `--project <name>` lists the foreign project's tasks with `ref` in
  `<project>/lv-xxxx` form
- a `ref` from that output is accepted verbatim by `levi dep add --on`
- `--all-projects` includes local tasks resolved `exact` and foreign tasks
  resolved `facts`
- both flags without a hub fail with the hub-required message
- `--branch` combined with either flag is rejected
- a task with no status changes lists as open without any facts fetched

Dashboard rendering stays untested, as today.

## Sequencing

Two implementation plans, in order:

1. **Core layer + CLI** — `crossproject` and `graph` modules, `resolve_client`
   moved out of the dash, the two `ls` flags. Delivers the blocker-assignment
   workflow on its own.
2. **Dashboard Issues page** — consumes the same core modules.
