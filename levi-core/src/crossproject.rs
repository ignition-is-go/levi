//! Cross-project status resolution over hub entities (spec
//! 2026-07-21-cross-project-graph-design).
//!
//! Two precisions, deliberately:
//!
//! - [`statuses_unanchored`] resolves from StatusChanges alone — no ancestry,
//!   so no CommitFacts cross the wire. A task with no close is open; one with
//!   a close is "closed somewhere" (`Resolution::Partial`), honest that the
//!   branch containing it was not established. This is what the cross-project
//!   *listing* uses: enough to pick a blocker, cheap regardless of history.
//! - [`statuses_for_project`] resolves exactly against a branch head by
//!   walking the CommitFact graph (moved here from levi-dash so the dashboard
//!   and any future hub report share one implementation).

use std::collections::BTreeMap;

use crate::entities::{CommitFact, RefFact, StatusChange, StatusKind, Task};
use crate::resolve::{FactsAncestors, Resolution, ResolvedStatus, Status, effective_status};

/// Status from StatusChanges alone — no ancestry walk, no facts. Open when a
/// task has no close event; Closed + `Partial` when it has one (a close
/// exists, but which branches contain it is not established here).
pub fn statuses_unanchored(
    tasks: &[Task],
    changes: &[StatusChange],
    project_id: &str,
) -> BTreeMap<String, ResolvedStatus> {
    // Latest change per task in (created, id) order decides the status; any
    // close at all marks the resolution Partial.
    let mut by_task: BTreeMap<&str, Vec<&StatusChange>> = BTreeMap::new();
    for c in changes.iter().filter(|c| c.project_id == project_id) {
        by_task.entry(&c.task_id).or_default().push(c);
    }
    tasks
        .iter()
        .filter(|t| t.project_id == project_id)
        .map(|t| {
            let id = t.id.to_string();
            let mut mine = by_task.get(id.as_str()).cloned().unwrap_or_default();
            mine.sort_by(|a, b| {
                (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0))
            });
            let mut status = Status::Open;
            let mut ever_closed = false;
            for c in &mine {
                status = match c.to_status {
                    StatusKind::Closed => {
                        ever_closed = true;
                        Status::Closed
                    }
                    StatusKind::Reopened => Status::Open,
                };
            }
            // Any anchored history means we cannot claim exactness here.
            let resolution = if ever_closed {
                Resolution::Partial
            } else {
                Resolution::Exact
            };
            (id, ResolvedStatus { status, resolution })
        })
        .collect()
}

/// The branch a project resolves against when none is named: its `main`
/// RefFact if present, else the most recently observed.
pub fn default_head(refs: &[RefFact], project_id: &str) -> Option<String> {
    let mine: Vec<&RefFact> = refs.iter().filter(|r| r.project_id == project_id).collect();
    mine.iter()
        .find(|r| r.branch == "main")
        .or_else(|| mine.iter().max_by(|a, b| a.observed.cmp(&b.observed)))
        .map(|r| r.head.clone())
}

/// Exact per-branch resolution over the CommitFact graph (moved from
/// `levi-dash/src/resolve_client.rs`). `None` head resolves anchored changes
/// as `Partial` rather than guessing.
pub fn statuses_for_project(
    tasks: &[Task],
    changes: &[StatusChange],
    facts: &[CommitFact],
    project_id: &str,
    head: Option<&str>,
) -> BTreeMap<String, ResolvedStatus> {
    let fact_map: BTreeMap<String, CommitFact> = facts
        .iter()
        .filter(|f| f.project_id == project_id)
        .map(|f| (f.id.to_string(), f.clone()))
        .collect();
    let mut sorted: Vec<&StatusChange> = changes
        .iter()
        .filter(|c| c.project_id == project_id)
        .collect();
    sorted.sort_by(|a, b| (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0)));

    let mut anc = head.map(|h| FactsAncestors::new(&fact_map, h));
    tasks
        .iter()
        .filter(|t| t.project_id == project_id)
        .map(|t| {
            let id = t.id.to_string();
            let mine: Vec<&StatusChange> = sorted
                .iter()
                .copied()
                .filter(|c| *c.task_id == id)
                .collect();
            let resolved = match &mut anc {
                Some(anc) => effective_status(&mine, anc, Resolution::Facts),
                None => {
                    // No head to resolve against: unanchored changes apply,
                    // anchored ones are unknown -> Partial.
                    let mut none = NoAncestry;
                    effective_status(&mine, &mut none, Resolution::Partial)
                }
            };
            (id, resolved)
        })
        .collect()
}

/// An ancestor set that knows nothing: every anchored sha is `Unknown`.
struct NoAncestry;
impl crate::resolve::AncestorSet for NoAncestry {
    fn contains(&mut self, _sha: &str) -> crate::resolve::Ancestry {
        crate::resolve::Ancestry::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, project: &str) -> Task {
        Task {
            id: id.into(),
            project_id: project.into(),
            title: format!("task {id}"),
            body: String::new(),
            priority: Default::default(),
            labels: vec![],
            created_by_dev: "d".into(),
            created_by_machine: "m".into(),
            created: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn change(id: &str, task: &str, project: &str, to: StatusKind, at: &str) -> StatusChange {
        StatusChange {
            id: id.into(),
            project_id: project.into(),
            task_id: task.into(),
            to_status: to,
            anchor_commit: Some("deadbeef".into()),
            created: at.into(),
            by_dev: "d".into(),
            by_machine: "m".into(),
        }
    }

    #[test]
    fn open_when_no_change() {
        let tasks = vec![task("t1", "p")];
        let got = statuses_unanchored(&tasks, &[], "p");
        assert_eq!(got["t1"].status, Status::Open);
        assert_eq!(got["t1"].resolution, Resolution::Exact);
    }

    #[test]
    fn closed_is_partial_ignoring_anchors() {
        let tasks = vec![task("t1", "p")];
        // An anchored close: statuses_unanchored must not need the anchor's
        // ancestry — it reports Closed + Partial regardless.
        let changes = vec![change(
            "c1",
            "t1",
            "p",
            StatusKind::Closed,
            "2026-02-01T00:00:00Z",
        )];
        let got = statuses_unanchored(&tasks, &changes, "p");
        assert_eq!(got["t1"].status, Status::Closed);
        assert_eq!(got["t1"].resolution, Resolution::Partial);
    }

    #[test]
    fn latest_change_wins() {
        let tasks = vec![task("t1", "p")];
        let changes = vec![
            change("c1", "t1", "p", StatusKind::Closed, "2026-02-01T00:00:00Z"),
            change(
                "c2",
                "t1",
                "p",
                StatusKind::Reopened,
                "2026-03-01T00:00:00Z",
            ),
        ];
        let got = statuses_unanchored(&tasks, &changes, "p");
        assert_eq!(got["t1"].status, Status::Open);
        // Still Partial: a close event exists in history.
        assert_eq!(got["t1"].resolution, Resolution::Partial);
    }

    #[test]
    fn other_projects_ignored() {
        let tasks = vec![task("t1", "p"), task("t2", "other")];
        let got = statuses_unanchored(&tasks, &[], "p");
        assert!(got.contains_key("t1"));
        assert!(!got.contains_key("t2"));
    }

    #[test]
    fn default_head_prefers_main() {
        let refs = vec![
            RefFact {
                id: "p:feature".into(),
                project_id: "p".into(),
                branch: "feature".into(),
                head: "aaaa".into(),
                observed: "2026-05-01T00:00:00Z".into(),
            },
            RefFact {
                id: "p:main".into(),
                project_id: "p".into(),
                branch: "main".into(),
                head: "bbbb".into(),
                observed: "2026-01-01T00:00:00Z".into(),
            },
        ];
        assert_eq!(default_head(&refs, "p").as_deref(), Some("bbbb"));
    }

    #[test]
    fn default_head_falls_back_to_latest_observed() {
        let refs = vec![
            RefFact {
                id: "p:a".into(),
                project_id: "p".into(),
                branch: "a".into(),
                head: "aaaa".into(),
                observed: "2026-05-01T00:00:00Z".into(),
            },
            RefFact {
                id: "p:b".into(),
                project_id: "p".into(),
                branch: "b".into(),
                head: "bbbb".into(),
                observed: "2026-06-01T00:00:00Z".into(),
            },
        ];
        assert_eq!(default_head(&refs, "p").as_deref(), Some("bbbb"));
    }

    #[test]
    fn default_head_none_without_refs() {
        assert_eq!(default_head(&[], "p"), None);
    }

    fn commit(id: &str, project: &str, parents: &[&str]) -> CommitFact {
        CommitFact {
            id: id.into(),
            project_id: project.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn statuses_for_project_resolves_against_head_via_facts() {
        // c1 <- c2 <- c3 ; a close anchored at c2.
        let tasks = vec![task("t1", "p")];
        let changes = vec![StatusChange {
            anchor_commit: Some("c2".into()),
            ..change("x1", "t1", "p", StatusKind::Closed, "2026-02-01T00:00:00Z")
        }];
        let facts = vec![
            commit("c1", "p", &[]),
            commit("c2", "p", &["c1"]),
            commit("c3", "p", &["c2"]),
        ];

        // Against c3 (c2 is an ancestor): closed, exactly (Facts).
        let at_c3 = statuses_for_project(&tasks, &changes, &facts, "p", Some("c3"));
        assert_eq!(at_c3["t1"].status, Status::Closed);
        assert_eq!(at_c3["t1"].resolution, Resolution::Facts);

        // Against c1 (before the fix): open.
        let at_c1 = statuses_for_project(&tasks, &changes, &facts, "p", Some("c1"));
        assert_eq!(at_c1["t1"].status, Status::Open);

        // No head: the anchored change is unknown -> open + Partial.
        let no_head = statuses_for_project(&tasks, &changes, &facts, "p", None);
        assert_eq!(no_head["t1"].status, Status::Open);
        assert_eq!(no_head["t1"].resolution, Resolution::Partial);
    }
}
