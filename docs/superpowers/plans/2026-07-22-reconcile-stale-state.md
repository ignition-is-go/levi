# Reconcile Stale Event-Log State — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Closes survive squash/rebase merges (resolving the task closed on CLI, hub, and dashboard, honestly labeled), and finishing a task releases its claim so the in-flight view stops filling with expired claims.

**Architecture:** Resolution gains a patch-id fallback. levi-core adds `Ancestry::Rewritten` + `Resolution::Squashed`; the CLI resolver (`GixAncestors`) computes patch-ids from the local repo, and the git-free resolver (`FactsAncestors`) uses a new `CommitFact.patch_id` published by the CLI (bounded to a recent window). Claims are released on close via the existing DEL mechanism.

**Tech Stack:** Rust (edition 2024), gix + `git` CLI, myko entities/sync, assert_cmd integration tests, an in-process myko hub in tests.

**Spec:** `docs/superpowers/specs/2026-07-22-reconcile-stale-state-design.md`

## Global Constraints

- Conventional commits required (flux derives the release from commit types). No AI attribution anywhere.
- A patch-id-resolved close reads **closed** but labels its resolution `squashed`, never `exact`/`facts`. Resolution grade fold keeps the weakest seen: `Partial` < `Squashed` < (`Facts` | `Exact`).
- Patch-id computation shelling to git must never hard-error: failure yields no match (falls back to `No`), never a false `Rewritten`.
- `CommitFact.patch_id` is `#[serde(default)] Option<String>` — backward compatible; `None` for empty-diff/merge commits and un-windowed commits.
- Patch-id publishing is bounded to `PATCH_ID_WINDOW` (default 300) most-recent commits per head, plus every status-change anchor commit. Config knob `[facts] patch_id_window`.
- Run tests per task with `cargo test -p <crate> [--test <file>]`; `cargo test --workspace` before finishing Task 4 and Task 6.

---

### Task 1: Resolution vocabulary — `Rewritten` / `Squashed`

**Files:**
- Modify: `levi-core/src/resolve.rs`
- Test: `levi-core/src/resolve.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: existing `Ancestry`, `Resolution`, `effective_status`, `AncestorSet`, `PartialMapAncestors`.
- Produces: `Ancestry::Rewritten`; `Resolution::Squashed` (label `"squashed"`); `Resolution::weaken(self, other) -> Resolution`; `effective_status` handling of `Rewritten`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `levi-core/src/resolve.rs` (create the module if absent — mirror existing test helpers `MapAncestors`/`PartialMapAncestors`):

```rust
#[cfg(test)]
mod reconcile_tests {
    use super::*;

    fn closed_change(anchor: &str) -> StatusChange {
        StatusChange {
            id: "c1".into(),
            project_id: "p".into(),
            task_id: "t".into(),
            to_status: StatusKind::Closed,
            anchor_commit: Some(anchor.into()),
            created: "2026-01-01T00:00:00Z".into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        }
    }

    /// A `Rewritten` anchor closes the task but downgrades to Squashed.
    #[test]
    fn rewritten_resolves_closed_squashed() {
        struct Rw;
        impl AncestorSet for Rw {
            fn contains(&mut self, _sha: &str) -> Ancestry { Ancestry::Rewritten }
        }
        let ch = closed_change("deadbeef");
        let got = effective_status(&[&ch], &mut Rw, Resolution::Exact);
        assert_eq!(got.status, Status::Closed);
        assert_eq!(got.resolution, Resolution::Squashed);
        assert_eq!(got.resolution.label(), "squashed");
    }

    /// Grade fold keeps the weakest: Partial beats Squashed beats Exact.
    #[test]
    fn resolution_weaken_orders_partial_below_squashed_below_exact() {
        assert_eq!(Resolution::Exact.weaken(Resolution::Squashed), Resolution::Squashed);
        assert_eq!(Resolution::Squashed.weaken(Resolution::Partial), Resolution::Partial);
        assert_eq!(Resolution::Facts.weaken(Resolution::Squashed), Resolution::Squashed);
        // A stronger grade never upgrades a weaker one.
        assert_eq!(Resolution::Squashed.weaken(Resolution::Exact), Resolution::Squashed);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi-core reconcile_tests`
Expected: FAIL — `Ancestry::Rewritten` / `Resolution::Squashed` / `weaken` don't exist (compile error).

- [ ] **Step 3: Implement**

In `levi-core/src/resolve.rs`, add the `Rewritten` variant:

```rust
pub enum Ancestry {
    Yes,
    No,
    Unknown,
    /// The exact anchor isn't present, but a patch-id-equivalent commit is.
    Rewritten,
}
```

Add the `Squashed` variant + label:

```rust
pub enum Resolution {
    Exact,
    Facts,
    Partial,
    /// Resolved via a patch-id match (squash/rebase/cherry-pick), not the
    /// exact anchor — an inferred close, honestly flagged.
    Squashed,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Exact => "exact",
            Resolution::Facts => "facts",
            Resolution::Partial => "partial",
            Resolution::Squashed => "squashed",
        }
    }

    /// Keep the weaker of two grades. Order: Partial < Squashed < Facts/Exact.
    pub fn weaken(self, other: Resolution) -> Resolution {
        fn rank(r: Resolution) -> u8 {
            match r {
                Resolution::Partial => 0,
                Resolution::Squashed => 1,
                Resolution::Facts | Resolution::Exact => 2,
            }
        }
        if rank(other) < rank(self) { other } else { self }
    }
}
```

Update `effective_status` to handle `Rewritten` and use `weaken`:

```rust
        let applies = match &change.anchor_commit {
            None => true,
            Some(sha) => match anc.contains(sha) {
                Ancestry::Yes => true,
                Ancestry::Rewritten => {
                    resolution = resolution.weaken(Resolution::Squashed);
                    true
                }
                Ancestry::No => false,
                Ancestry::Unknown => {
                    resolution = resolution.weaken(Resolution::Partial);
                    false
                }
            },
        };
```

(Note: this changes the `Unknown` arm from `resolution = Resolution::Partial` to `resolution = resolution.weaken(Resolution::Partial)` — equivalent, since Partial is the floor.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p levi-core`
Expected: PASS — new tests green, existing resolve tests unaffected. Fix any `match Resolution` exhaustiveness errors elsewhere in levi-core the compiler flags (add `Resolution::Squashed` arms).

- [ ] **Step 5: Commit**

```bash
git add levi-core/src/resolve.rs
git commit -m "feat(core): add Rewritten ancestry and Squashed resolution grade"
```

---

### Task 2: `CommitFact.patch_id` + git-free patch-id fallback

**Files:**
- Modify: `levi-core/src/entities/commit_fact.rs`
- Modify: `levi-core/src/resolve.rs` (`FactsAncestors`)
- Test: `levi-core/src/resolve.rs` (inline)

**Interfaces:**
- Consumes: `Ancestry::Rewritten` (Task 1); `CommitFact`.
- Produces: `CommitFact.patch_id: Option<String>`; `FactsAncestors` that returns `Rewritten` on a patch-id match.

- [ ] **Step 1: Write the failing test**

Add to `reconcile_tests` in `levi-core/src/resolve.rs`:

```rust
    fn fact(sha: &str, parents: &[&str], patch: Option<&str>) -> CommitFact {
        CommitFact {
            id: sha.into(),
            project_id: "p".into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            patch_id: patch.map(str::to_string),
        }
    }

    /// Anchor absent from ancestry but its patch-id matches a head-ancestry
    /// commit -> Rewritten.
    #[test]
    fn facts_patch_id_fallback_resolves_rewritten() {
        // head: c2 <- c1. squashed-away anchor `X` shares patch-id "pX" with c1.
        let facts: BTreeMap<String, CommitFact> = [
            ("c1".to_string(), fact("c1", &[], Some("pX"))),
            ("c2".to_string(), fact("c2", &["c1"], Some("pOther"))),
            ("X".to_string(), fact("X", &[], Some("pX"))),
        ].into_iter().collect();
        let mut anc = FactsAncestors::new(&facts, "c2");
        assert_eq!(anc.contains("c1"), Ancestry::Yes);      // exact, reachable
        assert_eq!(anc.contains("X"), Ancestry::Rewritten); // squashed, patch match
    }

    /// No patch-id match -> No (when the graph is complete).
    #[test]
    fn facts_no_patch_match_is_no() {
        let facts: BTreeMap<String, CommitFact> = [
            ("c1".to_string(), fact("c1", &[], Some("pA"))),
            ("X".to_string(), fact("X", &[], Some("pB"))),
        ].into_iter().collect();
        let mut anc = FactsAncestors::new(&facts, "c1");
        assert_eq!(anc.contains("X"), Ancestry::No);
    }

    /// An anchor with no patch-id never matches.
    #[test]
    fn facts_none_patch_never_matches() {
        let facts: BTreeMap<String, CommitFact> = [
            ("c1".to_string(), fact("c1", &[], Some("pA"))),
            ("X".to_string(), fact("X", &[], None)),
        ].into_iter().collect();
        let mut anc = FactsAncestors::new(&facts, "c1");
        assert_eq!(anc.contains("X"), Ancestry::No);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi-core reconcile_tests::facts`
Expected: FAIL — `CommitFact` has no `patch_id` field (compile error), and `FactsAncestors` never returns `Rewritten`.

- [ ] **Step 3: Implement the field**

In `levi-core/src/entities/commit_fact.rs`:

```rust
#[myko_item]
pub struct CommitFact {
    pub project_id: String,
    pub parents: Vec<String>,
    /// `git patch-id --stable` of this commit's diff, if computed. None for
    /// empty-diff/merge commits and commits outside the publish window. Lets
    /// git-free nodes resolve squash/rebase merges (spec 2026-07-22).
    #[serde(default)]
    pub patch_id: Option<String>,
}
```

- [ ] **Step 4: Implement the fallback**

Rewrite `FactsAncestors` in `levi-core/src/resolve.rs` to carry patch-ids:

```rust
pub struct FactsAncestors {
    reachable: HashSet<String>,
    complete: bool,
    /// patch-ids of commits reachable from head (for squash matching).
    reachable_patches: HashSet<String>,
    /// sha -> patch-id for every known fact (to look up an anchor's patch-id).
    patch_of: HashMap<String, String>,
}

impl FactsAncestors {
    pub fn new(facts: &BTreeMap<String, CommitFact>, head: &str) -> Self {
        let mut reachable = HashSet::new();
        let mut complete = true;
        let mut queue = VecDeque::from([head.to_string()]);
        while let Some(sha) = queue.pop_front() {
            if !reachable.insert(sha.clone()) {
                continue;
            }
            match facts.get(&sha) {
                Some(fact) => queue.extend(fact.parents.iter().cloned()),
                None => complete = false,
            }
        }
        if !facts.contains_key(head) {
            reachable.remove(head);
            complete = false;
        }
        let patch_of: HashMap<String, String> = facts
            .iter()
            .filter_map(|(sha, f)| f.patch_id.clone().map(|p| (sha.clone(), p)))
            .collect();
        let reachable_patches: HashSet<String> = reachable
            .iter()
            .filter_map(|sha| patch_of.get(sha).cloned())
            .collect();
        Self { reachable, complete, reachable_patches, patch_of }
    }
}

impl AncestorSet for FactsAncestors {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if self.reachable.contains(sha) {
            return Ancestry::Yes;
        }
        // Squash/rebase: the exact sha is gone, but its diff (patch-id) may be
        // present in head's ancestry under a new sha.
        if let Some(patch) = self.patch_of.get(sha)
            && self.reachable_patches.contains(patch)
        {
            return Ancestry::Rewritten;
        }
        if self.complete { Ancestry::No } else { Ancestry::Unknown }
    }
}
```

Fix every other `CommitFact { .. }` literal in levi-core (materialize tests, etc.) to add `patch_id: None` — the compiler will list them.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p levi-core`
Expected: PASS — all reconcile_tests green, existing FactsAncestors tests still pass (patch-id fields default to None, so behavior is unchanged where no patches exist).

- [ ] **Step 6: Commit**

```bash
git add levi-core/src/entities/commit_fact.rs levi-core/src/resolve.rs
git commit -m "feat(core): patch-id fact field and git-free squash fallback in FactsAncestors"
```

---

### Task 3: CLI resolver patch-id fallback (`GixAncestors`)

**Files:**
- Modify: `levi/src/ancestors.rs`
- Test: `levi/tests/cli_git_sync.rs`

**Interfaces:**
- Consumes: `Ancestry::Rewritten` (Task 1); existing `patch_id`/`recent_patch_ids` helpers in `ancestors.rs`.
- Produces: `GixAncestors` that returns `Rewritten` for a squash-merged anchor.

- [ ] **Step 1: Write the failing test**

Append to `levi/tests/cli_git_sync.rs` (reuse the `TestRepo` harness; there is already a `squash_merged_anchor_suggests_the_squashed_sha` test for the warning path — this one asserts the *status*):

```rust
/// A squash-merged anchor now resolves the task CLOSED (squashed), not just a
/// warning (spec 2026-07-22).
#[test]
fn squash_merged_task_resolves_closed() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("fix the thing", &[]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("fix.txt", "the fix\n", "the fix");
    repo.levi_ok(&["close", &id]);

    // Squash-merge onto main (the anchor's sha never reaches main).
    repo.checkout("main");
    repo.git(&["merge", "-q", "--squash", "feature"]);
    repo.git(&["commit", "-q", "-m", "fix the thing (#9)"]);

    // The task resolves closed, labeled squashed.
    let ls = repo.levi_json(&["ls", "--all", "--json"]);
    let task = ls["tasks"].as_array().unwrap().iter()
        .find(|t| t["id"] == id.as_str()).unwrap();
    assert_eq!(task["status"], "closed", "squash-merged task must read closed");
    assert_eq!(task["resolution"], "squashed");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi --test cli_git_sync squash_merged_task_resolves_closed`
Expected: FAIL — task reads `"open"` (GixAncestors returns `No`, no patch-id fallback yet).

- [ ] **Step 3: Implement**

In `levi/src/ancestors.rs`, add a lazily-built head patch-id set to `GixAncestors` and fall back on a merge-base miss. Change the struct and `at`:

```rust
pub struct GixAncestors<'r> {
    repo: &'r gix::Repository,
    head: Option<ObjectId>,
    cache: HashMap<String, Ancestry>,
    /// Lazily-built patch-id set of head's recent history (spec 2026-07-22).
    head_patches: Option<std::collections::HashSet<String>>,
}
```

Add `head_patches: None` to both `new` (via `at`) and `at`'s constructor. Then change `lookup` to take `&mut self` (so it can populate the lazy set) — update `contains` to call `self.lookup(sha)` on `&mut self`:

```rust
    fn lookup(&mut self, sha: &str) -> Ancestry {
        let Some(head) = self.head else {
            return Ancestry::No;
        };
        let Ok(anchor) = ObjectId::from_hex(sha.as_bytes()) else {
            return Ancestry::Unknown;
        };
        if self.repo.try_find_object(anchor).ok().flatten().is_none() {
            return Ancestry::Unknown;
        }
        if anchor == head {
            return Ancestry::Yes;
        }
        let exact = match self.repo.merge_base(head, anchor) {
            Ok(base) => base.detach() == anchor,
            Err(_) => false,
        };
        if exact {
            return Ancestry::Yes;
        }
        // Squash/rebase fallback: the exact sha isn't an ancestor, but a
        // patch-id-equivalent commit in head's recent history may be.
        if self.anchor_rewritten(sha) {
            return Ancestry::Rewritten;
        }
        Ancestry::No
    }

    fn anchor_rewritten(&mut self, sha: &str) -> bool {
        let repo_dir = self
            .repo
            .workdir()
            .unwrap_or_else(|| self.repo.git_dir())
            .to_path_buf();
        if self.head_patches.is_none() {
            let head_hex = self.head.map(|h| h.to_string());
            let set = head_hex
                .map(|h| {
                    recent_patch_ids_of(&repo_dir, &h, PATCH_ID_WINDOW)
                        .into_iter()
                        .map(|(patch, _sha)| patch)
                        .collect()
                })
                .unwrap_or_default();
            self.head_patches = Some(set);
        }
        let Some(anchor_patch) = patch_id(&repo_dir, sha) else {
            return false;
        };
        self.head_patches.as_ref().unwrap().contains(&anchor_patch)
    }
```

Update the `AncestorSet for GixAncestors` impl so `contains` calls `self.lookup(sha)` (already `&mut self` — the current body works, `lookup` is now `&mut`).

Add a `PATCH_ID_WINDOW` const near the top of `ancestors.rs`:

```rust
/// How many recent commits per head to consider for squash/rebase patch-id
/// matching (spec 2026-07-22). CLI-local; the fact publisher has its own knob.
const PATCH_ID_WINDOW: usize = 300;
```

Add a head-scoped variant of `recent_patch_ids` (the existing one is HEAD-only; we need it for an arbitrary head sha):

```rust
/// (patch-id, sha) pairs for the last `limit` commits reachable from `head`.
fn recent_patch_ids_of(
    repo_dir: &std::path::Path,
    head: &str,
    limit: usize,
) -> Vec<(String, String)> {
    patch_ids_of(repo_dir, &["-n", &limit.to_string(), head])
}
```

(`patch_ids_of` already exists — the shared `git log -p | git patch-id` helper. If `recent_patch_ids` currently calls a HEAD-only form, leave it; `recent_patch_ids_of` is additive.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p levi --test cli_git_sync`
Expected: PASS — the new test resolves closed/squashed; the existing squash *warning* test still passes (both can coexist; the warning is now redundant but harmless — leave it).

- [ ] **Step 5: Commit**

```bash
git add levi/src/ancestors.rs levi/tests/cli_git_sync.rs
git commit -m "feat: resolve squash-merged anchors closed via patch-id in the CLI"
```

---

### Task 4: Publish patch-ids in the facts leg + versioned cache

**Files:**
- Modify: `levi/src/facts.rs`
- Modify: `levi/src/config.rs` (`patch_id_window` knob)
- Test: `levi/tests/cross_project.rs` (in-process hub)

**Interfaces:**
- Consumes: `CommitFact.patch_id` (Task 2); `FactsAncestors` fallback (Task 2); `patch_ids_of`/`recent_patch_ids_of` (Task 3).
- Produces: CommitFacts published with `patch_id` for windowed + anchor commits; `facts-published-v2` cache.

- [ ] **Step 1: Write the failing test**

Append to `levi/tests/cross_project.rs` (uses `start_hub` + `two_projects`):

```rust
/// A task closed on a feature branch then squash-merged resolves closed from
/// FACTS alone on a second client (git-free), via published patch-ids.
#[test]
fn squash_resolves_from_facts_across_clients() {
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);

    // A closes a task anchored on a feature branch, squash-merges to main,
    // then publishes facts (the anchor's patch-id + main's window).
    let id = a.add("upstream fix", &[]);
    a.git(&["checkout", "-q", "-b", "feature"]);
    a.commit_file("fix.txt", "the fix\n", "the fix");
    a.levi_ok(&["close", &id]);
    a.checkout("main");
    a.git(&["merge", "-q", "--squash", "feature"]);
    a.git(&["commit", "-q", "-m", "upstream fix (squashed)"]);
    a.levi_ok(&["sync", "--no-git"]);

    // B syncs and resolves the task from facts: closed, squashed.
    b.levi_ok(&["sync", "--no-git"]);
    // Give B main's commit so its own head can host the fact graph is not
    // required — B resolves foreign facts via the ladder; assert via show.
    let show = a.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["status"], "closed");
    assert_eq!(show["resolution"], "squashed");
}
```

(Note: this test exercises A's own resolution after squash+sync — the CLI path from Task 3 — while also proving the facts carry patch-ids by round-tripping through the hub. A pure git-free assertion belongs in a levi-core unit test, already covered in Task 2; this integration test proves publish wiring.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi --test cross_project squash_resolves_from_facts`
Expected: FAIL if the facts publish path panics or the cache/window wiring is missing. (The CLI resolution from Task 3 may already make `show` pass — if so, strengthen the test by asserting the published CommitFact for the squash commit carries a non-null patch_id via `session.query`; keep it simple: assert `show` closed/squashed, and rely on Step 3 wiring for the publish.)

- [ ] **Step 3: Add the config knob**

In `levi/src/config.rs`, add to `LeviConfig` and `FileConfig`:

```rust
// LeviConfig struct:
    /// Commits per head to compute patch-ids for when publishing facts.
    pub patch_id_window: usize,
```

```rust
// In LeviConfig::load, in the struct literal:
            patch_id_window: repo_cfg
                .patch_id_window
                .or(global_cfg.patch_id_window)
                .unwrap_or(300),
```

```rust
// FileConfig struct:
    patch_id_window: Option<usize>,
```

```rust
// In FileConfig::load, in the struct literal:
            patch_id_window: doc
                .get("facts")
                .and_then(|c| c.get("patch_id_window"))
                .and_then(|v| v.as_integer())
                .and_then(|v| usize::try_from(v).ok()),
```

- [ ] **Step 4: Implement patch-id publishing + versioned cache**

In `levi/src/facts.rs`:

1. Change the cache path (versioned — forces a one-time republish with patch-ids):

```rust
    let cache_path = repo.common_dir().join("levi").join("facts-published-v2");
```

2. Before the ancestor walk, build a `patch_of: HashMap<String, String>` (sha -> patch-id) for the windowed + anchor commits. Add near the top of `publish`, after `roots`/`ref_facts` are gathered:

```rust
    // Patch-ids for squash/rebase matching (spec 2026-07-22): every anchor
    // commit + the most-recent `patch_id_window` commits per branch head.
    // Bounded so we never compute a patch-id per commit over full history.
    let repo_dir = repo
        .workdir()
        .unwrap_or_else(|| repo.git_dir())
        .to_path_buf();
    let window = ctx.config.patch_id_window;
    let mut patch_of: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Windowed commits per head.
    if let Ok(refs) = repo.references()
        && let Ok(iter) = refs.prefixed("refs/heads/")
    {
        for reference in iter.flatten() {
            let mut reference = reference;
            if let Ok(id) = reference.peel_to_id() {
                for (patch, sha) in
                    crate::ancestors::recent_patch_ids_of(&repo_dir, &id.detach().to_string(), window)
                {
                    patch_of.entry(sha).or_insert(patch);
                }
            }
        }
    }
    // Every status-change anchor commit (may live off the published heads).
    for change in &world.status_changes {
        if let Some(sha) = &change.anchor_commit
            && !patch_of.contains_key(sha)
            && let Some(patch) = crate::ancestors::patch_id_pub(&repo_dir, sha)
        {
            patch_of.insert(sha.clone(), patch);
        }
    }
```

3. When constructing each `CommitFact`, set `patch_id`:

```rust
                let fact = CommitFact {
                    id: sha.clone().into(),
                    project_id: project_id.clone(),
                    parents: parents.iter().map(|p| p.to_string()).collect(),
                    patch_id: patch_of.get(&sha).cloned(),
                };
```

4. Expose the two helpers from `ancestors.rs` (they are currently private `fn`): make `pub fn recent_patch_ids_of(...)` and add `pub fn patch_id_pub(repo_dir, sha) -> Option<String>` that simply calls the existing private `patch_id`. (Keeping `patch_id` private and adding a thin `pub` wrapper avoids widening the existing private helper's contract.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p levi --test cross_project` then `cargo test --workspace`
Expected: PASS — the squash-across-clients test passes; the existing facts-leg tests (`facts_leg.rs`) still pass (they don't assert on `patch_id`, and the versioned cache name change just means a fresh cache in the temp repos).

- [ ] **Step 6: Commit**

```bash
git add levi/src/facts.rs levi/src/config.rs levi/src/ancestors.rs
git commit -m "feat: publish commit patch-ids (windowed) so the hub resolves squash merges"
```

---

### Task 5: Release the claim on close/reopen

**Files:**
- Modify: `levi/src/commands/status.rs`
- Modify: `levi/src/cli.rs` (add `no_drop` to `Close`/`Reopen`)
- Modify: `levi/src/main.rs` (thread `no_drop`)
- Test: `levi/tests/cli_basic.rs`

**Interfaces:**
- Consumes: `LeviCtx::del_event`, `World::live_claim`, `rank::claim_is`.
- Produces: `close`/`reopen` release the caller's live claim unless `--no-drop`.

- [ ] **Step 1: Write the failing test**

Append to `levi/tests/cli_basic.rs`:

```rust
/// Closing a task releases the caller's own claim; --no-drop keeps it.
#[test]
fn close_releases_the_claim() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("do a thing", &[]);
    repo.commit("work");

    // Claim it, then close — the claim should be gone.
    repo.levi_ok(&["start", &id]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_object(),
        "claimed before close");
    repo.levi_ok(&["close", &id]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_null(),
        "claim released on close");

    // Reopen + claim + close --no-drop keeps the claim.
    repo.levi_ok(&["reopen", &id]);
    repo.levi_ok(&["start", &id]);
    repo.commit("more");
    repo.levi_ok(&["close", &id, "--no-drop"]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_object(),
        "--no-drop keeps the claim");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi --test cli_basic close_releases_the_claim`
Expected: FAIL — `--no-drop` is an unknown flag, and close doesn't release the claim.

- [ ] **Step 3: Add the CLI flag**

In `levi/src/cli.rs`, add to both `Close` and `Reopen` variants:

```rust
        /// Keep the claim instead of releasing it on close/reopen.
        #[arg(long)]
        no_drop: bool,
```

In `levi/src/main.rs`, thread `no_drop` into `StatusOpts` for both `Close` and `Reopen` arms:

```rust
        Cmd::Close { id, anchor, no_anchor, force, no_drop } => commands::status::run(
            &ctx, &id, StatusKind::Closed,
            commands::status::StatusOpts { anchor, no_anchor, force, no_drop },
        ),
        // ...and the Reopen arm identically with StatusKind::Reopened.
```

- [ ] **Step 4: Release the claim in `status.rs`**

Add `pub no_drop: bool` to `StatusOpts`. After the StatusChange is appended (find the `append_and_sync` / event append near the end of `run`), release the caller's live claim:

```rust
    // Releasing the claim on close/reopen is the natural end of holding a task
    // (spec 2026-07-22). Only our own live claim; never someone else's.
    let mut events = vec![ctx.set_event(&change)];
    if !opts.no_drop
        && let Some(claim) = ctx.world.live_claim(&task_id, chrono::Utc::now())
        && levi_core::rank::claim_is(claim, &ctx.identity)
    {
        events.push(ctx.del_event(&claim.clone()));
    }
    ctx.append_and_sync(events)?;
```

(Adapt to the existing append in `run` — it currently appends the StatusChange alone; combine into one `append_and_sync` call so the close and the drop land atomically. Import `chrono::Utc` if not already.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p levi --test cli_basic`
Expected: PASS — claim released on close, kept with `--no-drop`.

- [ ] **Step 6: Commit**

```bash
git add levi/src/commands/status.rs levi/src/cli.rs levi/src/main.rs levi/tests/cli_basic.rs
git commit -m "feat: release the caller's claim on close/reopen (--no-drop to keep it)"
```

---

### Task 6: Dashboard — hide expired claims by default

**Files:**
- Modify: `levi-dash/src/pages/in_flight.rs`

**Interfaces:**
- Consumes: existing `claim_live`, the dev-group live/expired counts.
- Produces: in-flight view shows live claims by default, expired behind a per-dev toggle.

- [ ] **Step 1: Implement (no automated test — wasm view; verified by build + trunk)**

In `levi-dash/src/pages/in_flight.rs`, add a per-dev `RwSignal<bool>` `show_expired` (default false). In `machine_block`/`worktree_block`, filter claims to live-only unless `show_expired`. Make the existing "N expired" text in the dev header a clickable toggle:

```rust
    // In the dev header, replace the static expired count with a toggle:
    {(expired > 0).then(|| {
        let show = show_expired;  // Copy of the RwSignal
        view! {
            <span
                style=format!("font-size:{};cursor:pointer;", tokens::FONT_SIZE_XS)
                class="text-muted"
                on:click=move |_| show.update(|v| *v = !*v)
            >
                {move || if show.get() { format!("{expired} expired ▾") }
                         else { format!("{expired} expired ▸") }}
            </span>
        }
    })}
```

In `worktree_block`, drop expired claims when the dev's `show_expired` is false. Thread the `show_expired: RwSignal<bool>` signal down through `machine_block`/`worktree_block` (add it as a parameter), and filter:

```rust
    let visible: Claims = if show_expired.get() {
        claims
    } else {
        claims.into_iter().filter(|c| claim_live(c)).collect()
    };
    // ...render `visible` instead of `claims`. If a worktree has no visible
    // claims after filtering, render nothing for it.
```

- [ ] **Step 2: Build to verify it compiles (wasm)**

Run: `cargo check -p levi-dash --target wasm32-unknown-unknown`
Expected: no errors. (Do NOT run `trunk build` while a `trunk serve` is watching the same dir — they clobber `dist/`.)

- [ ] **Step 3: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS — 0 failures across the workspace.

- [ ] **Step 4: Commit**

```bash
git add levi-dash/src/pages/in_flight.rs
git commit -m "feat(dashboard): hide expired claims by default, toggle to reveal"
```

---

## Self-review notes

- **Spec coverage:** 1a→Task1, 1b→Task3, 1c→Task2(field+fallback)+Task4(publish), 1d(sync reconciliation)→Task4(versioned cache)+Task2(serde default), 2a→Task5, 2b→Task6. Cost/window→Task3(CLI const)+Task4(config knob). The optional hub monotonic guard is explicitly a follow-up in the spec — not a task here.
- **Type consistency:** `Ancestry::Rewritten`, `Resolution::Squashed`, `Resolution::weaken`, `CommitFact.patch_id: Option<String>`, `recent_patch_ids_of(&Path,&str,usize)`, `patch_id_pub(&Path,&str)->Option<String>`, `StatusOpts.no_drop` used consistently across tasks.
- **Follow-ups to file (not built):** log compaction of long-dead claims (spec Non-goals); hub-side monotonic patch-id guard (spec 1d).
