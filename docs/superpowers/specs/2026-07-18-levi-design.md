# levi — git-aware, agent-first, distributed issue tracker

*Design spec, 2026-07-18. Status: approved pending review.*

**levi**: the one who comes around about what's owed. A task tracker where
status is a function of git history, storage travels with the repo, and the
primary consumer is a coding agent asking "what should I work on next?"

## Problem

Task status in existing trackers is not correlated with git state. A task
closed on one branch appears closed everywhere, even on branches that do not
contain the fixing commits — so task lists are polluted with issues that are
resolved elsewhere but genuinely open *here*. Conversely, work completed on a
feature branch shows as outstanding on that branch's own list. Agents working
in parallel worktrees need a tracker that answers "what is open *on this
checkout*," coordinates claims across agents/machines, and works offline.

## Goals

- Task status resolved against git ancestry: closed-at-commit-X means closed
  only where X is an ancestor of HEAD.
- Distributed-first: fully functional offline from a bare clone; no required
  server or daemon.
- Optional aggregation: a hub server collects events from many repos/projects
  and serves cross-repo views — without cloning any repo.
- Agent-first surface: a CLI with stable `--json` output on every command;
  `levi next` deterministically surfaces the most important eligible work.
- Coordination: advisory claims keyed to (developer, machine, worktree) so
  parallel agents don't grab the same task.
- Live dashboards: a Leptos web UI connected to the hub.

## Non-goals (v1)

- MCP server mode (CLI + `--json` is the surface; MCP can wrap it later).
- Hard locks / guaranteed mutual exclusion on claims.
- Automatic re-anchoring across rebases/cherry-picks (detected and warned,
  fixed manually).
- CRDT field-level merging of task metadata (myko LWW is sufficient).
- Import/export bridges to GitHub Issues et al.

## Architecture

Cargo workspace, four crates:

| crate       | kind | role |
|-------------|------|------|
| `levi-core` | lib  | myko model: entities, commands, queries, ranking. No git, no terminal. |
| `levi`      | bin  | the CLI. Embeds a myko node per invocation. Git access via `gix`. |
| `levi-hub`  | bin  | myko `CellServer` + Postgres. Git-free aggregation server. |
| `levi-dash` | lib→wasm | Leptos CSR dashboard, served by the hub, connects as a myko client over WebSocket. |

There is **no daemon**. Each CLI invocation: discover git root (worktree-aware,
`gix`) → load events from the hidden ref → materialize myko state in memory →
execute one command → append new events to the ref (atomic CAS, retry on race)
→ opportunistic hub sync → exit. This is git's own model: nothing runs in the
background; sync piggybacks on invocations. `levi watch` is the long-running
exception, holding a live myko subscription for dashboards/agents that want
push instead of poll.

A future per-user system-wide daemon (single hub connection, repo registry,
cross-repo `next`) is compatible with this design but out of scope for v1.

### Identity

- **Project**: stable UUID minted by `levi init`, stored in the first event.
- **Developer**: `git config user.email`.
- **Machine**: hostname.
- **Worktree**: canonicalized worktree path.

All auto-detected; every event records its origin.

## Storage: the event ref

Canonical store is an append-only event log inside the repo under
`refs/levi/events` — never the working tree, so no diff/status/merge
pollution (git-bug precedent).

- Each event is one content-addressed CBOR blob (myko wire encoding).
- A commit on the ref holds a tree of all events, sharded by id prefix
  (`ab/cdef1234…`, like `.git/objects`).
- Append = write blobs + new tree + commit; update ref by compare-and-swap;
  on race, re-read, union, retry (bounded retries, then a clear error).
- Merging two histories = tree union + merge commit. Events are immutable
  and add-only, so union is always conflict-free; any sync path converges.
- Transport: `git push`/`fetch` of `refs/levi/*` (via `levi sync` or refspec),
  and/or the hub. Clones carry the full task history once the ref is fetched.

myko materializes state from this log at startup (its normal replay model);
for a task tracker the log is thousands of small events — milliseconds.

## Data model

All `#[myko_item]` entities in `levi-core`. The key decision: **status is not
a field on Task**. It is derived per-checkout from immutable status-change
records.

- **`Task`** — id, title, body, priority (P0–P3), labels, created_by
  (dev/machine), created_at. Metadata edits are ordinary myko SET events,
  last-writer-wins by `created_at`, ties broken by event id.
- **`StatusChange`** — task_id, to_status (`closed` | `reopened`),
  anchor_commit (optional sha), at, by. Append-only, never edited.
- **`Dependency`** — blocker_task_id, blocked_task_id.
- **`Claim`** — task_id, dev, machine, worktree, at, ttl. Advisory; newest
  wins; ignored after ttl expires (default 24h, configurable).
- **`Comment`** — task_id, body, by, at. Append-only.
- **`CommitFact`** — sha → parent shas. **`RefFact`** — project, branch →
  head sha, observed_at. The commit-graph slices that let git-free nodes
  (the hub, remote peers) resolve ancestry. Content-addressed and immutable
  (a sha's parents never change), so they sync like every other event.

### Status resolution

Effective status of a task on a checkout =
fold of its StatusChanges, restricted to those whose `anchor_commit` is an
ancestor of HEAD (unanchored changes and task creation apply everywhere),
ordered by `at` (ties by event id). No qualifying changes ⇒ open.

- Locally: ancestry via `gix` merge-base against the real repo (exact).
- On the hub / for unfetched branches: same fold walking the CommitFact
  graph. Missing facts for an anchor ⇒ status `unknown`, treated as open and
  flagged; `--json` carries `resolution: exact | facts | partial`.

Dependency satisfaction uses the same per-checkout resolution: a blocker
"closed" only counts where its anchor is in your ancestry.

### Anchoring rules

- `levi close ID` anchors at the current worktree's HEAD; `--anchor SHA`
  overrides; `--no-anchor` for tasks unrelated to code state (resolve as
  closed everywhere). Reopen works identically.
- Rebase/cherry-pick moves shas: an anchor no longer reachable from any ref
  makes the task look open on the rewritten history — correct per the model,
  but surprising. The CLI detects orphaned anchors (reflog-only reachability;
  patch-id match against rewritten commits where cheap) and warns, suggesting
  a re-close at the new HEAD. No auto-migration in v1.

## Sync

`levi sync` runs three independent, best-effort legs (each skippable):

1. **Git leg** (`--no-git` to skip): fetch `refs/levi/events` from the
   remote, union-merge, push back.
2. **Hub leg** (`--no-hub`): connect as a myko client, exchange event diffs
   both ways. Content-addressed ids make "what are you missing" a cheap set
   difference; Merkle-style comparison is a later optimization. The hub
   relays events between machines that share no git remote.
3. **Facts leg**: publish fresh CommitFacts/RefFacts — ancestors of anchor
   commits and current branch heads only, depth-capped, deduped against
   what's already published.

Every mutating command attempts an opportunistic background sync on exit
unless `--no-sync` is passed or no hub/remote is configured. Hub address:
`git config levi.hub` (per-repo) overriding `~/.config/levi/config.toml`.

### Hub

`levi-hub`: myko `CellServer`, Postgres event-log persistence
(`.with_postgres`), no git anywhere. Receives task events + graph facts from
CLIs, serves aggregate reactive queries across all projects, and serves the
dashboard's static files on the same origin as the `/myko` WS endpoint (no
CORS). Auth for v1: a shared bearer token per hub (config/env); real
multi-user auth is future work.

## CLI surface

```
levi init                        mint project id, create the ref
levi add "title" [-p p1] [-b body] [-l label]... [--dep ID]...
levi ls [--json] [--all|--closed] [-l label] [--branch X] [--mine]
levi show ID [--json]            detail + comments + deps + claim + history
levi next [--claim] [-n N] [--json]
levi start ID | levi steal ID | levi drop ID
levi close ID [--anchor SHA | --no-anchor] | levi reopen ID
levi dep add BLOCKED --on BLOCKER | levi dep rm BLOCKED --on BLOCKER
levi comment ID "text"
levi edit ID [-p P] [--title T] [-l +label] [-l -label]
levi sync [--no-git] [--no-hub]
levi watch [--json]              live event stream
levi-hub serve                   --bind ADDR; Postgres via env
```

- Every read command supports `--json` with stable, versioned schemas.
- Task ids display as short unique prefixes (`lv-3f2a`), git-style prefix
  matching on input.
- `levi next --claim` is atomic: the claim event is appended (CAS on the ref)
  before the task is printed, so parallel agents on one machine cannot both
  claim it.

### Ranking (`levi next`)

Eligible = open on this checkout ∧ every blocker closed on this checkout ∧
no live claim by someone else. Order:

1. priority (P0 first)
2. transitive unblock count (closing this frees the most work)
3. age (oldest first)

Deterministic and explainable; `--json` includes a `reason` string stating
exactly why the task ranked first.

## Dashboard (`levi-dash`)

Leptos CSR → WASM, served by the hub. Connects with the WASM-capable myko
client (`autosocket`); wraps `watch_query` diff streams in Leptos signals —
fully live, no polling, no REST layer, entities shared from `levi-core`.

V1 pages:

1. **Overview** — projects with open/closed counts, P0 alerts, live activity
   feed.
2. **In flight** — active claims grouped by developer → machine → worktree.
3. **Project browser** — task list with `ls`-equivalent filters plus a
   branch selector (from RefFacts): view any project's tasks as resolved
   against any branch. Task drawer: comments, deps, status history.

## Edge cases

- **Concurrent appends** (parallel agents, one machine): ref CAS + retry.
- **Fresh clone, ref never fetched**: commands run against empty state and
  print how to fetch; `init` refuses if project events already exist.
- **Clock skew**: LWW uses event `created_at`; ties broken by event id, so
  ordering is deterministic on every node.
- **Large repos**: gix commit-graph acceleration where present; fact
  publication incremental and depth-capped.
- **Unknown ancestry** (hub missing facts): resolve as open + flagged, never
  silently closed.

## Licensing note

`myko`/`myko-macros`/`autosocket` are MIT/Apache; `myko-server` is
AGPL-3.0. The hub links `myko-server` and will be AGPL. Whether the CLI
needs `myko-server` (for `CellServer` construction) or can drive
`CellServerCtx` from `myko` core alone determines the CLI's license
obligations — resolve during implementation; if the CLI must link the
server crate, either accept AGPL for the whole tool or split the needed
context machinery into a permissively-licensed layer in myko.

## Testing

- **`levi-core`**: pure unit tests — ranking, LWW/tie-breaking, and the
  ancestry fold over synthetic commit DAGs (resolution is a function of
  (events, ancestor-set); no real git required).
- **CLI integration**: temp git repos driven through scripted multi-branch /
  merge / rebase / worktree scenarios, asserting `--json` output; concurrent
  CAS-retry test with parallel invocations.
- **Sync convergence**: two temp repos + in-process hub; mutate both
  offline, sync via each leg, assert byte-identical materialized state.
- **Dashboard**: core-level query tests only in v1; no WASM e2e yet.
