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

impl LogEntry {
    /// Wrap an event for hub transport. `event_id` must be the event's
    /// content address (git blob OID of `cbor_bytes`).
    pub fn wrap(event_id: &str, project_id: &str, cbor_bytes: &[u8], created_at: &str) -> Self {
        use base64::Engine as _;
        LogEntry {
            id: event_id.into(),
            project_id: project_id.into(),
            cbor_b64: base64::engine::general_purpose::STANDARD.encode(cbor_bytes),
            created_at: created_at.into(),
        }
    }

    /// Decode back to raw CBOR bytes.
    pub fn cbor_bytes(&self) -> anyhow::Result<Vec<u8>> {
        use base64::Engine as _;
        Ok(base64::engine::general_purpose::STANDARD.decode(&self.cbor_b64)?)
    }

    /// Decode the inner wire event.
    pub fn unwrap_event(&self) -> anyhow::Result<myko::wire::MEvent> {
        Ok(ciborium::from_reader(self.cbor_bytes()?.as_slice())?)
    }
}
