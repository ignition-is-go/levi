//! `levi watch` — the long-running exception: holds a live myko subscription
//! to the hub and streams this project's events as they arrive (spec
//! §Architecture). Requires a configured hub (spec deviation 8).

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, bail};
use myko::hyphae::{Signal, Watchable};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;
use crate::output::SCHEMA_WATCH;

pub fn run(ctx: &LeviCtx, json: bool) -> Result<()> {
    let Some(addr) = ctx.config.hub.clone() else {
        bail!(
            "levi watch needs a hub: run `levi onboard --hub <host:port>` \
             (writes .levi/config.toml), or set [hub] address in \
             ~/.config/levi/config.toml"
        );
    };
    let project_id = ctx.project_id()?;
    let session = HubSession::connect(&addr, Duration::from_secs(10))?;

    // History = the explicit id set fetched up-front (count-marked, so the
    // snapshot is complete). Anything not in it is news — including events
    // that land while we're still catching up, which a count-based
    // "synced" heuristic would silently swallow.
    let seen_init: HashSet<String> = session
        .log_entries(&project_id, Duration::from_secs(10))?
        .iter()
        .map(|e| e.id.to_string())
        .collect();

    let filter = levi_core::PartialLogEntry {
        project_id: Some(project_id.clone()),
        ..Default::default()
    };
    let cell = session
        .client
        .watch_query(levi_core::GetLogEntrysByQuery(filter));

    let (tx, rx) = mpsc::channel();
    let guard_tx = tx.clone();
    let _guard = cell.subscribe(move |sig| {
        if let Signal::Value(entries) = sig {
            let _ = guard_tx.send(entries.to_vec());
        }
    });
    // Prime with the cell's current value: anything that raced in between
    // the history fetch and the subscription is caught here.
    let _ = tx.send(myko::hyphae::Gettable::get(&cell).to_vec());

    eprintln!("watching (ctrl-c to stop)…");
    let mut seen = seen_init;
    while let Ok(entries) = rx.recv() {
        let mut batch: Vec<_> = entries
            .iter()
            .filter(|e| !seen.contains(&*e.id.0))
            .cloned()
            .collect();
        batch.sort_by(|a, b| a.created.cmp(&b.created));
        for entry in batch {
            seen.insert(entry.id.to_string());
            if entry.project_id != project_id {
                continue;
            }
            let Ok(event) = entry.unwrap_event() else {
                continue;
            };
            if json {
                println!(
                    "{}",
                    json!({
                        "schema": SCHEMA_WATCH,
                        "id": entry.id.0.as_ref(),
                        "item_type": event.item_type,
                        "change_type": format!("{:?}", event.change_type),
                        "created_at": event.created_at,
                        "item": event.item,
                    })
                );
            } else {
                let title = event
                    .item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .map(|t| format!(" {t:?}"))
                    .unwrap_or_default();
                println!("{} {:?}{}", event.item_type, event.change_type, title);
            }
        }
    }
    Ok(())
}
