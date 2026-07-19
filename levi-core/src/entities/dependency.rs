use myko::prelude::*;

/// blocker blocks blocked. id = "{blocker_task_id}->{blocked_task_id}" —
/// deterministic, so `dep add` is idempotent and `dep rm` is a plain DEL.
#[myko_item]
pub struct Dependency {
    pub project_id: String,
    pub blocker_task_id: String,
    pub blocked_task_id: String,
}

pub fn dependency_id(blocker: &str, blocked: &str) -> String {
    format!("{blocker}->{blocked}")
}
