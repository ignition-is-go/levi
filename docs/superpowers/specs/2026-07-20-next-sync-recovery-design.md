# Safe init, merged onboard, and uninitialized-recovery for reads

*Design spec, 2026-07-20. Status: approved pending review.*

## Problem

On a fresh clone, `refs/levi/events` does not exist locally until someone
fetches it by hand (`git fetch <remote> '+refs/levi/events:refs/levi/events'`)
or runs `levi sync`. Until then:

- `levi next` reports "no levi events here. Run `levi init` first." — a dead
  end, and actively dangerous advice: `levi init` mints a brand-new project
  id even when the remote already has one, forking the project.
- `levi ls` prints a manual fetch hint a human has to act on.

For an agent-first tool, the agent's very first command on a fresh checkout
should not fail when the events are one fetch away, and the recovery command
it is pointed at must be safe to run.

Separately, `init` (mint the project) and `onboard` (init if needed + record
hub + write agent instructions) overlap enough that having both is surface
without benefit. One idempotent setup command should do it all.

## Goals

- `levi next` and `levi ls` recover automatically when the events ref is
  absent locally but available from the git remote.
- `levi init` never forks a project: when the remote already has levi
  events, init adopts them instead of minting.
- One setup command: `init` absorbs onboard's behavior; `onboard` is removed
  from the documented surface.
- The normal (initialized) read path stays offline: no network I/O is added
  when events already exist locally.
- `--no-sync` continues to mean "never touch the network".

## Non-goals

- Staleness-based or always-on syncing before reads (rejected: reads never
  hit the network by design).
- Recovery in `show`/`status`/mutating commands: on a fresh clone there is
  no task id to name yet.
- Recovery inside `LeviCtx::load` (rejected: hidden network I/O on every
  command's load path).
- Hub-based recovery of an uninitialized repo. The hub is queried by project
  id, which a fresh clone without events does not know. A hub-only checkout
  with no git remote cannot auto-recover; this is a known limitation.

## Design

### Recovery helper (`levi/src/sync.rs`)

```rust
/// If the world is uninitialized, try a blocking best-effort git-leg sync
/// and reload. Returns true when events materialized.
pub fn recover_uninitialized(ctx: &mut LeviCtx) -> bool
```

- Returns `false` immediately when `ctx.no_sync` is set or the world is
  already initialized.
- Runs `git_leg(ctx)` best-effort: any error (no remote configured, network
  down, push of a nonexistent local ref on a truly fresh repo) is reduced to
  a single stderr note (`levi: sync attempt failed: <err>`), never a hard
  error. The hub leg cannot run uninitialized (no project id — see
  Non-goals); once the git leg lands events, `hub_leg` runs best-effort to
  top up, since the project id is then known.
- Calls `ctx.reload()` and returns `!ctx.uninitialized()`.
- On success, prints a one-line stderr notice so the recovery is visible,
  e.g. `levi: fetched events via sync (repo had none locally)`. stdout stays
  reserved for the command's own (possibly `--json`) output.

### Call sites: `next` and `ls`

The existing `uninitialized()` branches in `commands/next.rs` and
`commands/ls.rs` become:

```rust
if ctx.uninitialized() && !crate::sync::recover_uninitialized(ctx) {
    // today's message + early return
}
```

`ls::run` changes its receiver from `&LeviCtx` to `&mut LeviCtx` for the
reload (`next::run` is already `&mut`). Both commands' dead-end messages
converge on the `ls` wording (init + fetch hint) — safe now that init
adopts instead of forking.

### Safe `init`: adopt before minting

`init` probes the remote before creating anything:
`git ls-remote --exit-code <remote> refs/levi/events`.

| Probe outcome | Action |
| --- | --- |
| no remote configured | mint (today's behavior) |
| ref present (exit 0) | fetch + union-merge via the tracking-ref path `git_leg` uses, reload, report `joined existing levi project '<name>' (<id>)` |
| ref absent (exit 2) | mint |
| remote unreachable | bail: retry when online, or `--no-sync` to init standalone |

- A fetch failure *after* a successful probe is a hard error — never fall
  back to minting when we know events exist.
- `--no-sync` skips the probe entirely and mints (the offline escape hatch;
  the global flag already exists).
- The unreachable-remote bail is what keeps init safe: silently minting when
  we merely could not check would recreate the fork footgun.

### One setup command: `init` absorbs `onboard`

`levi init [--name <n>] [--hub <addr>] [--file <path>...]`:

1. Project: adopt-or-mint per the table above. An already-initialized repo
   no longer bails — init becomes idempotent.
2. Hub: `--hub` records the address in `.levi/config.toml` (onboard's step,
   unchanged).
3. Agent instructions: write/refresh the marker-delimited block in every
   existing CLAUDE.md/AGENTS.md (or a new AGENTS.md), exactly as onboard
   does today, including the corrupted-marker bail. The block's tip text
   changes `levi onboard --hub` → `levi init --hub`.

The `Onboard` subcommand is removed from the CLI; `onboard` remains as a
hidden clap alias of `init` for one release, then drops. `commands/onboard.rs`
merges into `commands/init.rs`.

## Error handling

Recovery in `next`/`ls` is strictly best-effort: every failure mode degrades
to today's behavior (message + hint, exit 0). `init` is the opposite by
design — when it knows events exist (successful probe) or cannot know
(unreachable remote), it refuses to mint rather than guess.

## Testing

In `levi/tests/cli_git_sync.rs`, following the existing two-clone pattern:

1. Remote has events, fresh clone → `levi next --json` recovers and returns
   the task.
2. Same setup with `--no-sync` → no recovery; empty result + message.
3. No remote configured → today's message, exit 0.
4. Remote has events, fresh clone → `levi init` adopts: same project id, no
   new Project event, output says "joined".
5. Remote reachable, no events ref → `levi init` mints (today's behavior).
6. Remote configured but unreachable (URL points at a nonexistent path) →
   `levi init` bails; with `--no-sync` it mints.
7. `levi init` on an initialized repo → idempotent (no bail, instructions
   refreshed).

Existing onboard tests migrate to init; existing sync tests
(`sync_round_trips_events_between_clones`, `sync_without_remote_is_graceful`)
already cover the legs.
