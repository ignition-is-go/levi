mod applied;
mod claim;
mod comment;
mod commit_fact;
mod dependency;
mod log_entry;
mod project;
mod ref_fact;
mod status_change;
mod task;

pub use applied::*;
pub use claim::*;
pub use comment::*;
pub use commit_fact::*;
pub use dependency::*;
pub use log_entry::*;
pub use project::*;
pub use ref_fact::*;
pub use status_change::*;
pub use task::*;

#[cfg(test)]
mod tests {
    use super::*;
    use myko::wire::{MEvent, MEventType};

    #[test]
    fn task_event_cbor_roundtrip() {
        let t = Task {
            id: "abc123".into(),
            project_id: "p".into(),
            title: "T".into(),
            body: String::new(),
            priority: Priority::P1,
            labels: vec!["x".into()],
            created_by_dev: "d@e".into(),
            created_by_machine: "m".into(),
            created: "2026-07-18T00:00:00Z".into(),
        };
        let ev = MEvent::from_item(&t, MEventType::SET, "m");
        let mut buf = Vec::new();
        ciborium::into_writer(&ev, &mut buf).unwrap();
        let back: MEvent = ciborium::from_reader(buf.as_slice()).unwrap();
        let t2: Task = serde_json::from_value(back.item.clone()).unwrap();
        assert_eq!(t, t2);
        assert_eq!(back.item_type.as_ref(), "Task");
    }

    #[test]
    fn priority_orders_p0_first() {
        assert!(Priority::P0.rank() < Priority::P3.rank());
        assert_eq!(Priority::default(), Priority::P2);
    }
}
