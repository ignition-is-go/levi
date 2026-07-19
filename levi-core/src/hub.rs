//! Hub-side machinery: unwrap incoming LogEntries into real entities so
//! aggregate queries and the dashboard see Tasks/StatusChanges/etc.
//! (spec §Sync, hub leg).
//!
//! Note this command never inspects entity types itself — `emit_event_batch`
//! routes by `item_type` exactly like direct ingestion. The command exists
//! because the hub keeps the *journal* (LogEntries carry the original
//! content-addressed bytes the pull leg needs), and because it is the
//! interposition point for LWW ordering: myko applies raw events in arrival
//! order, so without the [`Applied`] guard an event synced late would
//! clobber newer state that every CLI (which sorts the full log before
//! replay) resolves correctly.

use std::sync::Arc;

use myko::command::{CommandContext, CommandError, CommandHandler};
use myko::prelude::*;
use myko::saga::{SagaContext, SagaHandler};
use myko::wire::MEventType;

use crate::entities::{Applied, GetAppliedsByIds, LogEntry, applied_id};

/// Apply the event wrapped inside a LogEntry to this node's stores, unless a
/// newer event for the same entity was already applied (LWW by
/// (created_at, event id), the same key `materialize()` sorts by).
/// Idempotent: re-applying converges.
#[myko_command]
pub struct ApplyLogEntry {
    pub cbor_b64: String,
    /// The LogEntry id — the event's content address (git blob OID).
    pub event_oid: String,
}

impl CommandHandler for ApplyLogEntry {
    fn execute(self, ctx: CommandContext) -> Result<(), CommandError> {
        use base64::Engine as _;
        let err = |m: String| CommandError {
            tx: ctx.tx().to_string(),
            command_id: ctx.command_id.to_string(),
            message: m,
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.cbor_b64)
            .map_err(|e| err(format!("invalid base64 in LogEntry: {e}")))?;
        let event: myko::wire::MEvent = ciborium::from_reader(bytes.as_slice())
            .map_err(|e| err(format!("invalid CBOR in LogEntry: {e}")))?;
        // Never let a wrapped LogEntry apply another LogEntry: no recursion.
        if event.item_type == LogEntry::ENTITY_NAME_STATIC {
            return Err(err("refusing to unwrap a nested LogEntry".into()));
        }
        let Some(item_id) = event.item.get("id").and_then(|v| v.as_str()) else {
            return Err(err("wrapped event has no item id".into()));
        };

        // LWW guard: skip events older than what this entity already shows.
        let guard_id = applied_id(&event.item_type, item_id);
        let existing = ctx.exec_query_first(GetAppliedsByIds {
            ids: vec![guard_id.clone().into()],
        })?;
        let incoming = (event.created_at.clone(), self.event_oid.clone());
        if let Some(applied) = existing
            && (applied.event_created.as_str(), applied.event_oid.as_str())
                > (incoming.0.as_str(), incoming.1.as_str())
        {
            return Ok(()); // stale: a newer event was already applied
        }

        ctx.emit_event_batch(vec![event])?;
        ctx.emit_set(&Applied {
            id: guard_id.into(),
            event_created: incoming.0,
            event_oid: incoming.1,
        })?;
        Ok(())
    }
}

myko::register_command_handler!(ApplyLogEntry);

/// Saga: every LogEntry SET (from CLI pushes or Postgres replay) is unwrapped
/// into its inner event.
#[myko_saga]
pub struct UnwrapLogEntries;

impl SagaHandler for UnwrapLogEntries {
    type EventItem = LogEntry;
    type Command = ApplyLogEntry;
    const EVENT_TYPE: MEventType = MEventType::SET;

    fn handle(
        item: LogEntry,
        _event: myko::wire::MEvent,
        _ctx: Arc<SagaContext>,
    ) -> Option<ApplyLogEntry> {
        Some(ApplyLogEntry {
            cbor_b64: item.cbor_b64,
            event_oid: item.id.to_string(),
        })
    }
}
