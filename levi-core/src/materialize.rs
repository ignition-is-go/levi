//! Deterministic replay of the levi event log into materialized state.
//!
//! myko applies events in arrival order; levi guarantees the spec's LWW
//! ordering — `created_at`, ties broken by event id — by sorting every record
//! with that key before replay. Event id = git blob OID of the event's CBOR
//! bytes, so ordering is identical on every node (spec: "Clock skew").

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use myko::prelude::Eventable;
use myko::wire::{MEvent, MEventType};

use crate::entities::*;

/// One event as stored in the ref: `id` is the git blob OID (hex) of the CBOR
/// bytes, `event` the decoded myko wire event.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub id: String,
    pub event: MEvent,
}

/// Materialized state of one project's event log.
#[derive(Debug, Default)]
pub struct World {
    pub project: Option<Project>,
    pub tasks: BTreeMap<String, Task>,
    /// Sorted by (at, id).
    pub status_changes: Vec<StatusChange>,
    pub deps: BTreeMap<String, Dependency>,
    /// Keyed by task_id (== Claim entity id); SET overwrite = newest wins.
    pub claims: BTreeMap<String, Claim>,
    /// Sorted by (at, id).
    pub comments: Vec<Comment>,
    pub commit_facts: BTreeMap<String, CommitFact>,
    pub ref_facts: BTreeMap<String, RefFact>,
}

impl World {
    /// This task's status changes, in (at, id) order.
    pub fn changes_for(&self, task_id: &str) -> Vec<&StatusChange> {
        self.status_changes
            .iter()
            .filter(|c| &*c.task_id == task_id)
            .collect()
    }

    /// The claim on this task, if it exists and its ttl has not expired.
    pub fn live_claim(&self, task_id: &str, now: DateTime<Utc>) -> Option<&Claim> {
        let claim = self.claims.get(task_id)?;
        let at = DateTime::parse_from_rfc3339(&claim.at).ok()?.with_timezone(&Utc);
        let ttl = Duration::seconds(i64::try_from(claim.ttl_secs).ok()?);
        (at + ttl > now).then_some(claim)
    }
}

pub fn materialize(mut records: Vec<EventRecord>) -> World {
    records.sort_by(|a, b| {
        (a.event.created_at.as_str(), a.id.as_str())
            .cmp(&(b.event.created_at.as_str(), b.id.as_str()))
    });

    let mut w = World::default();
    let mut status_changes: BTreeMap<String, StatusChange> = BTreeMap::new();
    let mut comments: BTreeMap<String, Comment> = BTreeMap::new();

    for r in &records {
        let ev = &r.event;
        let t = ev.item_type.as_str();
        if t == Project::ENTITY_NAME_STATIC {
            if matches!(ev.change_type, MEventType::SET)
                && let Ok(p) = serde_json::from_value::<Project>(ev.item.clone())
            {
                w.project = Some(p);
            }
        } else if t == Task::ENTITY_NAME_STATIC {
            apply(&mut w.tasks, ev);
        } else if t == StatusChange::ENTITY_NAME_STATIC {
            apply(&mut status_changes, ev);
        } else if t == Dependency::ENTITY_NAME_STATIC {
            apply(&mut w.deps, ev);
        } else if t == Claim::ENTITY_NAME_STATIC {
            apply(&mut w.claims, ev);
        } else if t == Comment::ENTITY_NAME_STATIC {
            apply(&mut comments, ev);
        } else if t == CommitFact::ENTITY_NAME_STATIC {
            apply(&mut w.commit_facts, ev);
        } else if t == RefFact::ENTITY_NAME_STATIC {
            apply(&mut w.ref_facts, ev);
        }
        // Unknown item types are skipped: forward compatibility with newer levi.
    }

    w.status_changes = status_changes.into_values().collect();
    w.status_changes
        .sort_by(|a, b| (a.at.as_str(), &*a.id.0).cmp(&(b.at.as_str(), &*b.id.0)));
    w.comments = comments.into_values().collect();
    w.comments
        .sort_by(|a, b| (a.at.as_str(), &*a.id.0).cmp(&(b.at.as_str(), &*b.id.0)));
    w
}

/// SET upserts by entity id; DEL removes (tombstone applies even when the
/// item was never seen — events replay in deterministic order, so this
/// converges on every node).
fn apply<T>(map: &mut BTreeMap<String, T>, ev: &MEvent)
where
    T: Eventable,
{
    match ev.change_type {
        MEventType::SET => {
            if let Ok(item) = serde_json::from_value::<T>(ev.item.clone()) {
                map.insert(item.id().to_string(), item);
            }
        }
        MEventType::DEL => {
            if let Some(id) = ev.item.get("id").and_then(|v| v.as_str()) {
                map.remove(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec<T: myko::prelude::Eventable>(oid: &str, item: &T, created_at: &str) -> EventRecord {
        let mut event = MEvent::from_item(item, MEventType::SET, "m");
        event.created_at = created_at.to_string();
        EventRecord { id: oid.to_string(), event }
    }

    fn del<T: myko::prelude::Eventable>(oid: &str, item: &T, created_at: &str) -> EventRecord {
        let mut event = MEvent::from_item(item, MEventType::DEL, "m");
        event.created_at = created_at.to_string();
        EventRecord { id: oid.to_string(), event }
    }

    fn task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            project_id: "p".into(),
            title: title.into(),
            body: String::new(),
            priority: Priority::default(),
            labels: vec![],
            created_by_dev: "d".into(),
            created_by_machine: "m".into(),
            created_at: "2026-07-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn lww_by_created_at_regardless_of_arrival_order() {
        // The later edit arrives first in the vec; sorting must fix it.
        let w = materialize(vec![
            rec("bb", &task("t1", "newer title"), "2026-07-02T00:00:00Z"),
            rec("aa", &task("t1", "older title"), "2026-07-01T00:00:00Z"),
        ]);
        assert_eq!(w.tasks["t1"].title, "newer title");
    }

    #[test]
    fn created_at_tie_broken_by_event_id() {
        let at = "2026-07-01T00:00:00Z";
        let w = materialize(vec![
            rec("ff", &task("t1", "id ff wins"), at),
            rec("aa", &task("t1", "id aa loses"), at),
        ]);
        assert_eq!(w.tasks["t1"].title, "id ff wins");
    }

    #[test]
    fn del_removes_dependency() {
        let d = Dependency {
            id: dependency_id("a", "b").into(),
            project_id: "p".into(),
            blocker_task_id: "a".into(),
            blocked_task_id: "b".into(),
        };
        let w = materialize(vec![
            rec("aa", &d, "2026-07-01T00:00:00Z"),
            del("bb", &d, "2026-07-02T00:00:00Z"),
        ]);
        assert!(w.deps.is_empty());
    }

    #[test]
    fn claim_liveness_respects_ttl() {
        let c = Claim {
            id: "t1".into(),
            project_id: "p".into(),
            task_id: "t1".into(),
            dev: "d".into(),
            machine: "m".into(),
            worktree: "/w".into(),
            at: "2026-07-01T00:00:00Z".into(),
            ttl_secs: 3600,
        };
        let w = materialize(vec![rec("aa", &c, "2026-07-01T00:00:00Z")]);
        let live = "2026-07-01T00:30:00Z".parse::<DateTime<Utc>>().unwrap();
        let dead = "2026-07-01T02:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(w.live_claim("t1", live).is_some());
        assert!(w.live_claim("t1", dead).is_none());
    }

    #[test]
    fn project_and_sorted_append_only_sets() {
        let p = Project { id: "p".into(), name: "levi".into(), created_at: "2026-07-01T00:00:00Z".into() };
        let c2 = Comment { id: "c2".into(), project_id: "p".into(), task_id: "t1".into(), body: "second".into(), by_dev: "d".into(), at: "2026-07-02T00:00:00Z".into() };
        let c1 = Comment { id: "c1".into(), project_id: "p".into(), task_id: "t1".into(), body: "first".into(), by_dev: "d".into(), at: "2026-07-01T00:00:00Z".into() };
        let w = materialize(vec![
            rec("cc", &c2, "2026-07-02T00:00:00Z"),
            rec("aa", &p, "2026-07-01T00:00:00Z"),
            rec("bb", &c1, "2026-07-01T00:00:00Z"),
        ]);
        assert_eq!(w.project.as_ref().unwrap().name, "levi");
        let bodies: Vec<_> = w.comments.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, ["first", "second"]);
    }

    #[test]
    fn unknown_item_type_is_ignored() {
        let mut event = MEvent::from_item(&task("t1", "x"), MEventType::SET, "m");
        event.item_type = "FutureThing".into();
        let w = materialize(vec![EventRecord { id: "aa".into(), event }]);
        assert!(w.tasks.is_empty());
    }
}
