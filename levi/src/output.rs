//! Rendering: human lines and stable, versioned `--json` schemas.

use chrono::{DateTime, Utc};
use levi_core::Task;
use levi_core::ids::short_id;
use levi_core::materialize::World;
use levi_core::resolve::ResolvedStatus;
use serde_json::{Value, json};

pub const SCHEMA_LS: &str = "levi.ls/1";
pub const SCHEMA_SHOW: &str = "levi.show/1";
pub const SCHEMA_NEXT: &str = "levi.next/1";
pub const SCHEMA_ADD: &str = "levi.add/1";
pub const SCHEMA_WATCH: &str = "levi.watch/1";

pub fn claim_json(world: &World, task_id: &str, now: DateTime<Utc>) -> Value {
    match world.live_claim(task_id, now) {
        Some(c) => json!({
            "dev": c.dev, "machine": c.machine, "worktree": c.worktree,
            "at": c.created, "ttl_secs": c.ttl_secs,
        }),
        None => Value::Null,
    }
}

pub fn task_json(world: &World, task: &Task, status: &ResolvedStatus, now: DateTime<Utc>) -> Value {
    let id = task.id.to_string();
    json!({
        "id": id,
        "short": short_id(world, &id),
        "title": task.title,
        "status": status.status.label(),
        "resolution": status.resolution.label(),
        "priority": task.priority.label(),
        "labels": task.labels,
        "created_at": task.created,
        "created_by": task.created_by_dev,
        "claim": claim_json(world, &id, now),
    })
}

pub fn task_line(
    world: &World,
    task: &Task,
    status: &ResolvedStatus,
    now: DateTime<Utc>,
) -> String {
    let id = task.id.to_string();
    let mut line = format!(
        "{:<12} {:<2} {:<7} {}",
        short_id(world, &id),
        task.priority.label(),
        status.status.label(),
        task.title
    );
    if !task.labels.is_empty() {
        line.push_str(&format!("  [{}]", task.labels.join(", ")));
    }
    if let Some(c) = world.live_claim(&id, now) {
        line.push_str(&format!("  (claimed by {})", c.dev));
    }
    if status.resolution == levi_core::resolve::Resolution::Partial {
        line.push_str("  ⚠ unknown anchor");
    }
    line
}
