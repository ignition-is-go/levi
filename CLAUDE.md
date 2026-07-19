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
