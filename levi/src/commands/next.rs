//! `levi next` — deterministically surface the most important eligible work
//! (spec §Ranking). `--claim` appends the claim atomically before printing:
//! the "no foreign live claim" check and the append happen inside the store's
//! mutation lock, so parallel agents on one machine cannot both claim a task.

use anyhow::Result;
use chrono::Utc;
use levi_core::Claim;
use levi_core::ids::short_id;
use levi_core::materialize::materialize;
use levi_core::rank::{RankedTask, rank_next};
use levi_core::resolve::{Resolution, Status, effective_status};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::{SCHEMA_NEXT, task_json};

pub fn run(ctx: &mut LeviCtx, claim: bool, count: usize, json: bool) -> Result<()> {
    if ctx.uninitialized() {
        if json {
            println!("{}", json!({"schema": SCHEMA_NEXT, "tasks": []}));
        }
        eprintln!("no levi events here. Run `levi init` first.");
        return Ok(());
    }

    if !claim {
        let ranked = rank(ctx)?;
        return print(ctx, &ranked[..ranked.len().min(count)], json);
    }

    // --claim: rank, try to atomically claim the top task, re-rank on a lost
    // race (someone else's claim landed between our rank and our append).
    let project_id = ctx.project_id()?;
    for _ in 0..10 {
        let ranked = rank(ctx)?;
        let Some(top) = ranked.first() else {
            return print(ctx, &[], json);
        };
        let task_id = top.task_id.clone();
        let me = ctx.identity.clone();
        let now = Utc::now();
        let claim_item = Claim {
            id: task_id.clone().into(),
            project_id: project_id.clone(),
            task_id: task_id.clone(),
            dev: me.dev.clone(),
            machine: me.machine.clone(),
            worktree: me.worktree.clone(),
            created: LeviCtx::now(),
            ttl_secs: ctx.config.claim_ttl_secs,
        };
        let event = ctx.set_event(&claim_item);
        let appended = ctx.store.append_if(&[event], |records| {
            // Full eligibility recheck inside the critical section: another
            // process may have closed the task (or its blocker may have
            // reopened, or a claim landed) between our rank() and this lock.
            let world = materialize(records.to_vec());
            if !world.tasks.contains_key(&task_id) {
                return false;
            }
            let mut anc = crate::ancestors::GixAncestors::new(ctx.store.repo());
            let is_open = |anc: &mut crate::ancestors::GixAncestors, id: &str| {
                effective_status(&world.changes_for(id), anc, Resolution::Exact).status
                    == Status::Open
            };
            if !is_open(&mut anc, &task_id) {
                return false;
            }
            let blocked_by_open_blocker = world.deps.values().any(|dep| {
                *dep.blocked_task_id == task_id
                    && world.tasks.contains_key(&*dep.blocker_task_id)
                    && is_open(&mut anc, &dep.blocker_task_id)
            });
            if blocked_by_open_blocker {
                return false;
            }
            match world.live_claim(&task_id, now) {
                None => true,
                Some(c) => c.dev == me.dev && c.machine == me.machine && c.worktree == me.worktree,
            }
        })?;
        ctx.reload()?;
        if appended.is_some() {
            if !ctx.no_sync {
                crate::sync::opportunistic(ctx);
            }
            let ranked = vec![top.clone()];
            return print(ctx, &ranked, json);
        }
        // Lost the race: the world changed under us; re-rank.
    }
    anyhow::bail!("could not claim a task after repeated races; try again")
}

fn rank(ctx: &LeviCtx) -> Result<Vec<RankedTask>> {
    let mut anc = ctx.ancestors_for(None)?;
    let statuses = ctx.statuses(&mut anc, Resolution::Exact);
    Ok(rank_next(&ctx.world, &statuses, Utc::now(), &ctx.identity))
}

fn print(ctx: &LeviCtx, ranked: &[RankedTask], json: bool) -> Result<()> {
    let now = Utc::now();
    let mut anc = ctx.ancestors_for(None)?;
    let tips = crate::ancestors::BranchTips::new(ctx.store.repo());
    let elsewhere_of = |anc: &mut crate::ancestors::GixAncestors, task_id: &str| {
        crate::ancestors::elsewhere_closes(
            ctx.store.repo(),
            &ctx.world.changes_for(task_id),
            anc,
            &tips,
        )
    };
    // "Fix exists elsewhere" belongs in the reason an agent acts on: merging
    // an existing fix beats re-implementing it (lv-98bf).
    let full_reason = |r: &RankedTask, elsewhere: &[crate::ancestors::ElsewhereClose]| {
        match crate::output::elsewhere_hint(elsewhere) {
            Some(hint) => format!("{}; note: {hint} — consider merging it", r.reason),
            None => r.reason.clone(),
        }
    };

    if json {
        let statuses = ctx.statuses(&mut anc, Resolution::Exact);
        let tasks: Vec<_> = ranked
            .iter()
            .map(|r| {
                let task = &ctx.world.tasks[&r.task_id];
                let elsewhere = elsewhere_of(&mut anc, &r.task_id);
                let mut v = task_json(&ctx.world, task, &statuses[&r.task_id], now, &elsewhere);
                v["reason"] = json!(full_reason(r, &elsewhere));
                v["unblocks"] = json!(r.unblocks);
                v
            })
            .collect();
        println!("{}", json!({"schema": SCHEMA_NEXT, "tasks": tasks}));
        return Ok(());
    }
    if ranked.is_empty() {
        println!("no eligible tasks");
        return Ok(());
    }
    for r in ranked {
        let task = &ctx.world.tasks[&r.task_id];
        let elsewhere = elsewhere_of(&mut anc, &r.task_id);
        println!(
            "{} {} {}",
            short_id(&ctx.world, &r.task_id),
            task.priority.label(),
            task.title
        );
        println!("  reason: {}", full_reason(r, &elsewhere));
    }
    Ok(())
}
