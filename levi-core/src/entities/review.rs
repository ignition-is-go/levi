use myko::prelude::*;

#[myko_subtype(derive(Eq, Copy))]
pub enum ReviewVerdict {
    Verified,
    SentBack,
}

impl ReviewVerdict {
    pub fn label(self) -> &'static str {
        match self {
            ReviewVerdict::Verified => "verified",
            ReviewVerdict::SentBack => "sent_back",
        }
    }
}

/// Someone's verdict on a task, having read it at `commit`. Append-only;
/// id = uuid. Like a StatusChange it resolves by ancestry: a review applies
/// on a checkout when `commit` is an ancestor of HEAD. The verdict is
/// declared by the reviewer; levi doesn't check what was read.
#[myko_item]
pub struct Review {
    pub project_id: String,
    pub task_id: String,
    pub verdict: ReviewVerdict,
    /// Full sha of the commit the reviewer read.
    pub commit: String,
    /// What was checked, or what's missing.
    #[serde(default)]
    #[searchable]
    pub note: String,
    pub by_dev: String,
    /// Display name (hostname).
    pub by_machine: String,
    /// Minted machine id; empty on hosts without levi state.
    #[serde(default)]
    pub by_machine_id: String,
    pub by_worktree: String,
    /// RFC3339
    pub created: String,
}
