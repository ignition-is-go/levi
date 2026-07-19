use myko::prelude::*;

/// Commit-graph slice: sha -> parent shas. id = commit sha (hex) — content
/// addressed and immutable (a sha's parents never change), so publication is
/// idempotent. Lets git-free nodes (the hub) resolve ancestry.
#[myko_item]
pub struct CommitFact {
    pub project_id: String,
    pub parents: Vec<String>,
}
