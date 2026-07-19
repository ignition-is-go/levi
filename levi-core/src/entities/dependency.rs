use myko::prelude::*;

/// blocker blocks blocked. id = "{blocker_task_id}->{blocked_task_id}" —
/// deterministic, so `dep add` is idempotent and `dep rm` is a plain DEL.
#[myko_item]
pub struct Dependency {
    pub project_id: String,
    pub blocker_task_id: String,
    pub blocked_task_id: String,
    /// Set when the blocker lives in another project (cross-project dep).
    #[serde(default)]
    pub blocker_project_id: Option<String>,
    /// Consumption branch for cross-project resolution; None = the foreign
    /// project's default branch.
    #[serde(default)]
    pub blocker_ref: Option<String>,
    /// Agent-authored note on *how* the dependency is consumed (e.g.
    /// "cargo: crates.io myko ^4.24"). levi never parses it.
    #[serde(default)]
    pub via: Option<String>,
}

pub fn dependency_id(blocker: &str, blocked: &str) -> String {
    format!("{blocker}->{blocked}")
}

/// Deterministic id for a cross-project dependency.
pub fn foreign_dependency_id(blocker_project: &str, blocker: &str, blocked: &str) -> String {
    format!("{blocker_project}/{blocker}->{blocked}")
}

/// The ladder/cache key for a foreign blocker.
pub fn foreign_key(project_id: &str, task_id: &str) -> String {
    format!("{project_id}/{task_id}")
}
