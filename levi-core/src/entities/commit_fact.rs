use myko::prelude::*;

/// Commit-graph slice: sha -> parent shas. id = commit sha (hex) — content
/// addressed and immutable (a sha's parents never change), so publication is
/// idempotent. Lets git-free nodes (the hub) resolve ancestry.
#[myko_item]
pub struct CommitFact {
    pub project_id: String,
    pub parents: Vec<String>,
    /// `git patch-id --stable` of this commit's diff, if computed. None for
    /// empty-diff/merge commits and commits outside the publish window. Lets
    /// git-free nodes resolve squash/rebase merges (spec 2026-07-22).
    #[serde(default)]
    pub patch_id: Option<String>,
}
