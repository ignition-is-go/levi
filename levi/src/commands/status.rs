//! `levi close` / `levi reopen` — append a StatusChange anchored at HEAD
//! (default), `--anchor SHA`, or unanchored with `--no-anchor` (spec
//! §Anchoring rules).

use anyhow::{Result, bail};
use chrono::Utc;
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::resolve::{Resolution, Status, effective_status};
use levi_core::{StatusChange, StatusKind};

use crate::ctx::LeviCtx;

pub struct StatusOpts {
    pub anchor: Option<String>,
    pub no_anchor: bool,
    pub force: bool,
    pub no_drop: bool,
}

pub fn run(ctx: &LeviCtx, id_input: &str, kind: StatusKind, opts: StatusOpts) -> Result<()> {
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);

    let mut anc = ctx.ancestors_for(None)?;
    let current = effective_status(
        &ctx.world.changes_for(&task_id),
        &mut anc,
        Resolution::Exact,
    );
    match (kind, current.status) {
        (StatusKind::Closed, Status::Closed) if !opts.force => {
            bail!("{short} is already closed on this checkout (use --force to close anyway)")
        }
        (StatusKind::Reopened, Status::Open) if !opts.force => {
            bail!("{short} is already open on this checkout (use --force to reopen anyway)")
        }
        _ => {}
    }

    let anchor_commit = if opts.no_anchor {
        None
    } else if let Some(spec) = &opts.anchor {
        Some(ctx.rev(spec)?.to_string())
    } else {
        match ctx.head_commit() {
            Some(head) => Some(head.to_string()),
            None => bail!(
                "HEAD has no commits to anchor to; use --no-anchor for a task unrelated to code state"
            ),
        }
    };

    let change = StatusChange {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id,
        task_id: task_id.clone(),
        to_status: kind,
        anchor_commit: anchor_commit.clone(),
        created: LeviCtx::now(),
        by_dev: ctx.identity.dev.clone(),
        by_machine: ctx.identity.machine.clone(),
    };

    // Releasing the claim on close/reopen is the natural end of holding a task
    // (spec 2026-07-22). Only our own live claim; never someone else's.
    let mut events = vec![ctx.set_event(&change)];
    if !opts.no_drop
        && let Some(claim) = ctx.world.live_claim(&task_id, Utc::now())
        && levi_core::rank::claim_is(claim, &ctx.identity)
    {
        events.push(ctx.del_event(&claim.clone()));
    }
    ctx.append_and_sync(events)?;

    let verb = match kind {
        StatusKind::Closed => "closed",
        StatusKind::Reopened => "reopened",
    };
    match &anchor_commit {
        Some(sha) => println!("{short} {verb} @ {}", &sha[..8]),
        None => println!("{short} {verb} (everywhere)"),
    }
    Ok(())
}
