use std::collections::HashSet;

use anyhow::Result;
use chrono::Utc;
use levi_core::resolve::{Resolution, Status};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::{SCHEMA_LS, task_json, task_line};

/// Warn about anchors orphaned by rebase/cherry-pick (spec §Anchoring rules)
/// or landed here under a new sha by a squash merge (lv-1e92).
///
/// Only tasks that actually read open are considered: the warnings all end in
/// "re-close with ...", which is wrong advice for a task whose status already
/// resolves correctly — a re-anchored task keeps its superseded anchors in the
/// log forever, and warning about those is pure noise.
pub fn warn_orphaned_anchors<'a>(ctx: &LeviCtx, open_task_ids: impl Iterator<Item = &'a str>) {
    let mut anchors = Vec::new();
    for id in open_task_ids {
        for change in ctx.world.changes_for(id) {
            if let Some(sha) = &change.anchor_commit {
                anchors.push(sha.clone());
            }
        }
    }
    let mut warned: HashSet<String> = HashSet::new();
    for orphan in crate::ancestors::orphaned_anchors(ctx.store.repo(), anchors.clone()) {
        warned.insert(orphan.sha.clone());
        let sha = &orphan.sha[..8.min(orphan.sha.len())];
        match &orphan.rewritten_as {
            Some(new_sha) => eprintln!(
                "warning: anchor {sha} was rewritten as {} (same patch-id); affected tasks \
                 may show as open here. Re-close with `levi close <id> --anchor {}`.",
                &new_sha[..8.min(new_sha.len())],
                &new_sha[..8.min(new_sha.len())],
            ),
            None => eprintln!(
                "warning: anchor {sha} is unreachable from any ref — likely rebased away; \
                 affected tasks may show as open here. Re-run `levi close <id>` at the new HEAD.",
            ),
        }
    }
    // Squash/rebase merges keep the source branch alive, so the anchor stays
    // reachable and the orphan pass above says nothing — but the change is in
    // HEAD under a new sha, and the task reads open until it is re-anchored.
    let unwarned = anchors.into_iter().filter(|sha| !warned.contains(sha));
    for squashed in crate::ancestors::squashed_anchors(ctx.store.repo(), unwarned) {
        let sha = &squashed.sha[..8.min(squashed.sha.len())];
        let landed = &squashed.landed_as[..8.min(squashed.landed_as.len())];
        eprintln!(
            "warning: anchor {sha} is not in this history, but its change was squashed in \
             as {landed} (same patch-id); affected tasks show as open here. \
             Re-close with `levi close <id> --anchor {landed}`."
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

pub fn run(ctx: &mut LeviCtx, opts: LsOpts) -> Result<()> {
    if ctx.uninitialized() && !crate::sync::recover_uninitialized(ctx) {
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
                || ctx
                    .world
                    .live_claim(id, now)
                    .is_some_and(|c| levi_core::rank::claim_is(c, &ctx.identity));
            status_ok && label_ok && mine_ok
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.priority.rank(), a.created.as_str()).cmp(&(b.priority.rank(), b.created.as_str()))
    });

    warn_orphaned_anchors(
        ctx,
        rows.iter()
            .map(|t| &*t.id.0)
            .filter(|id| statuses[*id].status == Status::Open),
    );

    // Non-applying closes per open task: "the fix exists, just not here".
    let tips = crate::ancestors::BranchTips::new(ctx.store.repo());
    let mut elsewhere_of = |task_id: &str| -> Vec<crate::ancestors::ElsewhereClose> {
        if statuses[task_id].status != Status::Open {
            return Vec::new();
        }
        crate::ancestors::elsewhere_closes(
            ctx.store.repo(),
            &ctx.world.changes_for(task_id),
            &mut anc,
            &tips,
        )
    };

    if opts.json {
        let tasks: Vec<_> = rows
            .iter()
            .map(|t| {
                let elsewhere = elsewhere_of(&t.id.0);
                task_json(&ctx.world, t, &statuses[&*t.id.0], now, &elsewhere)
            })
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
            let elsewhere = elsewhere_of(&t.id.0);
            println!(
                "{}",
                task_line(&ctx.world, t, &statuses[&*t.id.0], now, &elsewhere)
            );
        }
    }
    Ok(())
}
