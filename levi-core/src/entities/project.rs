use myko::prelude::*;

/// Project identity, minted by `levi init`. id = project UUID (32 hex chars),
/// stored in the first event on the ref.
#[myko_item]
pub struct Project {
    pub name: String,
    /// RFC3339
    pub created_at: String,
}
