//! The review gate behind `levi check-claims --require-review`: does a task
//! carry a verification that holds at the tested commit?

use crate::materialize::World;
use crate::resolve::{
    AncestorSet, Resolution, ReviewDiffers, ReviewState, applying_reviews, closing_anchor,
    review_differs, review_state,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGate {
    /// The task's review state at the tested commit.
    pub review: ReviewState,
    /// How the reviews' ancestry was answered. `Partial` fails the gate: a
    /// review whose commit can't be placed is never silently counted.
    pub resolution: Resolution,
    /// Read requests still waiting for an answer.
    pub open_reads: usize,
    /// For information, not part of the verdict: the newest verification
    /// read a different commit from the one that closed the task.
    pub differs: Option<ReviewDiffers>,
}

impl ReviewGate {
    pub fn passes(&self) -> bool {
        self.review == ReviewState::Verified
            && self.resolution != Resolution::Partial
            && self.open_reads == 0
    }
}

/// Evaluate the gate for `task_id`, with `anc` answering ancestry at the
/// tested commit. `base` is `Exact` or `Facts`, as for `effective_status`.
pub fn review_gate(
    world: &World,
    task_id: &str,
    anc: &mut impl AncestorSet,
    base: Resolution,
) -> ReviewGate {
    let reviews = world.reviews_for(task_id);
    let (applying, resolution) = applying_reviews(&reviews, &mut *anc, base);
    let closed = closing_anchor(&world.changes_for(task_id), &mut *anc);
    // TODO(RAL-47 feature 2): count open read requests once ReadRequest
    // lands (reads::read_request_states(..).filter(is_open)), and add a test
    // that an open request fails the gate. Until then there are none.
    let open_reads = 0;
    ReviewGate {
        review: review_state(&applying),
        resolution,
        open_reads,
        differs: review_differs(closed, &applying),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::*;
    use crate::resolve::{MapAncestors, PartialMapAncestors};

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
        let g = review_gate(&w, "t1", &mut MapAncestors::of([]), Resolution::Exact);
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
        let g = review_gate(&w, "t1", &mut MapAncestors::of(["a"]), Resolution::Exact);
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
        let g = review_gate(&w, "t1", &mut MapAncestors::of(["a"]), Resolution::Exact);
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
        let g = review_gate(&w, "t1", &mut MapAncestors::of(["a"]), Resolution::Exact);
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
        let g = review_gate(&w, "t1", &mut anc, Resolution::Facts);
        // The send-back at `x` can't be placed: the verification alone would
        // pass, so the gate must refuse rather than guess.
        assert_eq!(g.review, ReviewState::Verified);
        assert_eq!(g.resolution, Resolution::Partial);
        assert!(!g.passes());
    }

    #[test]
    fn verifying_a_different_commit_still_passes_but_reports_it() {
        let mut w = World {
            reviews: vec![review(
                "r1",
                ReviewVerdict::Verified,
                "a",
                "2026-07-02T00:00:00Z",
            )],
            ..Default::default()
        };
        closed_at(&mut w, "b");
        let g = review_gate(
            &w,
            "t1",
            &mut MapAncestors::of(["a", "b"]),
            Resolution::Exact,
        );
        assert!(g.passes());
        assert_eq!(
            g.differs,
            Some(ReviewDiffers {
                verified: "a".into(),
                closed: "b".into()
            })
        );
    }
}
