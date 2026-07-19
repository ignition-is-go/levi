# levi

Git-aware, agent-first, distributed issue tracker. Task status is a function
of git history, storage travels with the repo (`refs/levi/events`), and the
primary consumer is a coding agent asking "what should I work on next?"

A task closed at commit X is closed only where X is an ancestor of HEAD:
feature-branch work shows as done on that branch and still-open on main until
merged. Fully functional offline from a bare clone — no server, no daemon.
Design: `docs/superpowers/specs/2026-07-18-levi-design.md`.

## Build

```sh
cargo build --release            # levi + levi-hub (native)
cd levi-dash && trunk build      # dashboard (wasm; needs trunk + wasm32 target)
```

Depends on a local checkout of the myko framework at `../myko`.

## Quickstart

```sh
cd your-repo
levi init                            # mint the project id, create the ref
levi add "fix flux capacitor" -p p0 -l engine
levi add "polish docs" --dep lv-3f2a # blocked by the first task
levi next                            # highest-priority eligible work + why
levi next --claim                    # …and atomically claim it
git commit -am "the fix"
levi close lv-3f2a                   # anchored at HEAD: closed only where
                                     # this commit is in ancestry
levi ls --json                       # stable schemas on every read command
levi show lv-3f2a                    # detail: deps, claim, comments, history
```

Anchoring: `levi close ID` anchors at HEAD; `--anchor SHA` overrides;
`--no-anchor` closes everywhere (tasks unrelated to code state). `reopen`
works identically. Rebased-away anchors are detected and warned about.

Claims are advisory, keyed to (developer, machine, worktree), newest wins,
expire after 24h (`git config levi.claimTtlSecs` or `[claim] ttl_secs`).
`levi start`/`steal`/`drop` manage them; parallel `levi next --claim` on one
machine never hands two agents the same task.

## Sync

```sh
levi sync            # all legs; --no-git / --no-hub to skip one
```

- **git leg**: `refs/levi/events` is fetched from / pushed to your remote
  (`git config levi.remote`, default `origin`). Histories union-merge;
  events are content-addressed and immutable, so sync never conflicts.
- **hub leg**: with `git config levi.hub <host:port>`, events are exchanged
  with a levi-hub — machines that share no git remote converge through it.
  Every mutating command also does an opportunistic best-effort hub sync
  (`--no-sync` to skip).
- **facts leg**: commit-graph slices (sha → parents, branch heads) are
  published so the git-free hub can resolve ancestry.

Config file fallback: `~/.config/levi/config.toml`
(`[hub] address`, `[sync] remote`, `[claim] ttl_secs`).

## Hub + dashboard

```sh
MYKO_POSTGRES_URL=postgres://… levi-hub serve --bind 0.0.0.0:7377
```

The hub is a plain myko CellServer (`ws://<bind>/myko`); without
`MYKO_POSTGRES_URL` it holds events in memory only. `levi watch` streams
live events from it.

The dashboard is a standalone Leptos CSR app:

```sh
cd levi-dash && trunk serve       # dev, http://localhost:1420
cd levi-dash && trunk build       # dist/ — host the static files anywhere
```

It connects to `ws://<page-hostname>:7377/myko` by default; override with
`?hub=host:port`. Pages: overview (open/closed counts, P0 alerts, live
activity feed), in-flight claims by dev → machine → worktree, and a project
browser with a branch selector: any project's tasks as resolved against any
branch, from commit facts alone.

## Testing

`cargo test --workspace` covers the resolution fold and ranking (pure unit
tests over synthetic DAGs), CLI integration against scripted multi-branch /
merge / rebase / worktree repos, concurrent CAS appends and parallel claims,
and two-repo + in-process-hub sync convergence.
