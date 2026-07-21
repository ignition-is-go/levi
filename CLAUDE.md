# levi

Git-aware, agent-first, distributed issue tracker. Rust, built on the
[myko](https://github.com/ignition-is-go/myko) framework (crates.io:
`myko`, `myko-server`, `myko-leptos`).

## Status

v1 implemented (branch `levi-impl`) per the approved spec at
`docs/superpowers/specs/2026-07-18-levi-design.md` and the plan at
`docs/superpowers/plans/2026-07-18-levi-implementation.md` (spec deviations
are recorded at the top of the plan). `cargo test --workspace` runs the full
suite; the dashboard builds with `trunk build` in `levi-dash/`.

## Gotchas

- myko dead-strip: a binary that receives levi entities only over the wire
  (the hub) must call `levi_core::link()` or inventory registrations vanish.
- gix ref transactions don't revalidate the expected value after waiting on
  a contended lock; `EventStore` serializes mutations with an flock.
- Entity field naming: myko's `QueryRequest` flattens the query beside its
  own wire keys `tx` and `createdAt`, and `Partial*` serializes `None` as
  explicit `null` — an entity field with a colliding name breaks remote
  `Get*ByQuery` (server parse fails on the null). levi therefore names
  timestamp fields `created`/`observed`, never `created_at`/`tx`. Keep new
  entity fields clear of those keys (myko-side fix would be
  `skip_serializing_if` on Partial fields).

## Conventions

- Never include AI/Claude attribution or references in commit messages or PRs.
- **Conventional commits are required** — `cargo flux version` derives the
  next semver from commit messages, and the release workflow runs on every
  push to main. `feat:` bumps minor, `fix:`/most other types bump patch,
  `feat!:`/`BREAKING CHANGE:` bumps major, `chore(release):` commits are the
  stamps flux itself creates (never write one by hand).

<!-- levi:begin -->
## Task tracking (levi)

This repo tracks tasks with levi, a git-aware issue tracker. State lives in
the repo itself (`refs/levi/events`); status is resolved against git
ancestry, so a task closed at commit X counts as closed only on checkouts
that contain X. Every read command takes `--json` (stable schemas) — prefer
it when parsing.

- **Pick work**: `levi next --claim --json` returns the most important
eligible task, claims it for this dev/machine/worktree (so parallel agents
never grab the same task), and tells you why it ranked first. If you stop
working on a task, release it: `levi drop <id>`.
- **Inspect**: `levi ls --json` (open on this checkout), `levi show <id>
--json` (body, deps, claim, comments, status history).
- **Create**: `levi add "title" [-p p0..p3] [-b body] [-l label]
[--dep <blocker-id>]` — file follow-ups you discover instead of fixing
drive-by; link blockers with `--dep`/`levi dep add`.
- **Complete**: commit the work first, then `levi close <id>` — the close
anchors at HEAD, so it only applies where the fixing commit exists
(feature-branch closes stay open on main until merged; that is correct).
`--no-anchor` is only for tasks unrelated to code state.
- **Reopen** regressions with `levi reopen <id>`; leave context with
`levi comment <id> "text"`.
- Sync is opportunistic after every mutation; `levi sync` forces a full
git-remote + hub exchange.
- **Cross-project**: file upstream bugs with `levi add --project <name>
"title"`; link with `levi dep add <id> --on <project>/lv-xxxx --via
"<how this repo consumes that project>"`. When a foreign blocker
closes, verify the fix is actually reachable through the `via`
mechanism (published release, updated pin, ...) before starting work.
<!-- levi:end -->
