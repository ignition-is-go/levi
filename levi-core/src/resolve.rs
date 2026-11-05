//! Per-checkout status resolution (spec §Status resolution).
//!
//! Effective status of a task on a checkout = fold of its StatusChanges,
//! restricted to those whose `anchor_commit` is an ancestor of HEAD
//! (unanchored changes apply everywhere), ordered by (at, id). A change whose
//! anchor's ancestry is *unknown* (missing commit facts / unfetched sha) is
//! skipped — treated open — and downgrades the resolution to `Partial` so the
//! caller can flag the task. No qualifying changes ⇒ open.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::entities::{CommitFact, Review, ReviewVerdict, StatusChange, StatusKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ancestry {
    Yes,
    No,
    Unknown,
    /// The exact anchor isn't present, but a patch-id-equivalent commit is.
    Rewritten,
}

/// Answers "is `sha` an ancestor of (or equal to) this checkout's head?".
/// `&mut` so implementations can memoize.
pub trait AncestorSet {
    fn contains(&mut self, sha: &str) -> Ancestry;
}

impl<A: AncestorSet + ?Sized> AncestorSet for &mut A {
    fn contains(&mut self, sha: &str) -> Ancestry {
        (**self).contains(sha)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Status {
    Open,
    Closed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Ancestry answered by the real repo (gix merge-base).
    Exact,
    /// Ancestry answered by walking the CommitFact graph.
    Facts,
    /// At least one relevant anchor could not be resolved; unknown changes
    /// were treated as open and the task should be flagged.
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
        if rank(other) < rank(self) {
            other
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedStatus {
    pub status: Status,
    pub resolution: Resolution,
}

/// Fold `changes` (must already be in (at, id) order, as produced by
/// `World::changes_for`) against an ancestor set. `base` is `Exact` or
/// `Facts` depending on where `anc` gets its answers.
pub fn effective_status(
    changes: &[&StatusChange],
    anc: &mut impl AncestorSet,
    base: Resolution,
) -> ResolvedStatus {
    let mut status = Status::Open;
    let mut resolution = base;
    for change in changes {
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
        if applies {
            status = match change.to_status {
                StatusKind::Closed => Status::Closed,
                StatusKind::Reopened => Status::Open,
            };
        }
    }
    ResolvedStatus { status, resolution }
}

/// The anchor of the change that leaves this task closed here: the newest
/// applying change, when it is an anchored close. `None` when the task is
/// open here, or was closed with `--no-anchor`.
pub fn closing_anchor<'a>(
    changes: &[&'a StatusChange],
    anc: &mut impl AncestorSet,
) -> Option<&'a str> {
    let mut last: Option<&'a StatusChange> = None;
    for change in changes {
        let applies = match &change.anchor_commit {
            None => true,
            Some(sha) => matches!(anc.contains(sha), Ancestry::Yes | Ancestry::Rewritten),
        };
        if applies {
            last = Some(change);
        }
    }
    let change = last?;
    match change.to_status {
        StatusKind::Closed => change.anchor_commit.as_deref(),
        StatusKind::Reopened => None,
    }
}

/// Reviews whose commit is an ancestor of HEAD, in the order given (which
/// must be (at, id), as `World::reviews_for` returns), each with how its
/// ancestry was answered. Graded like `effective_status`: a patch-id match
/// applies and weakens to `Squashed`; an unknown commit is skipped and
/// weakens to `Partial`, so a review is never silently applied or dropped.
pub fn applying_reviews<'a>(
    reviews: &[&'a Review],
    anc: &mut impl AncestorSet,
    base: Resolution,
) -> (Vec<(&'a Review, Ancestry)>, Resolution) {
    let mut applying = Vec::new();
    let mut resolution = base;
    for review in reviews {
        match anc.contains(&review.commit) {
            a @ Ancestry::Yes => applying.push((*review, a)),
            a @ Ancestry::Rewritten => {
                resolution = resolution.weaken(Resolution::Squashed);
                applying.push((*review, a));
            }
            Ancestry::No => {}
            Ancestry::Unknown => resolution = resolution.weaken(Resolution::Partial),
        }
    }
    (applying, resolution)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewState {
    Verified,
    SentBack,
    None,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            ReviewState::Verified => "verified",
            ReviewState::SentBack => "sent_back",
            ReviewState::None => "none",
        }
    }
}

/// The newest applying review decides. `applying` is as `applying_reviews`
/// returns it.
pub fn review_state(applying: &[(&Review, Ancestry)]) -> ReviewState {
    match applying.last() {
        None => ReviewState::None,
        Some((r, _)) => match r.verdict {
            ReviewVerdict::Verified => ReviewState::Verified,
            ReviewVerdict::SentBack => ReviewState::SentBack,
        },
    }
}

/// The task is closed here at `closed`, but the
/// newest applying verification read a different commit, so what was
/// checked isn't exactly what closed it. Worked out from git alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewDiffers {
    pub verified: String,
    pub closed: String,
}

/// `closed` is `closing_anchor` for this checkout. `None` when the task
/// isn't closed here, has no applying verification, or they agree.
pub fn review_differs(
    closed: Option<&str>,
    applying: &[(&Review, Ancestry)],
) -> Option<ReviewDiffers> {
    let closed = closed?;
    let (verified, _) = applying
        .iter()
        .rev()
        .find(|(r, _)| r.verdict == ReviewVerdict::Verified)?;
    (verified.commit != closed).then(|| ReviewDiffers {
        verified: verified.commit.clone(),
        closed: closed.to_string(),
    })
}

/// Ancestor set over the CommitFact graph, for git-free nodes (the hub, the
/// dashboard's branch selector). BFS closure from `head`; if the walk ever
/// needs a sha with no fact, the closure is incomplete and non-members answer
/// `Unknown` instead of `No` (spec: never silently closed — and never
/// silently *not*-closed either).
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
        // `head` itself only counts as reachable if we actually know it.
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
        Self {
            reachable,
            complete,
            reachable_patches,
            patch_of,
        }
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
        if self.complete {
            Ancestry::No
        } else {
            Ancestry::Unknown
        }
    }
}

/// Test helper: membership map, `Yes` iff contained, else `No`.
pub struct MapAncestors(pub HashSet<String>);

impl MapAncestors {
    pub fn of<const N: usize>(shas: [&str; N]) -> Self {
        Self(shas.iter().map(|s| s.to_string()).collect())
    }
}

impl AncestorSet for MapAncestors {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if self.0.contains(sha) {
            Ancestry::Yes
        } else {
            Ancestry::No
        }
    }
}

/// Test helper: like MapAncestors but with an explicit unknown set.
pub struct PartialMapAncestors {
    pub yes: HashSet<String>,
    pub unknown: HashSet<String>,
}

impl AncestorSet for PartialMapAncestors {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if self.yes.contains(sha) {
            Ancestry::Yes
        } else if self.unknown.contains(sha) {
            Ancestry::Unknown
        } else {
            Ancestry::No
        }
    }
}

/// Memoizing wrapper so repeated anchors don't repeat expensive lookups.
pub struct CachedAncestors<A: AncestorSet> {
    inner: A,
    cache: HashMap<String, Ancestry>,
}

impl<A: AncestorSet> CachedAncestors<A> {
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            cache: HashMap::new(),
        }
    }
}

impl<A: AncestorSet> AncestorSet for CachedAncestors<A> {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if let Some(a) = self.cache.get(sha) {
            return *a;
        }
        let a = self.inner.contains(sha);
        self.cache.insert(sha.to_string(), a);
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(id: &str, kind: StatusKind, anchor: Option<&str>, at: &str) -> StatusChange {
        StatusChange {
            id: id.into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            to_status: kind,
            anchor_commit: anchor.map(str::to_string),
            created: at.into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        }
    }

    fn resolve(changes: &[StatusChange], anc: &mut impl AncestorSet) -> ResolvedStatus {
        let mut refs: Vec<&StatusChange> = changes.iter().collect();
        refs.sort_by(|a, b| (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0)));
        effective_status(&refs, anc, Resolution::Exact)
    }

    #[test]
    fn no_changes_means_open() {
        let r = resolve(&[], &mut MapAncestors::of([]));
        assert_eq!(
            r,
            ResolvedStatus {
                status: Status::Open,
                resolution: Resolution::Exact
            }
        );
    }

    #[test]
    fn close_anchored_on_ancestor_closes_here_only() {
        let cs = [change(
            "a",
            StatusKind::Closed,
            Some("sha1"),
            "2026-07-01T00:00:00Z",
        )];
        // On a checkout containing sha1:
        assert_eq!(
            resolve(&cs, &mut MapAncestors::of(["sha1"])).status,
            Status::Closed
        );
        // On a branch that doesn't contain it:
        assert_eq!(resolve(&cs, &mut MapAncestors::of([])).status, Status::Open);
    }

    #[test]
    fn close_then_reopen_both_in_ancestry() {
        let cs = [
            change(
                "a",
                StatusKind::Closed,
                Some("sha1"),
                "2026-07-01T00:00:00Z",
            ),
            change(
                "b",
                StatusKind::Reopened,
                Some("sha2"),
                "2026-07-02T00:00:00Z",
            ),
        ];
        assert_eq!(
            resolve(&cs, &mut MapAncestors::of(["sha1", "sha2"])).status,
            Status::Open
        );
        // Reopen anchored elsewhere: still closed here.
        assert_eq!(
            resolve(&cs, &mut MapAncestors::of(["sha1"])).status,
            Status::Closed
        );
    }

    #[test]
    fn unanchored_changes_apply_everywhere() {
        let closed_everywhere = [change(
            "a",
            StatusKind::Closed,
            None,
            "2026-07-01T00:00:00Z",
        )];
        assert_eq!(
            resolve(&closed_everywhere, &mut MapAncestors::of([])).status,
            Status::Closed
        );

        let reopened_everywhere = [
            change(
                "a",
                StatusKind::Closed,
                Some("sha1"),
                "2026-07-01T00:00:00Z",
            ),
            change("b", StatusKind::Reopened, None, "2026-07-02T00:00:00Z"),
        ];
        assert_eq!(
            resolve(&reopened_everywhere, &mut MapAncestors::of(["sha1"])).status,
            Status::Open
        );
    }

    #[test]
    fn lww_tie_on_at_broken_by_change_id() {
        let at = "2026-07-01T00:00:00Z";
        let cs = [
            change("zz", StatusKind::Closed, None, at),
            change("aa", StatusKind::Reopened, None, at),
        ];
        // "zz" sorts after "aa", so the close wins.
        assert_eq!(
            resolve(&cs, &mut MapAncestors::of([])).status,
            Status::Closed
        );
    }

    #[test]
    fn unknown_anchor_treated_open_and_flagged() {
        let cs = [change(
            "a",
            StatusKind::Closed,
            Some("ghost"),
            "2026-07-01T00:00:00Z",
        )];
        let mut anc = PartialMapAncestors {
            yes: HashSet::new(),
            unknown: HashSet::from(["ghost".to_string()]),
        };
        let r = resolve(&cs, &mut anc);
        assert_eq!(r.status, Status::Open);
        assert_eq!(r.resolution, Resolution::Partial);
    }

    #[test]
    fn unknown_reopen_with_later_solid_close_stays_closed_but_partial() {
        let cs = [
            change(
                "a",
                StatusKind::Reopened,
                Some("ghost"),
                "2026-07-01T00:00:00Z",
            ),
            change(
                "b",
                StatusKind::Closed,
                Some("sha1"),
                "2026-07-02T00:00:00Z",
            ),
        ];
        let mut anc = PartialMapAncestors {
            yes: HashSet::from(["sha1".to_string()]),
            unknown: HashSet::from(["ghost".to_string()]),
        };
        let r = resolve(&cs, &mut anc);
        assert_eq!(r.status, Status::Closed);
        assert_eq!(r.resolution, Resolution::Partial);
    }

    fn facts(edges: &[(&str, &[&str])]) -> BTreeMap<String, CommitFact> {
        edges
            .iter()
            .map(|(sha, parents)| {
                (
                    sha.to_string(),
                    CommitFact {
                        id: (*sha).into(),
                        project_id: "p".into(),
                        parents: parents.iter().map(|p| p.to_string()).collect(),
                        patch_id: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn facts_linear_chain() {
        // c3 -> c2 -> c1 (root)
        let f = facts(&[("c1", &[]), ("c2", &["c1"]), ("c3", &["c2"])]);
        let mut anc = FactsAncestors::new(&f, "c3");
        assert_eq!(anc.contains("c1"), Ancestry::Yes);
        assert_eq!(anc.contains("c3"), Ancestry::Yes);
        let mut anc_at_c2 = FactsAncestors::new(&f, "c2");
        assert_eq!(anc_at_c2.contains("c3"), Ancestry::No);
    }

    #[test]
    fn facts_diamond_merge() {
        // m -> {a, b} -> root
        let f = facts(&[
            ("root", &[]),
            ("a", &["root"]),
            ("b", &["root"]),
            ("m", &["a", "b"]),
        ]);
        let mut anc = FactsAncestors::new(&f, "m");
        assert_eq!(anc.contains("a"), Ancestry::Yes);
        assert_eq!(anc.contains("b"), Ancestry::Yes);
        assert_eq!(anc.contains("root"), Ancestry::Yes);
    }

    #[test]
    fn facts_incomplete_graph_answers_unknown_for_absent() {
        // c2's parent c1 has no fact: closure incomplete.
        let f = facts(&[("c2", &["c1"]), ("c3", &["c2"])]);
        let mut anc = FactsAncestors::new(&f, "c3");
        assert_eq!(anc.contains("c2"), Ancestry::Yes); // actually reached
        assert_eq!(anc.contains("other"), Ancestry::Unknown); // can't rule out
    }

    #[test]
    fn facts_missing_head_is_all_unknown() {
        let f = facts(&[("c1", &[])]);
        let mut anc = FactsAncestors::new(&f, "nope");
        assert_eq!(anc.contains("c1"), Ancestry::Unknown);
        assert_eq!(anc.contains("nope"), Ancestry::Unknown);
    }

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
            ]
            .into_iter()
            .collect();
            let mut anc = FactsAncestors::new(&facts, "c2");
            assert_eq!(anc.contains("c1"), Ancestry::Yes); // exact, reachable
            assert_eq!(anc.contains("X"), Ancestry::Rewritten); // squashed, patch match
        }

        /// No patch-id match -> No (when the graph is complete).
        #[test]
        fn facts_no_patch_match_is_no() {
            let facts: BTreeMap<String, CommitFact> = [
                ("c1".to_string(), fact("c1", &[], Some("pA"))),
                ("X".to_string(), fact("X", &[], Some("pB"))),
            ]
            .into_iter()
            .collect();
            let mut anc = FactsAncestors::new(&facts, "c1");
            assert_eq!(anc.contains("X"), Ancestry::No);
        }

        /// An anchor with no patch-id never matches.
        #[test]
        fn facts_none_patch_never_matches() {
            let facts: BTreeMap<String, CommitFact> = [
                ("c1".to_string(), fact("c1", &[], Some("pA"))),
                ("X".to_string(), fact("X", &[], None)),
            ]
            .into_iter()
            .collect();
            let mut anc = FactsAncestors::new(&facts, "c1");
            assert_eq!(anc.contains("X"), Ancestry::No);
        }

        /// A `Rewritten` anchor closes the task but downgrades to Squashed.
        #[test]
        fn rewritten_resolves_closed_squashed() {
            struct Rw;
            impl AncestorSet for Rw {
                fn contains(&mut self, _sha: &str) -> Ancestry {
                    Ancestry::Rewritten
                }
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
            assert_eq!(
                Resolution::Exact.weaken(Resolution::Squashed),
                Resolution::Squashed
            );
            assert_eq!(
                Resolution::Squashed.weaken(Resolution::Partial),
                Resolution::Partial
            );
            assert_eq!(
                Resolution::Facts.weaken(Resolution::Squashed),
                Resolution::Squashed
            );
            // A stronger grade never upgrades a weaker one.
            assert_eq!(
                Resolution::Squashed.weaken(Resolution::Exact),
                Resolution::Squashed
            );
        }
    }

    fn review(id: &str, verdict: ReviewVerdict, commit: &str, at: &str) -> Review {
        Review {
            id: id.into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            verdict,
            commit: commit.into(),
            note: format!("note {id}"),
            by_dev: "r".into(),
            by_machine: "m".into(),
            by_machine_id: String::new(),
            by_worktree: "/r".into(),
            self_review_warning: false,
            created: at.into(),
        }
    }

    #[test]
    fn reviews_apply_by_ancestry() {
        let rs = [
            review("r1", ReviewVerdict::SentBack, "a", "2026-07-01T00:00:00Z"),
            review("r2", ReviewVerdict::Verified, "b", "2026-07-02T00:00:00Z"),
        ];
        let refs: Vec<&Review> = rs.iter().collect();
        // On a checkout holding only `a`, the later verification at `b`
        // doesn't apply, so the task reads as sent back here.
        let (applying, res) =
            applying_reviews(&refs, &mut MapAncestors::of(["a"]), Resolution::Exact);
        assert_eq!(applying.len(), 1);
        assert_eq!(res, Resolution::Exact);
        assert_eq!(review_state(&applying), ReviewState::SentBack);
        // Holding both: the newest applying review, the verification, wins.
        let (applying, _) =
            applying_reviews(&refs, &mut MapAncestors::of(["a", "b"]), Resolution::Exact);
        assert_eq!(review_state(&applying), ReviewState::Verified);
        // Holding neither: no review here.
        let (applying, _) = applying_reviews(&refs, &mut MapAncestors::of([]), Resolution::Exact);
        assert_eq!(review_state(&applying), ReviewState::None);
    }

    #[test]
    fn unknown_review_commit_is_skipped_and_partial() {
        let rs = [review(
            "r1",
            ReviewVerdict::Verified,
            "x",
            "2026-07-01T00:00:00Z",
        )];
        let refs: Vec<&Review> = rs.iter().collect();
        let mut anc = PartialMapAncestors {
            yes: HashSet::new(),
            unknown: ["x".to_string()].into(),
        };
        let (applying, res) = applying_reviews(&refs, &mut anc, Resolution::Facts);
        assert!(applying.is_empty());
        assert_eq!(res, Resolution::Partial);
    }

    #[test]
    fn rewritten_review_commit_applies_as_squashed() {
        struct Rewritten;
        impl AncestorSet for Rewritten {
            fn contains(&mut self, _: &str) -> Ancestry {
                Ancestry::Rewritten
            }
        }
        let rs = [review(
            "r1",
            ReviewVerdict::Verified,
            "x",
            "2026-07-01T00:00:00Z",
        )];
        let refs: Vec<&Review> = rs.iter().collect();
        let (applying, res) = applying_reviews(&refs, &mut Rewritten, Resolution::Exact);
        assert_eq!(review_state(&applying), ReviewState::Verified);
        assert_eq!(res, Resolution::Squashed);
    }

    #[test]
    fn closing_anchor_is_the_applying_close() {
        let cs = [
            change("c1", StatusKind::Closed, Some("a"), "2026-07-01T00:00:00Z"),
            change(
                "c2",
                StatusKind::Reopened,
                Some("b"),
                "2026-07-02T00:00:00Z",
            ),
            change("c3", StatusKind::Closed, Some("c"), "2026-07-03T00:00:00Z"),
        ];
        let refs: Vec<&StatusChange> = cs.iter().collect();
        assert_eq!(
            closing_anchor(&refs, &mut MapAncestors::of(["a"])),
            Some("a")
        );
        assert_eq!(
            closing_anchor(&refs, &mut MapAncestors::of(["a", "b"])),
            None
        );
        assert_eq!(
            closing_anchor(&refs, &mut MapAncestors::of(["a", "b", "c"])),
            Some("c")
        );
        assert_eq!(closing_anchor(&refs, &mut MapAncestors::of([])), None);
        let unanchored = [change(
            "c1",
            StatusKind::Closed,
            None,
            "2026-07-01T00:00:00Z",
        )];
        let refs: Vec<&StatusChange> = unanchored.iter().collect();
        assert_eq!(closing_anchor(&refs, &mut MapAncestors::of([])), None);
    }

    #[test]
    fn review_differs_compares_newest_verification_with_the_close() {
        let rs = [
            review("r1", ReviewVerdict::Verified, "a", "2026-07-01T00:00:00Z"),
            review("r2", ReviewVerdict::Verified, "b", "2026-07-02T00:00:00Z"),
            review("r3", ReviewVerdict::SentBack, "c", "2026-07-03T00:00:00Z"),
        ];
        let refs: Vec<&Review> = rs.iter().collect();
        let (applying, _) = applying_reviews(
            &refs,
            &mut MapAncestors::of(["a", "b", "c"]),
            Resolution::Exact,
        );
        // Newest verification is at `b`; a send-back after it doesn't count.
        assert_eq!(review_differs(Some("b"), &applying), None);
        assert_eq!(
            review_differs(Some("a"), &applying),
            Some(ReviewDiffers {
                verified: "b".into(),
                closed: "a".into()
            })
        );
        assert_eq!(review_differs(None, &applying), None);
        let (none, _) =
            applying_reviews(&refs[2..], &mut MapAncestors::of(["c"]), Resolution::Exact);
        assert_eq!(review_differs(Some("a"), &none), None);
    }
}
