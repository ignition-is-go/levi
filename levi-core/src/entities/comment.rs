use myko::prelude::*;

/// Append-only comment. id = uuid.
#[myko_item]
pub struct Comment {
    pub project_id: String,
    pub task_id: String,
    #[searchable]
    pub body: String,
    pub by_dev: String,
    /// RFC3339
    pub at: String,
}
