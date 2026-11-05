//! Requested reads: whether each request on a task
//! is still open, and whether the task was closed before anyone read it.

use crate::entities::{ReadRequest, Review, StatusKind};
use crate::materialize::World;
use crate::resolve::{AncestorSet, Ancestry, CommitOrder};

#[derive(Debug, Clone)]
pub struct ReadRequestState<'a> {
    pub request: &'a ReadRequest,
    /// The first review of the task made after the request, by anyone, that
    /// read the requested commit or a descendant of it. A review of an older
    /// commit, or one whose order against the request can't be answered,
    /// doesn't answer it. A review answers wherever it was made; its commit
    /// needn't apply on this checkout.
    pub answered_by: Option<&'a Review>,
    /// A close that applies on this checkout was made while the request was
    /// open, so it merged before anyone read it. Worked out from the log and git.
    pub closed_before_read: bool,
}

impl ReadRequestState<'_> {
    pub fn is_open(&self) -> bool {
        self.answered_by.is_none()
    }
}

/// Every read request on `task_id` that hasn't been withdrawn, oldest first,
/// with whether it's answered and whether a close beat the read here. `anc`
/// answers ancestry on this checkout; `order` compares a review's commit with
/// the requested one.
pub fn read_request_states<'a>(
    world: &'a World,
    task_id: &str,
    anc: &mut impl AncestorSet,
    order: &mut impl CommitOrder,
) -> Vec<ReadRequestState<'a>> {
    let reviews = world.reviews_for(task_id);
    let closes: Vec<&str> = world
        .changes_for(task_id)
        .into_iter()
        .filter(|c| c.to_status == StatusKind::Closed)
        .filter(|c| match &c.anchor_commit {
            None => true,
            Some(sha) => matches!(anc.contains(sha), Ancestry::Yes | Ancestry::Rewritten),
        })
        .map(|c| c.created.as_str())
        .collect();
    let mut requests: Vec<&ReadRequest> = world
        .read_requests
        .values()
        .filter(|r| &*r.task_id == task_id)
        .collect();
    requests.sort_by(|a, b| (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0)));
    requests
        .into_iter()
        .map(|request| {
            let asked = request.created.as_str();
            let answered_by = reviews
                .iter()
                .find(|r| {
                    r.created.as_str() > asked
                        && matches!(
                            order.is_ancestor(&request.commit, &r.commit),
                            Ancestry::Yes | Ancestry::Rewritten
                        )
                })
                .copied();
            let until = answered_by.map(|r| r.created.as_str());
            let closed_before_read = closes
                .iter()
                .any(|&at| at > asked && until.is_none_or(|u| at < u));
            ReadRequestState {
                request,
                answered_by,
                closed_before_read,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::*;
    use crate::resolve::{MapAncestors, MapOrder};

    /// a <- b, with requests made at a.
    fn line() -> MapOrder {
        MapOrder::of([("a", &[]), ("b", &["a"]), ("z", &[])])
    }

    fn request(dev: &str, worktree: &str, at: &str) -> ReadRequest {
        ReadRequest {
            id: read_request_id("t1", dev, "m", worktree).into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            commit: "a".into(),
            by_dev: dev.into(),
            by_machine: "m".into(),
            by_machine_id: String::new(),
            by_worktree: worktree.into(),
            created: at.into(),
        }
    }

    fn review(id: &str, at: &str) -> Review {
        Review {
            id: id.into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            verdict: ReviewVerdict::Verified,
            commit: "a".into(),
            note: String::new(),
            by_dev: "r".into(),
            by_machine: "m".into(),
            by_machine_id: String::new(),
            by_worktree: "/r".into(),
            self_review_warning: false,
            created: at.into(),
        }
    }

    fn close(id: &str, anchor: Option<&str>, at: &str) -> StatusChange {
        StatusChange {
            id: id.into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            to_status: StatusKind::Closed,
            anchor_commit: anchor.map(str::to_string),
            created: at.into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        }
    }

    fn world(requests: &[ReadRequest], reviews: &[Review], changes: &[StatusChange]) -> World {
        let mut w = World::default();
        for r in requests {
            w.read_requests.insert(r.id.0.to_string(), r.clone());
        }
        w.reviews = reviews.to_vec();
        w.status_changes = changes.to_vec();
        w
    }

    #[test]
    fn id_is_one_per_requester_per_task() {
        let a = read_request_id("t1", "x@laptop", "laptop", "/one");
        assert_eq!(a, read_request_id("t1", "x@laptop", "laptop", "/one"));
        assert_ne!(a, read_request_id("t1", "x@laptop", "laptop", "/two"));
        assert_ne!(a, read_request_id("t2", "x@laptop", "laptop", "/one"));
        assert!(a.starts_with("t1:") && a.len() == "t1:".len() + 16);
        // Fields are separated, so shifting text between them changes the id.
        assert_ne!(
            read_request_id("t", "ab", "c", "/w"),
            read_request_id("t", "a", "bc", "/w")
        );
    }

    #[test]
    fn a_later_review_answers_and_an_earlier_one_doesnt() {
        let w = world(
            &[request("x", "/one", "2026-07-02T00:00:00Z")],
            &[
                review("r0", "2026-07-01T00:00:00Z"),
                review("r1", "2026-07-03T00:00:00Z"),
            ],
            &[],
        );
        let states = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].answered_by.map(|r| &*r.id.0), Some("r1"));
        assert!(!states[0].is_open());
    }

    #[test]
    fn unanswered_request_is_open() {
        let w = world(&[request("x", "/one", "2026-07-02T00:00:00Z")], &[], &[]);
        let states = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(states[0].is_open());
        assert!(!states[0].closed_before_read);
    }

    #[test]
    fn close_while_open_and_applying_here_is_closed_before_read() {
        let req = [request("x", "/one", "2026-07-02T00:00:00Z")];
        let closes = [close("c1", Some("b"), "2026-07-03T00:00:00Z")];
        let w = world(&req, &[review("r1", "2026-07-04T00:00:00Z")], &closes);
        // The close applies here (b is an ancestor) and came before the read.
        let s = read_request_states(&w, "t1", &mut MapAncestors::of(["b"]), &mut line());
        assert!(s[0].closed_before_read);
        // On a checkout without b, the close doesn't apply.
        let s = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(!s[0].closed_before_read);
    }

    #[test]
    fn close_before_request_or_after_answer_doesnt_count() {
        let req = [request("x", "/one", "2026-07-02T00:00:00Z")];
        let closes = [
            close("c0", None, "2026-07-01T00:00:00Z"),
            close("c2", None, "2026-07-05T00:00:00Z"),
        ];
        let w = world(&req, &[review("r1", "2026-07-04T00:00:00Z")], &closes);
        let s = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(!s[0].closed_before_read);
    }

    #[test]
    fn withdrawn_request_is_gone() {
        let req = request("x", "/one", "2026-07-02T00:00:00Z");
        let mut set = myko::wire::MEvent::from_item(&req, myko::wire::MEventType::SET, "m");
        set.created_at = "2026-07-02T00:00:00Z".into();
        let mut del = myko::wire::MEvent::from_item(&req, myko::wire::MEventType::DEL, "m");
        del.created_at = "2026-07-03T00:00:00Z".into();
        let w = crate::materialize::materialize(vec![
            crate::materialize::EventRecord {
                id: "aa".into(),
                event: set,
            },
            crate::materialize::EventRecord {
                id: "bb".into(),
                event: del,
            },
        ]);
        assert!(read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line()).is_empty());
    }

    fn review_at(id: &str, commit: &str, at: &str) -> Review {
        Review {
            commit: commit.into(),
            ..review(id, at)
        }
    }

    #[test]
    fn only_a_review_of_the_requested_commit_or_later_answers() {
        // Requested at b; a review of the older a doesn't answer, a later
        // review of b does.
        let mut req = request("x", "/one", "2026-07-02T00:00:00Z");
        req.commit = "b".into();
        let w = world(
            &[req],
            &[
                review_at("r1", "a", "2026-07-03T00:00:00Z"),
                review_at("r2", "b", "2026-07-04T00:00:00Z"),
            ],
            &[],
        );
        let s = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert_eq!(s[0].answered_by.map(|r| &*r.id.0), Some("r2"));
        // Without r2, the request stays open.
        let w2 = World {
            reviews: w.reviews[..1].to_vec(),
            read_requests: w.read_requests.clone(),
            ..Default::default()
        };
        let s = read_request_states(&w2, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(s[0].is_open());
    }

    #[test]
    fn a_review_of_a_descendant_answers() {
        // Requested at a, read at its descendant b.
        let w = world(
            &[request("x", "/one", "2026-07-02T00:00:00Z")],
            &[review_at("r1", "b", "2026-07-03T00:00:00Z")],
            &[],
        );
        let s = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(!s[0].is_open());
    }

    #[test]
    fn unknown_order_leaves_the_request_open() {
        // `y` isn't in the commit graph, and `z` is unrelated to `a`.
        let w = world(
            &[request("x", "/one", "2026-07-02T00:00:00Z")],
            &[
                review_at("r1", "y", "2026-07-03T00:00:00Z"),
                review_at("r2", "z", "2026-07-04T00:00:00Z"),
            ],
            &[],
        );
        let s = read_request_states(&w, "t1", &mut MapAncestors::of([]), &mut line());
        assert!(s[0].is_open());
    }
}
