use myko::prelude::*;

/// Observed branch head. id = "{project_id}:{branch}" — LWW: the newest
/// observation wins.
#[myko_item]
pub struct RefFact {
    pub project_id: String,
    pub branch: String,
    /// Head commit sha (hex).
    pub head: String,
    /// RFC3339
    pub observed: String,
}

pub fn ref_fact_id(project_id: &str, branch: &str) -> String {
    format!("{project_id}:{branch}")
}
