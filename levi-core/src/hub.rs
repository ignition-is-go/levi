//! Hub-side machinery: unwrap incoming LogEntries into real entities so
//! aggregate queries and the dashboard see Tasks/StatusChanges/etc.
//! (spec §Sync, hub leg).

use std::sync::Arc;

use myko::command::{CommandContext, CommandError, CommandHandler};
use myko::prelude::*;
use myko::saga::{SagaContext, SagaHandler};
use myko::wire::MEventType;

use crate::entities::LogEntry;

/// Apply the event wrapped inside a LogEntry to this node's stores.
/// Idempotent: re-applying a SET converges (LWW overwrite).
#[myko_command]
pub struct ApplyLogEntry {
    pub cbor_b64: String,
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
        ctx.emit_event_batch(vec![event])?;
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
        })
    }
}
