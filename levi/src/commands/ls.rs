use anyhow::Result;
use chrono::Utc;
use levi_core::resolve::{Resolution, Status};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::{SCHEMA_LS, task_json, task_line};

/// Warn about anchors orphaned by rebase/cherry-pick (spec §Anchoring rules).
pub fn warn_orphaned_anchors<'a>(ctx: &LeviCtx, task_ids: impl Iterator<Item = &'a str>) {
    let mut anchors = Vec::new();
    for id in task_ids {
        for change in ctx.world.changes_for(id) {
            if let Some(sha) = &change.anchor_commit {
                anchors.push(sha.clone());
            }
        }
    }
    for sha in crate::ancestors::orphaned_anchors(ctx.store.repo(), anchors) {
        eprintln!(
            "warning: anchor {} is unreachable from any ref — likely rebased away; \
             affected tasks may show as open here. Re-run `levi close <id>` at the new HEAD.",
            &sha[..8.min(sha.len())]
        );
    }
}

pub struct LsOpts {
    pub json: bool,
    pub all: bool,
    pub closed: bool,
    pub label: Option<String>,
    pub branch: Option<String>,
    pub mine: bool,
}

pub fn run(ctx: &LeviCtx, opts: LsOpts) -> Result<()> {
    if ctx.uninitialized() {
        if opts.json {
            println!("{}", json!({"schema": SCHEMA_LS, "tasks": []}));
        }
        eprintln!(
            "no levi events here. Run `levi init`, or fetch existing events with \
             `git fetch <remote> '+refs/levi/events:refs/levi/events'` (or `levi sync`)."
        );
        return Ok(());
    }
    let now = Utc::now();
    let mut anc = ctx.ancestors_for(opts.branch.as_deref())?;
    let statuses = ctx.statuses(&mut anc, Resolution::Exact);

    let mut rows: Vec<_> = ctx
        .world
        .tasks
        .values()
        .filter(|t| {
            let id = &*t.id.0;
            let status = &statuses[id];
            let status_ok = if opts.all {
                true
            } else if opts.closed {
                status.status == Status::Closed
            } else {
                status.status == Status::Open
            };
            let label_ok = opts
                .label
                .as_ref()
                .map(|l| t.labels.contains(l))
                .unwrap_or(true);
            let mine_ok = !opts.mine
                || ctx.world.live_claim(id, now).is_some_and(|c| {
                    c.dev == ctx.identity.dev
                        && c.machine == ctx.identity.machine
                        && c.worktree == ctx.identity.worktree
                });
            status_ok && label_ok && mine_ok
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.priority.rank(), a.created_at.as_str()).cmp(&(b.priority.rank(), b.created_at.as_str()))
    });

    warn_orphaned_anchors(ctx, rows.iter().map(|t| &*t.id.0));

    if opts.json {
        let tasks: Vec<_> = rows
            .iter()
            .map(|t| task_json(&ctx.world, t, &statuses[&*t.id.0], now))
            .collect();
        println!(
            "{}",
            json!({
                "schema": SCHEMA_LS,
                "resolution_mode": if opts.branch.is_some() { "branch" } else { "head" },
                "tasks": tasks,
            })
        );
    } else {
        if rows.is_empty() {
            eprintln!("no matching tasks");
        }
        for t in rows {
            println!("{}", task_line(&ctx.world, t, &statuses[&*t.id.0], now));
        }
    }
    Ok(())
}
