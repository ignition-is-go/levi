use myko::prelude::*;

/// Advisory claim. id = task_id, so a SET overwrites any prior claim and
/// "newest wins" falls out of LWW replay order. Ignored after ttl expires.
#[myko_item]
pub struct Claim {
    pub project_id: String,
    pub task_id: String,
    pub dev: String,
    /// Display name (hostname). Identity comparisons use `machine_id`.
    pub machine: String,
    /// Minted per-machine UUID (empty on legacy events — compare by
    /// `machine` display name then).
    #[serde(default)]
    pub machine_id: String,
    pub worktree: String,
    /// RFC3339
    pub created: String,
    pub ttl_secs: u64,
}
