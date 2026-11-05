//! The review gate behind `levi check-claims --require-review`: does a task
//! carry a verification that holds at the tested commit, and did it read the
//! commit that closed the task, or something after it?

use crate::entities::ReviewVerdict;
use crate::materialize::World;
use crate::resolve::{
    AncestorSet, Ancestry, CommitOrder, Resolution, ReviewDiffers, ReviewState, applying_reviews,
    closing_anchor, review_differs, review_state,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGate {
    /// The task's review state at the tested commit.
    pub review: ReviewState,
    /// How ancestry was answered, for the reviews and for the verification's
    /// order against the close. `Partial` fails the gate: what can't be
    /// placed is never silently counted.
    pub resolution: Resolution,
    /// Read requests still waiting for an answer. An open one fails the
    /// gate: a requested read is answered before the merge.
    pub open_reads: usize,
    /// The counting verification read a commit strictly older than the one
    /// that closed the task, so the commits between were never reviewed. The
    /// rule is a read at the head being merged. Fails the gate.
    pub stale_review: bool,
    /// The counting verification was made with a self-review warning: the
    /// reviewer matched the closer on dev and machine only.
    pub self_review_warning: bool,
    /// For information, not part of the verdict: the newest verification
    /// read a different commit from the one that closed the task.
    pub differs: Option<ReviewDiffers>,
}

impl ReviewGate {
    pub fn passes(&self) -> bool {
        self.review == ReviewState::Verified
            && self.resolution != Resolution::Partial
            && self.open_reads == 0
            && !self.stale_review
    }
}

/// Evaluate the gate for `task_id`. `anc` answers ancestry at the tested
/// commit; `order` compares two commits (the close and the verification).
/// `base` is `Exact` or `Facts`, as for `effective_status`.
pub fn review_gate(
    world: &World,
    task_id: &str,
    anc: &mut impl AncestorSet,
    order: &mut impl CommitOrder,
    base: Resolution,
) -> ReviewGate {
    let reviews = world.reviews_for(task_id);
    let (applying, mut resolution) = applying_reviews(&reviews, &mut *anc, base);
    let closed = closing_anchor(&world.changes_for(task_id), &mut *anc);
    let review = review_state(&applying);
    let counting = applying
        .last()
        .map(|(r, _)| *r)
        .filter(|r| r.verdict == ReviewVerdict::Verified);
    // A verification counts only if it read the closing commit or a later
    // one. An unanchored close has no commit to compare with.
    let mut stale_review = false;
    if let (Some(verified), Some(closed)) = (counting, closed) {
        match order.is_ancestor(closed, &verified.commit) {
            Ancestry::Yes => {}
            Ancestry::Rewritten => resolution = resolution.weaken(Resolution::Squashed),
            Ancestry::No => stale_review = true,
            Ancestry::Unknown => resolution = resolution.weaken(Resolution::Partial),
        }
    }
    let open_reads = crate::reads::read_request_states(world, task_id, &mut *anc)
        .iter()
        .filter(|r| r.is_open())
        .count();
    ReviewGate {
        review,
        resolution,
        open_reads,
        stale_review,
        self_review_warning: counting.is_some_and(|r| r.self_review_warning),
        differs: review_differs(closed, &applying),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::*;
    use crate::resolve::{MapAncestors, MapOrder, PartialMapAncestors};

    fn review(id: &str, verdict: ReviewVerdict, commit: &str, at: &str) -> Review {
        Review {
            id: id.into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            verdict,
            commit: commit.into(),
            note: String::new(),
            by_dev: "r".into(),
            by_machine: "m".into(),
            by_machine_id: String::new(),
            by_worktree: "/r".into(),
            self_review_warning: false,
            created: at.into(),
        }
    }

    /// Linear history a <- b <- c.
    fn line() -> MapOrder {
        MapOrder::of([("a", &[]), ("b", &["a"]), ("c", &["b"])])
    }

    fn closed_at(w: &mut World, anchor: &str) {
        w.status_changes.push(StatusChange {
            id: "c1".into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            to_status: StatusKind::Closed,
            anchor_commit: Some(anchor.into()),
            created: "2026-07-01T00:00:00Z".into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        });
    }

    #[test]
    fn no_review_fails() {
        let w = World::default();
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of([]),
            &mut line(),
            Resolution::Exact,
        );
        assert_eq!(g.review, ReviewState::None);
        assert!(!g.passes());
    }

    #[test]
    fn verification_at_the_tested_commit_passes() {
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "a",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "a");
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert!(g.passes());
        assert_eq!(g.differs, None);
        assert_eq!(g.open_reads, 0);
    }

    #[test]
    fn verification_not_in_the_tested_history_fails() {
        let w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "b",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert!(!g.passes());
    }

    #[test]
    fn newest_send_back_fails_even_after_a_verification() {
        let w = World {
            reviews: vec![
                review("r1", ReviewVerdict::Verified, "a", "2026-07-02T00:00:00Z"),
                review("r2", ReviewVerdict::SentBack, "a", "2026-07-03T00:00:00Z"),
            ],
            ..Default::default()
        };
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert_eq!(g.review, ReviewState::SentBack);
        assert!(!g.passes());
    }

    #[test]
    fn an_unplaceable_review_fails_as_partial() {
        let w = World {
            reviews: vec![
                review("r1", ReviewVerdict::Verified, "a", "2026-07-02T00:00:00Z"),
                review("r2", ReviewVerdict::SentBack, "x", "2026-07-03T00:00:00Z"),
            ],
            ..Default::default()
        };
        let mut anc = PartialMapAncestors {
            yes: ["a".to_string()].into(),
            unknown: ["x".to_string()].into(),
        };
        let g = review_gate(&w, "t1", &mut anc, &mut line(), Resolution::Facts);
        // The send-back at `x` can't be placed: the verification alone would
        // pass, so the gate must refuse rather than guess.
        assert_eq!(g.review, ReviewState::Verified);
        assert_eq!(g.resolution, Resolution::Partial);
        assert!(!g.passes());
    }

    #[test]
    fn a_verification_after_the_close_passes_and_reports_differs() {
        // a <- b <- c: closed at b, verified at the later c.
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "c",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "b");
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a", "b", "c"]),
            &mut line(),
            Resolution::Exact,
        );
        assert!(!g.stale_review);
        assert!(g.passes());
        assert_eq!(
            g.differs,
            Some(ReviewDiffers {
                verified: "c".into(),
                closed: "b".into()
            })
        );
    }

    #[test]
    fn a_verification_older_than_the_close_fails_as_stale() {
        // Closed at c, but the verification read a: b and c were never read.
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "a",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "c");
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a", "b", "c"]),
            &mut line(),
            Resolution::Exact,
        );
        assert_eq!(g.review, ReviewState::Verified);
        assert!(g.stale_review);
        assert!(!g.passes());
        // Still reported, for information.
        assert!(g.differs.is_some());
    }

    #[test]
    fn a_verification_on_a_parallel_branch_is_stale() {
        // a <- b (closed) and a <- x (verified), both in the tested history:
        // the read never saw b.
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "x",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "b");
        let mut order = MapOrder::of([("a", &[]), ("b", &["a"]), ("x", &["a"])]);
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a", "b", "x"]),
            &mut order,
            Resolution::Exact,
        );
        assert!(g.stale_review);
        assert!(!g.passes());
    }

    #[test]
    fn unknown_order_between_close_and_verification_fails_as_partial() {
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "c",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "b");
        // The order graph doesn't know b.
        let mut order = MapOrder::of([("c", &[])]);
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["b", "c"]),
            &mut order,
            Resolution::Exact,
        );
        assert!(!g.stale_review);
        assert_eq!(g.resolution, Resolution::Partial);
        assert!(!g.passes());
    }

    #[test]
    fn self_review_warning_on_the_counting_verification_is_reported() {
        let mut r = review("r1", ReviewVerdict::Verified, "a", "2026-07-02T00:00:00Z");
        r.self_review_warning = true;
        let mut w = World {
            reviews: vec![r],
            ..Default::default()
        };
        closed_at(&mut w, "a");
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert!(g.passes());
        assert!(g.self_review_warning);
    }

    #[test]
    fn an_open_read_request_fails_and_a_later_review_answers_it() {
        let req = ReadRequest {
            id: read_request_id("t1", "q", "m", "/q").into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            commit: "a".into(),
            by_dev: "q".into(),
            by_machine: "m".into(),
            by_machine_id: String::new(),
            by_worktree: "/q".into(),
            created: "2026-07-03T00:00:00Z".into(),
        };
        // Verified before the request was made: the request is still open.
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "a",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        w.read_requests.insert(req.id.0.to_string(), req);
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert_eq!(g.open_reads, 1);
        assert!(!g.passes());
        // A verification after the request answers it.
        w.reviews.push(review(
            "r2",
            ReviewVerdict::Verified,
            "a",
            "2026-07-04T00:00:00Z",
        ));
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a"]),
            &mut line(),
            Resolution::Exact,
        );
        assert_eq!(g.open_reads, 0);
        assert!(g.passes());
    }
}
