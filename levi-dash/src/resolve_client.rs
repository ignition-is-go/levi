//! Per-branch status resolution in the browser: the same fold as the CLI,
//! over the CommitFact graph (levi-core `FactsAncestors`), so the project
//! browser can view any project's tasks as resolved against any branch.

use std::collections::BTreeMap;
use std::sync::Arc;

use levi_core::resolve::{FactsAncestors, Resolution, ResolvedStatus, effective_status};
use levi_core::{CommitFact, StatusChange, Task};

/// task id -> resolved status for one project against one head sha.
pub fn statuses(
    tasks: &[Arc<Task>],
    changes: &[Arc<StatusChange>],
    facts: &[Arc<CommitFact>],
    project_id: &str,
    head: Option<&str>,
) -> BTreeMap<String, ResolvedStatus> {
    let fact_map: BTreeMap<String, CommitFact> = facts
        .iter()
        .filter(|f| f.project_id == project_id)
        .map(|f| (f.id.to_string(), (**f).clone()))
        .collect();
    let mut sorted: Vec<&StatusChange> = changes
        .iter()
        .map(|c| &**c)
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
                // No known head: only unanchored changes can be judged.
                None => {
                    let mut unknown = NoHead;
                    effective_status(&mine, &mut unknown, Resolution::Partial)
                }
            };
            (id, resolved)
        })
        .collect()
}

struct NoHead;

impl levi_core::resolve::AncestorSet for NoHead {
    fn contains(&mut self, _sha: &str) -> levi_core::resolve::Ancestry {
        levi_core::resolve::Ancestry::Unknown
    }
}

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Is this claim still live? (at + ttl vs the browser clock.)
pub fn claim_live(claim: &levi_core::Claim) -> bool {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(&claim.created) else {
        return false;
    };
    let expires_ms = at.timestamp_millis() as f64 + (claim.ttl_secs as f64) * 1000.0;
    expires_ms > now_ms()
}

/// Git-style short id for the dashboard: `lv-` + the shortest prefix (min
/// 4 hex) unique among `all_ids` — mirroring the CLI's `levi_core::ids::short_id`.
pub fn short_id(all_ids: &[String], id: &str) -> String {
    let mut len = 4;
    while len < id.len() {
        let prefix = &id[..len];
        if !all_ids.iter().any(|o| o != id && o.starts_with(prefix)) {
            return format!("lv-{prefix}");
        }
        len += 2;
    }
    format!("lv-{id}")
}
