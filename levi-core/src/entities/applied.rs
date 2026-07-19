use myko::prelude::*;

/// Hub-internal LWW guard: the newest event applied per entity.
/// id = "{item_type}:{item_id}". `ApplyLogEntry` consults this before
/// emitting an unwrapped event so that out-of-arrival-order events cannot
/// clobber newer state (CLIs get the same guarantee by sorting the full log
/// before replay; the hub applies incrementally and needs the marker).
#[myko_item]
pub struct Applied {
    /// (created_at, event id) of the newest event applied for this entity —
    /// the same LWW sort key `materialize()` uses.
    pub event_created: String,
    pub event_oid: String,
}

pub fn applied_id(item_type: &str, item_id: &str) -> String {
    format!("{item_type}:{item_id}")
}
