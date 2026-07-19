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
            "levi watch needs a hub: set one with `git config levi.hub <host:port>` \
             (or [hub] address in ~/.config/levi/config.toml)"
        );
    };
    let project_id = ctx.project_id()?;
    let session = HubSession::connect(&addr, Duration::from_secs(10))?;

    // History size first: everything up to this count is history, not news.
    let mut filter = levi_core::PartialLogEntry::default();
    filter.project_id = Some(project_id.clone());
    let history: levi_core::LogEntryCount = session.report_once(
        levi_core::CountLogEntrys(filter.clone()),
        Duration::from_secs(10),
    )?;
    let cell = session
        .client
        .watch_query(levi_core::GetLogEntrysByQuery(filter));

    let (tx, rx) = mpsc::channel();
    let _guard = cell.subscribe(move |sig| {
        if let Signal::Value(entries) = sig {
            let _ = tx.send(entries.to_vec());
        }
    });

    eprintln!("watching (ctrl-c to stop)…");
    let mut seen: HashSet<String> = HashSet::new();
    let mut synced = history.count == 0;
    while let Ok(entries) = rx.recv() {
        if !synced {
            // Swallow the seed + initial snapshot silently.
            for entry in &entries {
                seen.insert(entry.id.to_string());
            }
            synced = entries.len() >= history.count;
            continue;
        }
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
