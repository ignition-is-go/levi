# Cross-project issue graph

*Design spec, 2026-07-21. Status: approved pending review.*

## Problem

levi has no view of issues across projects. `levi ls` is scoped to the local
repo's project; the dashboard's Browser page shows one project at a time
behind a selector; Overview shows per-project rollups. The hub already holds
every project's tasks, dependencies, and the CommitFacts/RefFacts needed to
resolve status — nothing surfaces them together.

Blocking relationships are the sharpest gap. Cross-project dependencies
exist today (levi/lv-39ad and levi/lv-5f11 both wait on myko/lv-a544) and
are invisible unless you open each task individually.

## The shape of the data

This drives the whole design: **~200 tasks across 3 projects, 3 dependency
edges.** A conventional node-link graph of all tasks would be ~195
unconnected dots with three lines through them — literally "everything and
the blocking relationships", practically unreadable.

So the graph shows only tasks that participate in a dependency, and
everything else lives in a list beside it. The picture stays true as the
backlog grows, because it never tries to draw the parts that have no
structure.

## Goals

- One page showing every project's issues, with blocking relationships drawn.
- Cross-project blockers legible at a glance, including the `via` mechanism
  a fix must travel through.
- Correct per-project status resolution (git ancestry, not a flat "closed"
  bit).
- Readable at 3 tasks and at 3,000.

## Non-goals

- A CLI equivalent (`levi graph --json`). The core model is built so this is
  cheap later, but it is not in this spec.
- Cross-project ranked work selection (`levi next` hub-wide).
- Editing from the graph. This is a read surface; mutations stay in the CLI.
- Pagination or virtualized rendering. At current scale the fold is trivial;
  revisit if a project reaches thousands of open tasks.

## Architecture

### `levi-core::graph` (new module)

The model is a pure fold in levi-core, not in the dashboard. levi-dash is
wasm-only, excluded from `default-members`, and has no test suite — logic
placed there cannot be tested in CI. levi-core already owns `resolve`,
`materialize`, and `rank`, is tested natively, and this is the same kind of
pure transformation. A future CLI command reuses it unchanged.

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
    pub unconnected: Vec<String>,   // task ids with no dependency edge
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
render, so nodes don't jitter or reshuffle as live data arrives. Cycles are
broken by dropping the edge that closes them, recorded in `broken_cycles` so
the UI can flag it; nothing in levi prevents filing a mutual block.

Nodes are sorted within a layer by (project, priority, created) so the
rendering order is stable too.

### `levi-dash`: new Issues page

A fourth page alongside Overview / Browser / In-Flight. Browser keeps its
per-project job and branch selector; this is a different question.

Data comes from live queries the dashboard already issues elsewhere —
`GetAllProjects`, `GetAllTasks`, `GetAllStatusChanges`, `GetAllCommitFacts`,
`GetAllRefFacts`, `GetAllDependencys`. No new hub queries.

Layout: graph pane left, backlog pane right, one shared filter row above
(status, free text, project multi-select). The task drawer is extracted from
Browser into a shared component so both pages open the same detail view.

### Status resolution

Each project resolves against **its own default branch** — its `main`
RefFact if present, otherwise its first branch (`branches()` already sorts
main-first). `resolve_client::statuses` is called once per project and the
results merged. The basis is stated in the UI ("resolved against each
project's default branch"), because `main` in levi is unrelated to `main` in
myko and a silently-assumed basis would be misleading.

## Visual design

Follows the dataviz skill's palette and rules.

- **Node color = project identity**, assigned from the validated categorical
  palette in fixed order, never cycled. The project name appears on the node
  as well, so identity is never color-alone.
- **Layout left→right by layer**, blockers on the left.
- **Resolved edges (blocker closed) are drawn dashed and muted, not hidden.**
  A blocker that just closed is the most actionable state in the system —
  "go verify the fix reached you, then start" — and hiding it destroys that
  signal.
- **Edge hover shows `via`.** For cross-project deps that annotation is the
  point: "cargo: crates.io myko >=4.24.4" says what has to happen before the
  unblock is real.
- **Node hover shows** title, priority, status, resolution basis. Click opens
  the drawer.
- Status colors (open/closed) come from the reserved status palette and ship
  with a label, never color alone.

## Error and empty states

- **No dependencies anywhere**: graph pane shows "nothing is blocked" and the
  backlog takes the full width. This is what a fresh hub looks like and it
  must not read as a broken chart.
- **No projects synced**: existing `EmptyState` component, as Overview does.
- **A dependency referencing an unknown task** (foreign task the hub has not
  seen): render the node as a stub labelled with its short id and project,
  visually distinct. Dropping the edge would hide a real blocker.
- **Cycles**: flagged in the UI from `broken_cycles`, with the dropped edge
  named.

## Testing

Unit tests in `levi-core` against the graph fold:

- a linear chain layers 0,1,2
- a diamond (two blockers, one blocked) layers correctly
- a cycle is broken, recorded, and does not hang
- cross-project edges carry `blocker_project_id` and `via`
- a closed blocker marks the edge `resolved`
- tasks with no edges land in `unconnected`, not `nodes`
- no dependencies at all yields empty nodes/edges and every task unconnected

Dashboard rendering stays untested, as today — which is the reason the model
lives in core rather than in the page.
