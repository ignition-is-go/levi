use myko::prelude::*;
use sha2::{Digest, Sha256};

/// A request that someone read a task at `commit` before it's merged. One per
/// requester per task: the id is derived from the task and the requester, so
/// asking again replaces the earlier request, and `withdraw-read` deletes it
/// (DEL), as `drop` deletes a claim. A Review of the task made after the
/// request answers it; see `reads::read_request_states`.
#[myko_item]
pub struct ReadRequest {
    pub project_id: String,
    pub task_id: String,
    /// Full sha of the commit to read.
    pub commit: String,
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

/// The id of `requester`'s read request on `task_id`: the task id, then a
/// short hash of the requester's (dev, machine, worktree), so paths and
/// emails stay out of the id. `machine` is the machine id when there is one,
/// else the hostname, as claims compare them.
pub fn read_request_id(task_id: &str, dev: &str, machine: &str, worktree: &str) -> String {
    let mut h = Sha256::new();
    for part in [dev, machine, worktree] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    let digest = h.finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{task_id}:{hex}")
}
