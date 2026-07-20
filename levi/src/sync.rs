//! Sync legs (spec §Sync): the git leg (fetch/union/push of the events
//! ref), the hub leg (event-diff exchange), and the facts leg it drives.
//! `opportunistic` is the detached background sync every mutating command
//! fires on exit — issues reach the git remote and hub without anyone
//! thinking about syncing.

use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use levi_core::LogEntry;
use levi_core::materialize::materialize;
use myko::wire::{MEvent, MEventType};

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;
use crate::store::{EVENTS_REF, EventStore};

/// Hub leg: exchange event diffs with the configured hub, then publish
/// commit-graph facts. `Ok(None)` when no hub is configured.
///
/// Content-addressed ids make "what are you missing" a cheap set difference
/// (spec): we compare LogEntry ids (event blob OIDs) both ways. The naive
/// full-snapshot exchange is fine for v1; Merkle-style comparison is a later
/// optimization (spec blesses this).
pub fn hub_leg(ctx: &LeviCtx) -> Result<Option<String>> {
    let Some(addr) = ctx.config.hub.clone() else {
        return Ok(None);
    };
    // Re-read and re-materialize: opportunistic syncs run right after an
    // append, and ctx.world predates it (init's Project event, a close's
    // StatusChange anchor, ...).
    let local = ctx.store.read()?;
    let world = materialize(local.clone());
    let Some(project) = &world.project else {
        bail!("no levi project here yet");
    };
    let project_id = project.id.to_string();
    let timeout = Duration::from_secs(10);

    let session = HubSession::connect(&addr, timeout)?;

    // Merkle-style comparison (spec §Sync): compare 2-hex-prefix bucket
    // hashes of event ids first; only differing buckets are enumerated and
    // only missing events are transferred. Same-state syncs cost one small
    // report round trip instead of the full log.
    let hub_buckets: levi_core::hub::LogEntryBucketsOut = session.report_once(
        levi_core::hub::LogEntryBuckets {
            project_id: project_id.clone(),
        },
        timeout,
    )?;
    let local_ids_all: Vec<&str> = local.iter().map(|r| r.id.as_str()).collect();
    let local_buckets = levi_core::merkle::bucket_hashes(local_ids_all.iter().copied());

    let mut differing: Vec<&String> = Vec::new();
    for bucket in local_buckets.keys().chain(hub_buckets.buckets.keys()) {
        if local_buckets.get(bucket) != hub_buckets.buckets.get(bucket)
            && !differing.contains(&bucket)
        {
            differing.push(bucket);
        }
    }

    let mut push = Vec::new();
    let mut pull_ids: Vec<String> = Vec::new();
    for bucket in differing {
        let hub_ids: HashSet<String> = session
            .report_once::<levi_core::hub::LogEntryBucketIds, levi_core::hub::LogEntryBucketIdsOut>(
                levi_core::hub::LogEntryBucketIds {
                    project_id: project_id.clone(),
                    bucket: bucket.clone(),
                },
                timeout,
            )?
            .ids
            .into_iter()
            .collect();
        for record in local.iter().filter(|r| r.id.starts_with(bucket.as_str())) {
            if !hub_ids.contains(&record.id) {
                let bytes = EventStore::encode(&record.event);
                let entry =
                    LogEntry::wrap(&record.id, &project_id, &bytes, &record.event.created_at);
                push.push(MEvent::from_item(
                    &entry,
                    MEventType::SET,
                    &ctx.identity.machine,
                ));
            }
        }
        let local_in_bucket: HashSet<&str> = local
            .iter()
            .map(|r| r.id.as_str())
            .filter(|id| id.starts_with(bucket.as_str()))
            .collect();
        pull_ids.extend(
            hub_ids
                .into_iter()
                .filter(|id| !local_in_bucket.contains(id.as_str())),
        );
    }

    let pushed = push.len();
    session.send_events(push)?;

    // Pull: fetch only the missing entries by id; unwrap and append
    // (content addressing dedups and re-derives the id from the bytes — a
    // tampered id cannot smuggle a mismatched event into the ref).
    let pulled = pull_ids.len();
    if !pull_ids.is_empty() {
        let entries = session.query_at_least(
            levi_core::GetLogEntrysByIds {
                ids: pull_ids.iter().map(|i| i.as_str().into()).collect(),
            },
            pull_ids.len(),
            timeout,
        )?;
        let mut pull = Vec::new();
        for entry in &entries {
            match entry.unwrap_event() {
                Ok(event) => pull.push(event),
                Err(e) => eprintln!(
                    "warning: skipping undecodable hub event {}: {e}",
                    entry.id.0
                ),
            }
        }
        ctx.store.append(&pull)?;
    }

    // Facts leg (spec leg 3) rides the same session.
    let facts = crate::facts::publish(ctx, &world, &session)?;

    // Foreign-status cache refresh (cross-project deps): the only place the
    // ladder's rung-2 answers come from — `ls`/`next` never hit the network.
    let foreign = crate::foreign::refresh_cache(ctx, &world, &session).unwrap_or(0);
    let _ = foreign;

    session.close();
    Ok(Some(format!(
        "pushed {pushed}, pulled {pulled} event(s), published {facts} fact(s)"
    )))
}

/// Fetch the remote's events ref to a tracking ref, union-merge, push back.
/// Transport is the `git` binary (spec deviation 7).
pub fn git_leg(ctx: &LeviCtx) -> Result<String> {
    let remote = &ctx.config.remote;
    let repo_dir = ctx
        .store
        .repo()
        .workdir()
        .unwrap_or_else(|| ctx.store.repo().git_dir())
        .to_path_buf();
    // Confirm the remote exists before doing anything.
    let has_remote = Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(&repo_dir)
        .output()
        .context("running git")?;
    if !has_remote.status.success() {
        return Ok(format!("no remote '{remote}' configured; skipped"));
    }

    let tracking = format!("refs/levi/remotes/{remote}/events");
    let fetch = Command::new("git")
        .args(["fetch", "-q", remote, &format!("+{EVENTS_REF}:{tracking}")])
        .current_dir(&repo_dir)
        .output()?;
    // A missing remote ref is fine (first push); other failures are not.
    let fetched = fetch.status.success();

    let new_events = if fetched {
        ctx.store.merge_ref(&tracking)?
    } else {
        0
    };

    // Push with fetch-merge-retry: our union commit always fast-forwards the
    // remote unless someone pushed meanwhile — then re-fetch, re-union, retry.
    let mut pushed = false;
    for _ in 0..3 {
        let push = Command::new("git")
            .args(["push", "-q", remote, &format!("{EVENTS_REF}:{EVENTS_REF}")])
            .current_dir(&repo_dir)
            .output()?;
        if push.status.success() {
            pushed = true;
            break;
        }
        let refetch = Command::new("git")
            .args(["fetch", "-q", remote, &format!("+{EVENTS_REF}:{tracking}")])
            .current_dir(&repo_dir)
            .output()?;
        if !refetch.status.success() {
            bail!(
                "push to {remote} failed and re-fetch failed: {}",
                String::from_utf8_lossy(&push.stderr).trim()
            );
        }
        ctx.store.merge_ref(&tracking)?;
    }
    if !pushed {
        bail!("could not push {EVENTS_REF} to {remote} after 3 attempts");
    }
    Ok(format!(
        "{new_events} new event(s) fetched, pushed to {remote}"
    ))
}

/// Best-effort background sync (spec §Sync: "opportunistic background sync
/// on exit"): spawn a detached `levi sync` so the mutating command never
/// waits on the network — an unreachable hub or slow remote costs nothing.
/// No hub and no git remote ⇒ no-op. Failures are silent by design; `levi
/// sync` reports loudly when run explicitly.
pub fn opportunistic(ctx: &LeviCtx) {
    let has_hub = ctx.config.hub.is_some();
    let has_remote = Command::new("git")
        .args(["remote", "get-url", &ctx.config.remote])
        .current_dir(
            ctx.store
                .repo()
                .workdir()
                .unwrap_or_else(|| ctx.store.repo().git_dir()),
        )
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_hub && !has_remote {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let dir = ctx
        .store
        .repo()
        .workdir()
        .unwrap_or_else(|| ctx.store.repo().git_dir())
        .to_path_buf();
    let _ = Command::new(exe)
        .arg("sync")
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Deliberately not waited on: the child outlives this process.
}

/// Blocking recovery for read commands on a fresh clone: when the events
/// ref is absent locally, run the git leg once and reload. Hub-based
/// recovery is impossible while uninitialized (the hub is queried by
/// project id, which we don't know yet) — but once the git leg lands
/// events, the hub leg tops up best-effort. Failures degrade to a stderr
/// note and `false`; stdout stays reserved for command output.
pub fn recover_uninitialized(ctx: &mut LeviCtx) -> bool {
    if ctx.no_sync || !ctx.uninitialized() {
        return false;
    }
    if let Err(e) = git_leg(ctx) {
        eprintln!("levi: sync attempt failed: {e:#}");
    }
    if ctx.reload().is_err() || ctx.uninitialized() {
        return false;
    }
    if let Err(e) = hub_leg(ctx) {
        eprintln!("levi: hub sync failed: {e:#}");
    }
    let _ = ctx.reload();
    eprintln!("levi: fetched events via sync (repo had none locally)");
    true
}
