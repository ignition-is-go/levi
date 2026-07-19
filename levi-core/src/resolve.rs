//! Per-checkout status resolution (spec §Status resolution).
//!
//! Effective status of a task on a checkout = fold of its StatusChanges,
//! restricted to those whose `anchor_commit` is an ancestor of HEAD
//! (unanchored changes apply everywhere), ordered by (at, id). A change whose
//! anchor's ancestry is *unknown* (missing commit facts / unfetched sha) is
//! skipped — treated open — and downgrades the resolution to `Partial` so the
//! caller can flag the task. No qualifying changes ⇒ open.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::entities::{CommitFact, StatusChange, StatusKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ancestry {
    Yes,
    No,
    Unknown,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Exact => "exact",
            Resolution::Facts => "facts",
            Resolution::Partial => "partial",
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
                Ancestry::No => false,
                Ancestry::Unknown => {
                    resolution = Resolution::Partial;
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

/// Ancestor set over the CommitFact graph, for git-free nodes (the hub, the
/// dashboard's branch selector). BFS closure from `head`; if the walk ever
/// needs a sha with no fact, the closure is incomplete and non-members answer
/// `Unknown` instead of `No` (spec: never silently closed — and never
/// silently *not*-closed either).
pub struct FactsAncestors {
    reachable: HashSet<String>,
    complete: bool,
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
        Self { reachable, complete }
    }
}

impl AncestorSet for FactsAncestors {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if self.reachable.contains(sha) {
            Ancestry::Yes
        } else if self.complete {
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
        if self.0.contains(sha) { Ancestry::Yes } else { Ancestry::No }
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
        Self { inner, cache: HashMap::new() }
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
            at: at.into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        }
    }

    fn resolve(changes: &[StatusChange], anc: &mut impl AncestorSet) -> ResolvedStatus {
        let mut refs: Vec<&StatusChange> = changes.iter().collect();
        refs.sort_by(|a, b| (a.at.as_str(), &*a.id.0).cmp(&(b.at.as_str(), &*b.id.0)));
        effective_status(&refs, anc, Resolution::Exact)
    }

    #[test]
    fn no_changes_means_open() {
        let r = resolve(&[], &mut MapAncestors::of([]));
        assert_eq!(r, ResolvedStatus { status: Status::Open, resolution: Resolution::Exact });
    }

    #[test]
    fn close_anchored_on_ancestor_closes_here_only() {
        let cs = [change("a", StatusKind::Closed, Some("sha1"), "2026-07-01T00:00:00Z")];
        // On a checkout containing sha1:
        assert_eq!(resolve(&cs, &mut MapAncestors::of(["sha1"])).status, Status::Closed);
        // On a branch that doesn't contain it:
        assert_eq!(resolve(&cs, &mut MapAncestors::of([])).status, Status::Open);
    }

    #[test]
    fn close_then_reopen_both_in_ancestry() {
        let cs = [
            change("a", StatusKind::Closed, Some("sha1"), "2026-07-01T00:00:00Z"),
            change("b", StatusKind::Reopened, Some("sha2"), "2026-07-02T00:00:00Z"),
        ];
        assert_eq!(resolve(&cs, &mut MapAncestors::of(["sha1", "sha2"])).status, Status::Open);
        // Reopen anchored elsewhere: still closed here.
        assert_eq!(resolve(&cs, &mut MapAncestors::of(["sha1"])).status, Status::Closed);
    }

    #[test]
    fn unanchored_changes_apply_everywhere() {
        let closed_everywhere = [change("a", StatusKind::Closed, None, "2026-07-01T00:00:00Z")];
        assert_eq!(resolve(&closed_everywhere, &mut MapAncestors::of([])).status, Status::Closed);

        let reopened_everywhere = [
            change("a", StatusKind::Closed, Some("sha1"), "2026-07-01T00:00:00Z"),
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
        assert_eq!(resolve(&cs, &mut MapAncestors::of([])).status, Status::Closed);
    }

    #[test]
    fn unknown_anchor_treated_open_and_flagged() {
        let cs = [change("a", StatusKind::Closed, Some("ghost"), "2026-07-01T00:00:00Z")];
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
            change("a", StatusKind::Reopened, Some("ghost"), "2026-07-01T00:00:00Z"),
            change("b", StatusKind::Closed, Some("sha1"), "2026-07-02T00:00:00Z"),
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
        let f = facts(&[("root", &[]), ("a", &["root"]), ("b", &["root"]), ("m", &["a", "b"])]);
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
}
