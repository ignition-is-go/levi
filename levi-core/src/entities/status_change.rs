use myko::prelude::*;

#[myko_subtype(derive(Eq, Copy))]
pub enum StatusKind {
    Closed,
    Reopened,
}

/// Append-only status-change record; never edited. Effective status of a task
/// on a checkout is a fold over these, restricted to changes whose
/// `anchor_commit` is an ancestor of HEAD (see `resolve::effective_status`).
#[myko_item]
pub struct StatusChange {
    pub project_id: String,
    pub task_id: String,
    pub to_status: StatusKind,
    /// Full commit sha (hex). None = unanchored: applies on every checkout.
    #[serde(default)]
    pub anchor_commit: Option<String>,
    /// RFC3339
    pub created: String,
    pub by_dev: String,
    pub by_machine: String,
}
