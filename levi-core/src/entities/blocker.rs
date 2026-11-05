use myko::prelude::*;

/// Something a task is waiting on that isn't another task: "waiting on the
/// operator to open port 6300". id = uuid. `unblock` closes it with a later
/// SET of the same id carrying the `closed_*` fields, so the log keeps both.
/// A task with an open blocker isn't eligible for `next`.
#[myko_item]
pub struct Blocker {
    pub project_id: String,
    pub task_id: String,
    #[searchable]
    pub reason: String,
    pub by_dev: String,
    /// Display name (hostname).
    pub by_machine: String,
    /// RFC3339
    pub created: String,
    /// Who closed it, when it's closed.
    #[serde(default)]
    pub closed_by: Option<String>,
    /// Why it no longer blocks.
    #[serde(default)]
    pub closed_reason: Option<String>,
    /// RFC3339
    #[serde(default)]
    pub closed_at: Option<String>,
}

impl Blocker {
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }
}
