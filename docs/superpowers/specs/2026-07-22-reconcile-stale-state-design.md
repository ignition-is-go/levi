# Reconciling stale event-log state

*Design spec, 2026-07-22. Status: approved pending review.*

## Problem

levi records events but never reconciles or retires two kinds of stale state,
and both bit us repeatedly in one session:

1. **Squash/rebase merges silently break status resolution.** A close is
   anchored at a commit sha; status resolves by "is that sha an ancestor of
   HEAD." A squash merge (GitHub's most common strategy) replaces the branch
   commit with a *new* one carrying the same changes, so the anchor is never
   an ancestor of main and the task reads **open forever** after the fix has
   shipped. We hit this on lv-1e92, lv-7fa4, and fix/squash-merged-anchors.
   The `squashed_anchors` detection added for lv-1e92 only **warns** ("re-close
   with `--anchor <new>`"); it never affects the resolved status, and it is
   CLI-only.

2. **Claims pile up.** The default claim TTL is 24h, and `close`/`reopen`
   never release the claim — finishing a task leaves its claim dangling until
   it expires, and the Claim event persists in the log forever after. A
   15-task session leaves ~14 expired claims cluttering the in-flight view.

Both are the same shape: the log accumulates state that nothing resolves or
cleans up.

## Goals

- A close survives a squash/rebase/cherry-pick, resolving the task **closed**
  on every surface (CLI, hub, dashboard) — honestly labeled as inferred, not
  masqueraded as exact.
- Finishing a task releases its claim; the in-flight view stops filling with
  expired claims.

## Non-goals

- Log compaction of long-dead claim or fact events (a later follow-up).
- Resolving *ancient* squashes — patch-id facts are bounded to a recent window
  (see Cost).
- Changing the anchor model itself (anchors stay commit-sha; patch-id is a
  *fallback*, not the primary key).

## Part 1 — Squash-resilient resolution

### 1a. Honest resolution vocabulary (`levi-core::resolve`)

`Ancestry` gains a variant:

```rust
pub enum Ancestry { Yes, No, Unknown, Rewritten }
```

`Rewritten` means: the exact anchor sha is not in ancestry, but a commit with
the same patch-id is. `Resolution` gains a matching grade:

```rust
pub enum Resolution { Exact, Facts, Partial, Squashed }  // label: "squashed"
```

`effective_status` treats `Rewritten` as **applies = true**, and downgrades
the run's resolution to `Squashed` (weakest-wins, alongside how `Unknown`
downgrades to `Partial`). So a squash-resolved task reads **closed** but is
visibly `squashed`, never `exact`. This preserves the core guarantee: an
inferred close is always distinguishable from a proven one.

Ordering of grades weakest→strongest for the "keep the weakest seen" fold:
`Partial` < `Squashed` < (`Facts` | `Exact`). A change that resolves exactly
keeps `Exact`; if any relevant change only resolved via patch-id, the task is
`Squashed`; an unknown anchor is still `Partial`.

### 1b. CLI resolver — `GixAncestors` (has the real repo)

When `merge_base(head, anchor)` shows the anchor is **not** an ancestor
(today: `Ancestry::No`), fall back:

1. Compute the anchor's patch-id (`git show <anchor> | git patch-id --stable`).
2. Look it up in a **patch-id → sha map of HEAD's history**, built once per
   `GixAncestors` and memoized (bounded to the recent window, see Cost).
3. Hit → `Ancestry::Rewritten`; miss → `Ancestry::No` as before.

This reuses `ancestors.rs::patch_id` / `recent_patch_ids` (already present for
the warning path). Empty-diff anchors (merge/empty commits) have no patch-id
and never match — correct, they carry no changes to have been squashed.

### 1c. Patch-id facts — the git-free reach (hub + dashboard)

Only a node with the real repo can compute a patch-id (it needs the diff), so
the git-free `FactsAncestors` needs the answer shipped to it.

`CommitFact` gains one field:

```rust
pub struct CommitFact {
    pub project_id: String,
    pub parents: Vec<String>,
    #[serde(default)]
    pub patch_id: Option<String>,   // None = empty-diff / not computed
}
```

The CLI computes `patch_id` when publishing facts (`facts.rs`), for commits in
the recent window (see Cost). `FactsAncestors::contains` then mirrors the CLI
fallback: on an exact miss, take the anchor's `patch_id` (from the anchor's own
CommitFact — anchors are always published as fact roots) and scan the head's
fact-ancestry for a commit whose `patch_id` matches → `Rewritten`.

### 1d. Sync reconciliation (the load-bearing detail)

The new field has to actually *reach* the hub and *update* facts already there.
Verified against the sync path:

- **The sync sees the difference.** A `CommitFact` carrying a `patch_id`
  serializes to different bytes, so its `LogEntry` id (the content address of
  the encoded event) differs from the old `patch_id: None` version. The merkle
  bucket walk compares `LogEntry` ids, so the patch-id-bearing fact registers
  as a *new* event in a differing bucket and transfers — it is not
  dedup-swallowed as "already have that sha."

- **The hub reconciles by LWW.** `ApplyLogEntry`'s guard is keyed on the
  entity item id (`CommitFact.id` = sha) and skips events older than what the
  entity already shows. The republished fact carries a fresh `created_at`, so
  it wins LWW over the stored `patch_id: None` version — the hub's materialized
  `CommitFact` gains the `patch_id`. Same mechanism that already makes RefFact
  updates converge.

- **The client must regenerate it.** The one gap: `facts.rs` suppresses
  re-publishing via the `facts-published` cache keyed on sha, so an
  already-published commit is never re-emitted — and would never gain a
  `patch_id`. **The cache is versioned** (`facts-published` →
  `facts-published-v2`, empty on upgrade) so each client republishes its
  window's facts once, now with patch-ids. Bounded by the patch-id window, so
  the one-time cost is the window size, not full history.

- **Determinism removes the conflict.** A commit's diff has exactly one
  patch-id, so every client computes the *same* value — patch-id facts never
  genuinely conflict. The only reconciliation wrinkle is a not-yet-upgraded
  client republishing a fact *without* a patch-id and, by a newer timestamp,
  clobbering a `Some` back to `None`. That is **honest degradation, not a wrong
  answer**: the task falls back to reading `open` (its pre-fix behavior) until
  a patch-id-aware client republishes it. Steady state (whole fleet upgraded +
  sha-keyed dedup) has no churn. An optional monotonic guard — the hub never
  overwrites a `Some(patch_id)` with `None` — removes even the transient
  flicker; filed as a small follow-up, not required for correctness.

Backward compatibility: `#[serde(default)]` means old facts on the hub read as
`patch_id: None` and simply don't participate in matching until republished, so
nothing breaks during rollout.

### Cost — the recent-window bound

A patch-id is one `git show | git patch-id` per commit; facts.rs can publish
thousands (myko: ~27k). Computing all of them on first publish is a large
one-time cost. So patch-id computation is **bounded to the most recent
`PATCH_ID_WINDOW` commits per branch head** (default 300, matching the
existing `recent_patch_ids` limit). Rationale: a squash merge is recent, so a
recently-squashed anchor matches a recent commit; an anchor squashed hundreds
of commits ago is both rare and low-value. Commit facts outside the window are
still published (sha→parents, cheap) — they just carry `patch_id: None`. The
window is a config knob (`[facts] patch_id_window`) for repos that want more.

Honest degradation: outside the window, a squashed task stays `open` (as
today) rather than being silently wrong — the same failure mode we have now,
just pushed to the tail.

## Part 2 — Claim lifecycle

### 2a. Release on close

`levi close` and `levi reopen` (`commands/status.rs`), after appending the
StatusChange, check for a live claim held by the caller and append a DEL for
it — exactly what `drop` does. Only the caller's own claim is released (never
someone else's); a foreign or absent claim is a silent no-op. Rationale:
finishing or reopening a task is the natural end of holding it. `--no-drop`
opts out for the rare case of closing a task you want to keep claimed.

### 2b. Hide expired by default

Expired claims are historical noise. Both surfaces default to **live only**:

- **Dashboard in-flight**: the dev-group counts already split live/expired;
  default the list to live claims, with the existing "N expired" affordance
  becoming a toggle to reveal them. An all-expired dev collapses to a one-line
  "N expired (hidden)".
- **CLI**: `levi ls --json` claim info and any claim listing show live claims;
  a `--expired` flag (or `--all`) reveals expired ones.

No event is deleted here — this is display only. Actual log compaction of
long-dead claims is a separate follow-up (filed, not built).

## Error handling

- Patch-id computation shelling out to git can fail (detached/odd states);
  a failure yields `None`/no-match and resolution falls back to `No` — never a
  hard error, never a false `Rewritten`.
- Patch-id collisions are possible (a revert-and-reapply, a cherry-pick landed
  on several branches). A collision produces a `Squashed` (inferred) close, not
  an `Exact` one — the honest label is exactly the mitigation: the status is
  presented as inferred, and a human can verify. We do not auto-mutate the log
  on a patch-id match.

## Testing

`levi-core` unit tests (native):
- `effective_status`: a `Rewritten` anchor resolves `Closed` + `Squashed`; the
  grade fold keeps the weakest (Partial < Squashed < Exact/Facts).
- `FactsAncestors` patch-id fallback: an anchor absent from the graph but whose
  patch-id matches a head-ancestry commit resolves `Rewritten`; no match →
  `No`; anchor with `patch_id: None` never matches.

CLI integration (`cli_git_sync.rs`): the squash scenario from lv-1e92's test —
a real `git merge --squash` — now resolves the task **closed (`squashed`)**
via `levi ls`/`show`, not just a warning. Ordinary unmerged work stays open.

Hub/dashboard path (`cross_project.rs`, in-process hub): publish a task closed
on a feature branch, squash-merge locally, sync facts, and confirm a second
client resolves it closed-`squashed` from facts alone.

Claims: `levi close` releases the caller's live claim (no live claim after);
`--no-drop` keeps it; closing a task claimed by someone else leaves theirs
intact. Dashboard/CLI default-live filtering covered by the in-flight tests.

## Sequencing

Two plans:

1. **Squash resolution** — the `Ancestry`/`Resolution` additions, the CLI
   `GixAncestors` fallback, `CommitFact.patch_id` + publish + `FactsAncestors`
   fallback, the window bound. Delivers squash-resilience end to end.
2. **Claim lifecycle** — release-on-close + hide-expired. Small and
   independent; can land in either order but is specced second.
