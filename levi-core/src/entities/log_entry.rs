use myko::prelude::*;

/// Hub transport wrapper for one levi event. id = the event's content address
/// (git blob OID of its CBOR bytes); payload = base64(CBOR(MEvent)). Immutable
/// and add-only, so "what are you missing" between a CLI and the hub is a set
/// difference over LogEntry ids. A hub-side saga unwraps the inner event so
/// dashboards query real entities.
#[myko_item]
pub struct LogEntry {
    pub project_id: String,
    pub cbor_b64: String,
    /// RFC3339 of the inner event (for activity-feed ordering).
    pub created_at: String,
}
