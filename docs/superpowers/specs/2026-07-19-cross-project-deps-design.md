# levi — cross-project dependencies and upstream bug filing

*Design spec, 2026-07-19. Status: approved pending review. Task: lv-3ab3.*

Agents working in one repo need to file bugs against sibling projects they
depend on, and block local tasks on those foreign tasks — with status
resolved against the code they actually consume.

## Problem

Dependencies are same-project by construction: `dep add` resolves both ids
against the local world (one project per events ref), and a foreign
blocker's anchors can't resolve against the local commit graph. The
archetype: an agent in levi hits a myko bug (lv-ee52's `Partial`
serialization). Today it can only file a levi-local task about someone
else's code. It should file the bug *in myko's project*, link the local
task as blocked by it, and have eligibility resolve automatically when the
myko fix becomes consumable.

## Goals

- **File bugs upstream**: create tasks (and comments) in another project
  from any repo, via the hub.
- **Cross-project blocking**: `Dependency` may name a blocker in another
  project; eligibility resolves against how that project is consumed.
- **Consumption-aware resolution**: a sibling checkout on this machine wins
  (exact, offline); otherwise the foreign default branch via hub facts;
  otherwise unknown-means-blocked. `next`/`ls` never wait on the network.
- **Agent-carried topology**: a free-text `via` annotation on each
  cross-project dep records *how* the dependency is consumed; agents write
  it and verify against it. levi never parses it.
- **Machine identity that is actually unique** (folded in): claims key on a
  minted machine id, not the hostname.

## Non-goals (this iteration)

- Foreign close/reopen/edit/claim — create + comment only. Changing a
  foreign task's lifecycle means you're working in that repo; do it there,
  where anchoring works.
- Cross-project dependency *cycle* detection (needs a global view; ranking
  is already cycle-safe).
- Language-aware inference of consumption (cargo/npm parsing) — that's what
  `via` + agents are for.
- Mirroring foreign event logs locally (the sibling-checkout path reads the
  sibling's own ref; no second copy).

## Identity: minted machine id

`~/.local/state/levi/machine-id` holds a UUID minted on first use. It is
the machine's identity in claims and event `source_id`; the hostname
remains a display label only. Rationale: hostnames collide (default laptop
names, cloud images, container fleets sharing hostname *and* worktree
path), which silently breaks claim ownership — two agents each believing a
claim is "mine". A container gets a fresh id per container: correct, since
each is a distinct claim-holder whose claims age out by ttl.

- `Claim` gains `machine_id: String` (`#[serde(default)]`); `machine`
  stays the display name. Claim identity comparisons (mine-ness in `next
  --claim`, `start`, `drop`, `--mine`) use `(dev, machine_id, worktree)`,
  falling back to the display name when `machine_id` is empty (legacy
  events).
- Dashboards keep grouping/showing the friendly name.

## Data model

- **`Dependency`** gains (all `#[serde(default)]`, absent = same-project,
  old CLIs skip deps whose blocker isn't in their world — graceful):
  - `blocker_project_id: Option<String>` — the immortal project id.
  - `blocker_ref: Option<String>` — consumption branch override; `None`
    means the foreign default branch.
  - `via: Option<String>` — agent-authored consumption annotation, e.g.
    `"cargo: crates.io myko ^4.24 in release; path dep ../myko in dev"`.
- The dep event lives in the **blocked task's project log only**. Nothing
  is written to the foreign project for a dep.
- **Machine checkout registry** (state, never config, never synced):
  `~/.local/state/levi/checkouts.toml` maps `project_id` → list of
  `{ path, last_used }`. Every levi invocation opportunistically upserts
  its own (project, worktree). Reads verify the path still exists (prune
  stale) and prefer most-recently-used.
- **Foreign-status cache** (per repo): `.git/levi/foreign-status.toml`
  maps `(project_id, task_id)` → `{ status, resolution, observed_at,
  title }`. Written only by sync (below); read by `ls`/`next`/`show`.

## Write path: `add --project` and foreign `comment`

```
levi add --project myko "Partial serializes None as explicit null" -p p2
  → myko/lv-7c21
levi comment myko/lv-7c21 "repro from levi's side: ..."
levi dep add lv-ee52 --on myko/lv-7c21 --via "cargo: crates.io myko ^4.24"
```

- Project names resolve to ids through the hub's project registry
  (`GetAllProjects`). An ambiguous name is a hard error listing candidate
  ids; `--project <id>` always works. Foreign task ids resolve by prefix
  against the hub's tasks for that project. `dep add --on` with a foreign
  target therefore also needs the hub — unless given
  `<project-id>/<full-task-id>` verbatim, which resolves nothing and works
  offline (the dep event itself is local).
- The event is built exactly as a local one but with the foreign
  `project_id`, wrapped as a LogEntry, and pushed with the existing
  send-then-verify-by-id pattern. **Hub-ack-then-forget**: on verified ack
  the CLI prints `‹project›/lv-xxxx` and stores nothing locally. The hub
  is the authoritative first home of the event until any checkout of the
  foreign project syncs and pulls it into its real ref (the existing pull
  leg, unchanged). The hub itself needs zero changes.
- No hub configured ⇒ clear error: cross-project operations need a hub.
  Durability expectation: a real hub runs Postgres; an in-memory dev hub
  carries the caveat.

## Resolution ladder (foreign blocker status)

For eligibility and display, in order:

1. **Sibling checkout** — the registry knows a live path for
   `blocker_project_id`: open that repo, read *its* `refs/levi/events`,
   resolve the blocker against *its current HEAD* (the code this machine
   actually consumes, path-dep style). `resolution: exact`. Offline.
2. **Hub facts, cached** — resolve via `FactsAncestors` over the foreign
   project's CommitFacts at the head of `blocker_ref` (or the default
   branch: RefFact `main`, else the project's most recently observed
   RefFact). Performed
   **during sync**, persisted to the foreign-status cache with
   `observed_at`. `resolution: facts`.
3. **Unknown** — no checkout, no cached answer: treated open (blocked),
   `resolution: partial`. Never silently unblocked.

`ls`/`next` consult only the ladder's local sources (sibling repo, cache)
— never the network. The cache refreshes on every sync leg, including the
detached background sync fired by every mutation, so answers are at worst
one-mutation stale and say so (`observed_at`).

Unanchored foreign closes (e.g. wontfix decisions) apply under every rung.

## Ranking and display

- Eligibility: a dep with `blocker_project_id` resolves through the ladder
  instead of `world.tasks`. Everything else in `rank_next` is unchanged.
- `show` / `ls --json`: foreign blockers render as
  `myko/lv-7c21 — <cached title> [open, facts, as of 2h ago] via: …`.
- `next` reason, the agent's decision point:
  - while blocked: `blocked by myko/lv-7c21 (open on their main as of 2h
    ago); via: cargo crates.io ^4.24`.
  - once unblocked: `myko/lv-7c21 closed on their main — verify
    availability via: cargo crates.io ^4.24` (closed-on-main may still
    mean "await the publish"; the agent has the exact string to check).
- **Onboard instructions** gain: record `--via` when adding cross-project
  deps; when a foreign blocker closes, verify the fix is reachable through
  the `via` mechanism before starting work.
- Dashboard: it already holds every project's entities; resolve
  cross-project deps live with `FactsAncestors` (no cache), render the
  foreign link + via in the task drawer.

## Edge cases

- Foreign project unknown to hub ⇒ `add --project` fails cleanly.
- Sibling checkout exists but has no events ref ⇒ fall through to rung 2.
- Registry path vanished ⇒ prune, fall through.
- Foreign task deleted ⇒ cache reports unknown; dep displayed with raw id.
- Two projects share a name on the hub ⇒ id required at `dep add`/`add
  --project` time; stored deps are immune (they hold ids).
- Old CLI meets new events ⇒ unknown fields ignored; cross-project deps
  invisible to it (blocker not in world ⇒ dep skipped), same-project
  behavior intact.

## Testing

- **Unit (levi-core)**: ladder resolution over synthetic inputs; dep
  eligibility with `blocker_project_id`; legacy-claim mine-ness fallback.
- **CLI integration (two repos + in-process hub)**: file foreign task from
  A, appears in B after B's sync; dep blocks A's task through the facts
  path until B closes-and-lands on its main; unblock reason carries `via`;
  ambiguous project name errors; no-hub error.
- **Sibling-checkout path (two local repos, no hub)**: registry knows B;
  A's dep resolves exact against B's HEAD; B checks out an older branch ⇒
  A's dep flips back to blocked.
- **Cache/staleness**: `next` with hub down uses cached answer and shows
  `observed_at`; cold cache ⇒ partial/blocked.
- **Machine id**: two identities sharing hostname+worktree but different
  ids don't see each other's claims as their own.
