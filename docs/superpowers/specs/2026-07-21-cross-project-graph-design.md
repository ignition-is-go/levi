# Cross-project issue visibility

*Design spec, 2026-07-21. Status: approved pending review.*

## Problem

levi has no way to see issues in other projects. `levi ls` is scoped to the
local repo's project; the dashboard's Browser page shows one project at a
time behind a selector; Overview shows per-project rollups.

Note what already works: you can *write* across projects today — `levi add
--project myko "title"` files a bug there, `levi comment myko/lv-a544 "..."`
comments, both through the hub. What is missing is **discovery**. Two
concrete failures:

1. **An agent cannot assign a cross-project blocker.** `levi dep add <id>
   --on <project>/lv-xxxx` requires the foreign task's id, and nothing in the
   CLI lists foreign tasks.
2. **Blocking relationships are invisible.** Cross-project deps exist
   (levi/lv-39ad and levi/lv-5f11 both wait on myko/lv-a544) and can only be
   seen by opening each task individually.

## The shape of the data

**~200 tasks across 3 projects, 3 dependency edges.** A node-link graph of
all tasks would be ~195 unconnected dots with three lines through them —
literally "everything and the blocking relationships", practically
unreadable. So the graph shows only tasks that participate in a dependency;
everything else is a list beside it.

## Architecture: compute on the hub, ship the answer

The governing principle (owner's direction, 2026-07-21): **prefer
server-side joins; avoid pulling raw entity sets — least of all the fact
log — across the wire.** The hub holds every project's tasks, status
changes, and the full CommitFact graph. It is the right place to run
resolution, and myko already has the machinery: a `ReportHandler` whose
`compute()` is gated `#[cfg(not(target_arch = "wasm32"))]` runs on the hub,
queries the hub's own stores in-process, and returns a small computed value.
The merkle bucket sync (`LogEntryBuckets`) already works exactly this way —
it hashes the log server-side so only differing buckets transfer.

This inverts the naive design. Instead of a client pulling `GetAllTasks` +
`GetAllStatusChanges` + `GetAllCommitFacts` and folding locally, the client
calls a report and the fold happens hub-side over data that never leaves the
hub. The fold logic still lives in levi-core (native + tested); it is simply
*invoked* inside the report handler rather than in each consumer.

### New levi-core reports (hub-computed)

```rust
// Cheap listing: tasks with unanchored status. No facts touched.
#[myko_report(TaskListingOut)]
pub struct TaskListing { pub project_id: Option<String> }  // None = all projects
pub struct TaskRow { project_id, project, task_id, r#ref, title,
                     priority, status, resolution }   // ref = "<project>/lv-xxxx"

// The dependency graph, fully resolved hub-side.
#[myko_report(IssueGraphOut)]
pub struct IssueGraphReport {}
// Output is the IssueGraph below: nodes (with status), edges, unconnected count,
// broken cycles. The client renders; it never sees a Task, Dependency, or fact.
```

Both `compute()` bodies call the pure folds in `levi_core::crossproject` and
`levi_core::graph` (below), so the logic is unit-tested natively and reused
by any future consumer. levi-hub picks the reports up through the existing
`levi_core::link()`.

### Pure folds in levi-core (the tested core)

`levi_core::crossproject::statuses_unanchored(tasks, changes, project_id)` —
status from StatusChanges alone: **open** when a task has no close event,
**closed (partial)** when it has one. No ancestry, so no facts. This is what
both the listing and the graph use; branch-exact resolution is more than
either needs (see Precision).

`levi_core::graph::build(tasks, deps, statuses) -> IssueGraph` — nodes are
only tasks in a dependency; layered longest-path from the roots
(deterministic, so no jitter as data arrives); cycles broken and recorded in
`broken_cycles`; unconnected task ids collected separately. Types as in the
prior draft (`GraphNode`/`GraphEdge`/`IssueGraph`), `GraphEdge.resolved` set
from the blocker's status, `via` carried through for cross-project edges.

`levi-dash/src/resolve_client.rs` (branch-exact resolution over facts) moves
into `levi_core::crossproject::statuses_for_project` so nothing but the
report handlers ever holds resolution logic.

## Precision

Resolving *exactly* which branches contain a close needs the CommitFact
graph. For **choosing a blocker**, that is more than needed: you need the
task's existence, id, title, and roughly whether it is done. So both new
surfaces resolve **unanchored** — open / closed(somewhere) — which needs no
facts at all, on either side of the wire.

Exact per-branch status still happens where it is already paid for: once a
dependency exists, `foreign::refresh_cache` resolves that blocker against its
branch and `levi next` reports the true unblock with the `--via` note.
Precision arrives when it is actionable. (Because facts live on the hub, a
future report *could* resolve exactly server-side without shipping the graph
— a cheap upgrade if ever wanted, deliberately not in scope.)

## Surface 1: CLI

Two mutually exclusive flags on `levi ls`, each backed by the `TaskListing`
report:

- `--project <name|id>` — one foreign project (registry lookup handles
  name/id and ambiguity).
- `--all-projects` — every project including the local one.

Both require a hub; without one they fail "cross-project listing needs a
hub: run `levi init --hub <host:port>`". Plain `levi ls` is unchanged and
fully offline. `--json`, `-l`, `--closed`, `--all` apply. `--branch` is
rejected (names a local branch; meaningless across projects, and unanchored
resolution has no branch to take).

Text output is project-qualified:
```
myko/lv-a544  P2 open               hub ingest needs bounded queues
myko/lv-a2ae  P1 closed (somewhere) autosocket frame duplication
```
"closed (somewhere)" states what is known and implies what is not; a bare
"closed" would claim branch truth this path never established.

JSON rows are `TaskRow`, whose `ref` field is `<project>/lv-xxxx` — **exactly
what `levi dep add --on` accepts**. Discovery-to-assignment is one pipe:
```
$ levi ls --project myko --json | jq -r '.tasks[]|select(.title|test("queue")).ref'
myko/lv-a544
$ levi dep add lv-39ad --on myko/lv-a544 --via "cargo: crates.io myko >=4.24.4"
```

## Surface 2: dashboard

A new **Issues** page (alongside Overview / Browser / In-Flight) that
subscribes to a single reactive report — `watch_report(IssueGraphReport)` —
and renders the returned nodes/edges plus a backlog list from `TaskListing`.
**It pulls no `GetAllCommitFacts`, no `GetAllTasks`, no `GetAllStatusChanges`.**
Browser keeps its per-project job, branch selector, and existing fact
subscription — branch-exact resolution is its actual purpose.

Layout: graph pane left, backlog right, one shared filter row (status, text,
project). The task drawer is extracted from Browser into a shared component.

Visual design follows the dataviz skill: node color = project identity from
the validated categorical palette in fixed order (name on the node too, so
never color-alone); left→right layout by layer; **resolved edges dashed and
muted, not hidden** (a just-closed blocker is the most actionable state —
"verify it reached you, then start"); edge hover shows `via`; status colors
from the reserved status palette, always with a label.

### Cost of a reactive server-computed report

`IssueGraphReport` recomputes when its inputs change. Unanchored resolution
touches only Tasks + StatusChanges + Dependencys (hub-wide ~200 + ~600 + a
handful) and no facts, so the fold is cheap and — critically — the output is
small and constant-ish regardless of history size. This is strictly better
than today's dashboard pages, which stream the entire CommitFact log to every
browser. Retrofitting Overview/Browser onto computed reports, and moving
`foreign::refresh_cache` off its full-graph pull, are the obvious next
applications of this pattern — noted as follow-ups, not this spec.

## Error and empty states

- **No hub**: clear error naming `levi init --hub`.
- **Unknown/ambiguous project**: the registry lookup's existing errors.
- **No dependencies anywhere** (dashboard): graph pane says "nothing is
  blocked", backlog takes full width. This is a fresh hub; it must not read
  as broken.
- **Dependency referencing an unknown task**: a stub node labelled with short
  id + project, visually distinct — dropping the edge would hide a real
  blocker.
- **Cycles**: flagged from `broken_cycles`, naming the dropped edge.

## Testing

levi-core unit tests (native — the reason the folds live here):
- graph: chain layers 0,1,2; diamond; cycle broken/recorded without hanging;
  cross-project edges carry `blocker_project_id` + `via`; closed blocker marks
  `resolved`; unconnected tasks excluded from `nodes`; no-dependencies case.
- crossproject: `statuses_unanchored` open with no changes, partial-closed
  with a close event, anchors ignored.

Report handlers are tested through the in-process hub used by
`cross_project.rs`:
- `TaskListing { project_id: Some }` returns that project's rows with correct
  `ref`; a returned `ref` is accepted verbatim by `levi dep add --on`.
- `TaskListing { None }` spans projects; a project whose CommitFacts are
  absent from the hub still lists (proves no fact dependency).
- `IssueGraphReport` returns the expected nodes/edges for a seeded
  cross-project dependency.

CLI integration tests: `--project`/`--all-projects` output and `ref`
round-trip; both flags without a hub fail with the hub-required message;
`--branch` rejected. Dashboard rendering stays untested, as today.

## Sequencing

1. **Core folds + reports + CLI** — `crossproject` and `graph` in levi-core,
   the `TaskListing` report, `resolve_client` moved into core, the two `ls`
   flags. Delivers the blocker-assignment workflow.
2. **Dashboard Issues page** — the `IssueGraphReport` report and the page
   consuming it.
