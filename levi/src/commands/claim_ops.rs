//! `levi start` / `levi steal` / `levi drop` — advisory claims keyed to
//! (dev, machine, worktree). Newest wins; `drop` only releases your own.

use anyhow::{Result, bail};
use chrono::Utc;
use levi_core::Claim;
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::materialize::materialize;

use crate::ctx::LeviCtx;

fn build_claim(ctx: &LeviCtx, task_id: &str, project_id: String) -> Claim {
    Claim {
        id: task_id.into(),
        project_id,
        task_id: task_id.into(),
        dev: ctx.identity.dev.clone(),
        machine: ctx.identity.machine.clone(),
        worktree: ctx.identity.worktree.clone(),
        at: LeviCtx::now(),
        ttl_secs: ctx.config.claim_ttl_secs,
    }
}

pub fn start(ctx: &LeviCtx, id_input: &str) -> Result<()> {
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);
    let me = ctx.identity.clone();
    let now = Utc::now();
    let claim = build_claim(ctx, &task_id, project_id);
    let event = ctx.set_event(&claim);
    // Same atomic pattern as `next --claim`: check-then-append in one
    // critical section.
    let appended = ctx.store.append_if(&[event], |records| {
        let world = materialize(records.to_vec());
        match world.live_claim(&task_id, now) {
            None => true,
            Some(c) => c.dev == me.dev && c.machine == me.machine && c.worktree == me.worktree,
        }
    })?;
    if appended.is_none() {
        let holder = ctx
            .world
            .live_claim(&task_id, now)
            .map(|c| format!("{} on {}", c.dev, c.machine))
            .unwrap_or_else(|| "someone else".into());
        bail!("{short} is already claimed by {holder} (use `levi steal {short}` to take it)");
    }
    if !ctx.no_sync {
        crate::sync::opportunistic(ctx);
    }
    println!("claimed {short}");
    Ok(())
}

pub fn steal(ctx: &LeviCtx, id_input: &str) -> Result<()> {
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let claim = build_claim(ctx, &task_id, project_id);
    ctx.append_and_sync(vec![ctx.set_event(&claim)])?;
    println!("stole {}", short_id(&ctx.world, &task_id));
    Ok(())
}

pub fn drop(ctx: &LeviCtx, id_input: &str) -> Result<()> {
    ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);
    let now = Utc::now();
    let Some(claim) = ctx.world.live_claim(&task_id, now) else {
        bail!("{short} is not claimed");
    };
    if claim.dev != ctx.identity.dev
        || claim.machine != ctx.identity.machine
        || claim.worktree != ctx.identity.worktree
    {
        bail!(
            "{short} is claimed by {} on {}, not you (use `levi steal {short}` to take it first)",
            claim.dev,
            claim.machine
        );
    }
    let claim = claim.clone();
    ctx.append_and_sync(vec![ctx.del_event(&claim)])?;
    println!("dropped {short}");
    Ok(())
}
