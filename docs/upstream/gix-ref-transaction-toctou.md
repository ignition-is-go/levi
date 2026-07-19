# Draft upstream report: gix ref transactions don't revalidate `expected` after waiting on a contended lock

Status: draft for review — not yet filed. (levi task lv-2091.)
Target: https://github.com/GitoxideLabs/gitoxide (crate `gix`, observed on 0.84/0.85; `gix-ref` 0.65)

## Summary

`Repository::edit_reference` with `PreviousValue::MustExistAndMatch` correctly
rejects a stale expected value when uncontended. Under contention, however,
concurrent CAS-style updates to the same ref can silently clobber each other:
the expected-value check appears to be made against a value read before/while
waiting for the ref lock, and is not revalidated once the lock is acquired.

## Reproduction

8 threads, each with its own `Repository` handle on one repo, each performing
5 read-modify-write cycles on `refs/example/log`:

1. read current target `old` (`try_find_reference` + peel)
2. write a new commit whose parent is `old`
3. `edit_reference(Change::Update { expected: MustExistAndMatch(old), new })`
4. on `Err`, re-read and retry (bounded)

Expected: every cycle either lands or errors; the final chain contains all 40
commits. Observed: **all 40 calls return `Ok`**, but only 13–20 commits
survive (`git rev-list --count` matches the survivors; the rest were
overwritten). Single-threaded and two-handle uncontended runs reject stale
expectations correctly — the loss only appears under lock contention.

Minimal probe (single-process, uncontended — passes; the contended loop
above is where it fails):

```rust
let r1 = gix::discover(dir)?;
let r2 = gix::discover(dir)?;
// r1 creates ref at A; r2 updates A -> B with MustExistAndMatch(A): Ok.
// r1 then attempts A -> C with MustExistAndMatch(A): correctly rejected.
```

## Impact

Any compare-and-swap ref scheme (append-only logs, refs-as-database patterns)
loses updates silently under concurrency. We work around it by serializing
all mutations of the ref behind an `flock` around the read→build→edit cycle.

## Suggested fix

Revalidate `expected` against the just-read on-disk value after the ref lock
is acquired (i.e., perform the read used for validation under the lock), the
way `git update-ref` does.
