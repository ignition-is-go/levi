# Uninitialized-recovery sync for `levi next` and `levi ls`

*Design spec, 2026-07-20. Status: approved pending review.*

## Problem

On a fresh clone, `refs/levi/events` does not exist locally until someone
fetches it by hand (`git fetch <remote> '+refs/levi/events:refs/levi/events'`)
or runs `levi sync`. Until then, `levi next` reports "no levi events here.
Run `levi init` first." — a dead end, and actively bad advice: `levi init`
on a checkout whose remote already has events would mint a second project.
`levi ls` at least prints the manual fetch hint, but still requires a human
to act. For an agent-first tool, the agent's very first command on a fresh
checkout should not fail when the events are one fetch away.

## Goals

- `levi next` and `levi ls` recover automatically when the events ref is
  absent locally but available from the git remote or the hub.
- The normal (initialized) read path stays offline: no network I/O is added
  to any command when events already exist locally.
- `--no-sync` continues to mean "never touch the network": with the flag,
  behavior is exactly today's.

## Non-goals

- Staleness-based or always-on syncing before reads (rejected: reads never
  hit the network by design).
- Recovery in `show`/`status`/mutating commands. On a fresh clone there is
  no task id to name yet, so recovery there adds surface without benefit.
- Recovery inside `LeviCtx::load` (rejected: hidden network I/O on every
  command's load path).

## Design

One new function in `levi/src/sync.rs`:

```rust
/// If the world is uninitialized, try a blocking best-effort sync (git leg,
/// then hub leg) and reload. Returns true when events materialized.
pub fn recover_uninitialized(ctx: &mut LeviCtx) -> bool
```

Behavior:

- Returns `false` immediately when `ctx.no_sync` is set or the world is
  already initialized.
- Runs `git_leg(ctx)` then `hub_leg(ctx)`, each best-effort: any error
  (no remote configured, network down, no hub, push of a nonexistent local
  ref on a truly fresh repo) is reduced to a single stderr note
  (`levi: sync attempt failed: <err>`), never a hard error. Both legs run
  even if the first fails — a hub-only project must still recover.
- Calls `ctx.reload()` and returns `!ctx.uninitialized()`.
- On success, prints a one-line stderr notice so the recovery is visible,
  e.g. `levi: fetched events via sync (repo had none locally)`. stdout stays
  reserved for the command's own (possibly `--json`) output.

Call sites — the existing `uninitialized()` branches in `commands/next.rs`
and `commands/ls.rs` become:

```rust
if ctx.uninitialized() && !crate::sync::recover_uninitialized(ctx) {
    // exactly today's message + early return
}
```

`ls::run` changes its receiver from `&LeviCtx` to `&mut LeviCtx` for the
reload (`next::run` is already `&mut`).

## Error handling

Recovery is strictly best-effort. Every failure mode degrades to today's
behavior (the "no levi events here" message with the manual hint); none
introduce a new error exit. The existing messages are unchanged so scripts
matching on them keep working — except `next`'s message, which gains the
same fetch hint `ls` already prints, since "run `levi init` first" is the
wrong first suggestion when a remote may hold events.

## Testing

In `levi/tests/cli_git_sync.rs`, following the existing two-clone pattern:

1. Remote has events, fresh clone has no events ref → `levi next --json`
   triggers recovery and returns the task.
2. Same setup with `--no-sync` → no recovery; today's empty result and
   stderr message.
3. No remote configured, no hub → today's message, exit code 0, no error.

Existing sync tests (`sync_round_trips_events_between_clones`,
`sync_without_remote_is_graceful`) already cover the legs themselves.
