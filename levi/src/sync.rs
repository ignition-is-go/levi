//! Sync legs (spec §Sync). The git leg lives in `commands::sync_cmd`; this
//! module holds the hub leg (event-diff exchange) and drives the facts leg.
//! `opportunistic` is the best-effort background sync every mutating command
//! attempts on exit.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use levi_core::LogEntry;
use myko::wire::{MEvent, MEventType};

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;
use crate::store::EventStore;

/// Hub leg: exchange event diffs with the configured hub, then publish
/// commit-graph facts. `Ok(None)` when no hub is configured.
///
/// Content-addressed ids make "what are you missing" a cheap set difference
/// (spec): we compare LogEntry ids (event blob OIDs) both ways. The naive
/// full-snapshot exchange is fine for v1; Merkle-style comparison is a later
/// optimization (spec blesses this).
pub fn hub_leg(ctx: &LeviCtx) -> Result<Option<String>> {
    let Some(addr) = ctx.config.hub.clone() else { return Ok(None) };
    let project_id = ctx.project_id()?;
    let timeout = Duration::from_secs(10);

    let session = HubSession::connect(&addr, ctx.config.token.as_deref(), timeout)?;

    // What does the hub have for this project?
    let hub_entries = session.log_entries(&project_id, timeout)?;
    let hub_ids: HashSet<String> = hub_entries.iter().map(|e| e.id.to_string()).collect();

    // Push: local events the hub lacks, wrapped as LogEntries.
    let local = ctx.store.read()?;
    let local_ids: HashSet<String> = local.iter().map(|r| r.id.clone()).collect();
    let mut push = Vec::new();
    for record in &local {
        if !hub_ids.contains(&record.id) {
            let bytes = EventStore::encode(&record.event);
            let entry =
                LogEntry::wrap(&record.id, &project_id, &bytes, &record.event.created_at);
            push.push(MEvent::from_item(&entry, MEventType::SET, &ctx.identity.machine));
        }
    }
    let pushed = push.len();
    session.send_events(push)?;

    // Pull: hub events we lack; unwrap and append (content addressing dedups
    // and re-derives the id from the bytes — a tampered id cannot smuggle a
    // mismatched event into the ref).
    let mut pull = Vec::new();
    for entry in &hub_entries {
        if !local_ids.contains(&*entry.id.0) {
            match entry.unwrap_event() {
                Ok(event) => pull.push(event),
                Err(e) => eprintln!("warning: skipping undecodable hub event {}: {e}", entry.id.0),
            }
        }
    }
    let pulled = pull.len();
    if !pull.is_empty() {
        ctx.store.append(&pull)?;
    }

    // Facts leg (spec leg 3) rides the same session.
    let facts = crate::facts::publish(ctx, &session)?;

    session.close();
    Ok(Some(format!("pushed {pushed}, pulled {pulled} event(s), published {facts} fact(s)")))
}

/// Best-effort, silent. No hub configured (or `--no-sync`) ⇒ no-op.
pub fn opportunistic(ctx: &LeviCtx) {
    if ctx.config.hub.is_none() || ctx.world.project.is_none() {
        return;
    }
    if let Err(e) = hub_leg(ctx) {
        log::debug!("opportunistic hub sync failed: {e:#}");
    }
}
