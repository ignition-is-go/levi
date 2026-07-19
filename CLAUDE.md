# levi

Git-aware, agent-first, distributed issue tracker. Rust, built on the myko
framework (local checkout: `~/Code/myko`; idiomatic consumer example:
`~/Code/rship/apps/asset_store`).

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
- myko `Get*ByQuery` live subscriptions don't match server-side (myko 4.24);
  use `GetAll*` + a `Count*` report as the arrival marker, filter client-side.

## Conventions

- Never include AI/Claude attribution or references in commit messages or PRs.
