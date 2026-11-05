//! Rendering: human lines and stable, versioned `--json` schemas.

use chrono::{DateTime, Utc};
use levi_core::Task;

use crate::ancestors::ElsewhereClose;
use levi_core::ids::short_id;
use levi_core::materialize::World;
use levi_core::resolve::ResolvedStatus;
use serde_json::{Value, json};

pub const SCHEMA_LS: &str = "levi.ls/1";
pub const SCHEMA_SHOW: &str = "levi.show/1";
pub const SCHEMA_NEXT: &str = "levi.next/1";
pub const SCHEMA_ADD: &str = "levi.add/1";
pub const SCHEMA_WATCH: &str = "levi.watch/1";
pub const SCHEMA_CHECK_CLAIMS: &str = "levi.check-claims/1";

pub fn claim_json(world: &World, task_id: &str, now: DateTime<Utc>) -> Value {
    match world.live_claim(task_id, now) {
        Some(c) => json!({
            "dev": c.dev, "machine": c.machine, "worktree": c.worktree,
            "git_ref": if c.git_ref.is_empty() { Value::Null } else { json!(c.git_ref) },
            "at": c.created, "ttl_secs": c.ttl_secs,
        }),
        None => Value::Null,
    }
}

pub fn task_json(
    world: &World,
    task: &Task,
    status: &ResolvedStatus,
    now: DateTime<Utc>,
    elsewhere: &[ElsewhereClose],
) -> Value {
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
        "closed_elsewhere": elsewhere
            .iter()
            .map(|e| json!({
                "anchor": e.anchor,
                "at": e.at,
                "by": e.by,
                "branches": e.branches,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One-line human hint for a fix that exists on other ancestry.
pub fn elsewhere_hint(elsewhere: &[ElsewhereClose]) -> Option<String> {
    let first = elsewhere.first()?;
    let place = if first.branches.is_empty() {
        format!("@ {}", &first.anchor[..8.min(first.anchor.len())])
    } else {
        format!(
            "on {} @ {}",
            first.branches.join(", "),
            &first.anchor[..8.min(first.anchor.len())]
        )
    };
    Some(format!("fix exists {place}"))
}

pub fn task_line(
    world: &World,
    task: &Task,
    status: &ResolvedStatus,
    now: DateTime<Utc>,
    elsewhere: &[ElsewhereClose],
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
    match status.resolution {
        levi_core::resolve::Resolution::Partial => line.push_str("  ⚠ unknown anchor"),
        levi_core::resolve::Resolution::Squashed => line.push_str("  ~ squashed"),
        _ => {}
    }
    if let Some(hint) = elsewhere_hint(elsewhere) {
        line.push_str(&format!("  ({hint} — consider merging)"));
    }
    line
}

/// A task's review state on one checkout: the newest review whose commit is
/// in this checkout's ancestry, and whether the verified commit differs from
/// the commit the task closed at here.
pub struct ReviewSummary {
    pub state: levi_core::resolve::ReviewState,
    pub resolution: levi_core::resolve::Resolution,
    pub differs: Option<levi_core::resolve::ReviewDiffers>,
    /// The newest applying review's note.
    pub note: Option<String>,
}

pub fn review_summary(
    world: &World,
    task_id: &str,
    anc: &mut impl levi_core::resolve::AncestorSet,
) -> ReviewSummary {
    use levi_core::resolve::{
        Resolution, applying_reviews, closing_anchor, review_differs, review_state,
    };
    let reviews = world.reviews_for(task_id);
    let (applying, resolution) = applying_reviews(&reviews, anc, Resolution::Exact);
    let closed = closing_anchor(&world.changes_for(task_id), anc);
    ReviewSummary {
        state: review_state(&applying),
        resolution,
        differs: review_differs(closed, &applying),
        note: applying.last().map(|(r, _)| r.note.clone()),
    }
}

/// Add the review fields to a task's JSON. The verdict is labelled
/// `declared`: levi's identity is self-declared, so a review says who claims
/// to have read what, not proof of it. `review_differs` is worked out from
/// git alone, so it is labelled `observed`.
pub fn add_review_fields(v: &mut Value, summary: &ReviewSummary) {
    use levi_core::resolve::ReviewState;
    v["review"] = json!(summary.state.label());
    v["review_source"] = match summary.state {
        ReviewState::None => Value::Null,
        _ => json!("declared"),
    };
    v["review_resolution"] = json!(summary.resolution.label());
    v["review_differs"] = match &summary.differs {
        Some(d) => json!({"verified": d.verified, "closed": d.closed, "source": "observed"}),
        None => Value::Null,
    };
}

/// A short human marker for `ls`.
pub fn review_marker(summary: &ReviewSummary) -> &'static str {
    use levi_core::resolve::ReviewState;
    match (summary.state, summary.differs.is_some()) {
        (ReviewState::Verified, false) => "  ✓ verified",
        (ReviewState::Verified, true) => "  ✓ verified (at another commit)",
        (ReviewState::SentBack, _) => "  ↩ sent back",
        (ReviewState::None, _) => "",
    }
}
