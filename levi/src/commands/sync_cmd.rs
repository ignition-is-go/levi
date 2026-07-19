//! `levi sync` — three independent, best-effort legs (spec §Sync):
//! 1. git leg: fetch `refs/levi/events` from the remote, union-merge, push.
//! 2. hub leg: exchange event diffs with the hub (Task 12).
//! 3. facts leg: publish CommitFacts/RefFacts to the hub (Task 12).

use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::ctx::LeviCtx;
use crate::store::EVENTS_REF;

pub fn run(ctx: &mut LeviCtx, no_git: bool, no_hub: bool) -> Result<()> {
    let mut failures = Vec::new();

    if !no_git {
        match git_leg(ctx) {
            Ok(summary) => println!("git: {summary}"),
            Err(e) => failures.push(format!("git leg: {e:#}")),
        }
    }
    if !no_hub {
        match crate::sync::hub_leg(ctx) {
            Ok(Some(summary)) => println!("hub: {summary}"),
            Ok(None) => println!("hub: not configured (set `git config levi.hub`)"),
            Err(e) => failures.push(format!("hub leg: {e:#}")),
        }
    }
    ctx.reload()?;
    if !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    Ok(())
}

/// Fetch the remote's events ref to a tracking ref, union-merge, push back.
/// Transport is the `git` binary (spec deviation 7).
fn git_leg(ctx: &LeviCtx) -> Result<String> {
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
