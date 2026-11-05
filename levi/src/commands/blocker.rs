//! `levi block` / `levi unblock` — say what a task is waiting on when it
//! isn't another task ("waiting on the operator to open port 6300"), and close
//! it again with why. A task with an open blocker isn't eligible for `next`.
//! Anyone can close a blocker; the log keeps who opened and who closed it.

use anyhow::{Result, bail};
use levi_core::Blocker;
use levi_core::ids::{resolve_prefix, short_id};

use crate::ctx::LeviCtx;

/// A blocker's short id: `bk-` and the first 8 hex of its id.
pub fn blocker_short(id: &str) -> String {
    format!("bk-{}", &id[..8.min(id.len())])
}

pub fn block(ctx: &LeviCtx, id_input: &str, reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("say what the task is waiting on (-m)");
    }
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let blocker = Blocker {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id,
        task_id: task_id.clone(),
        reason: reason.to_string(),
        by_dev: ctx.identity.dev.clone(),
        by_machine: ctx.identity.machine.clone(),
        created: LeviCtx::now(),
        closed_by: None,
        closed_reason: None,
        closed_at: None,
    };
    ctx.append_and_sync(vec![ctx.set_event(&blocker)])?;
    println!(
        "{} blocks {}",
        blocker_short(&blocker.id.0),
        short_id(&ctx.world, &task_id)
    );
    Ok(())
}

pub fn unblock(ctx: &LeviCtx, blocker_input: &str, why: &str) -> Result<()> {
    if why.trim().is_empty() {
        bail!("say why it no longer blocks (-m)");
    }
    ctx.project_id()?;
    let prefix = blocker_input.strip_prefix("bk-").unwrap_or(blocker_input);
    if prefix.len() < 4 {
        bail!("give at least 4 characters of the blocker id");
    }
    let matches: Vec<&Blocker> = ctx
        .world
        .blockers
        .values()
        .filter(|b| b.id.0.starts_with(prefix))
        .collect();
    let blocker = match matches.as_slice() {
        [] => bail!("no blocker matches {blocker_input}"),
        [one] => *one,
        _ => bail!("{blocker_input} matches more than one blocker; give more of its id"),
    };
    if !blocker.is_open() {
        bail!(
            "{} is already closed, by {}",
            blocker_short(&blocker.id.0),
            blocker.closed_by.as_deref().unwrap_or("someone")
        );
    }
    let mut closed = blocker.clone();
    closed.closed_by = Some(ctx.identity.dev.clone());
    closed.closed_reason = Some(why.to_string());
    closed.closed_at = Some(LeviCtx::now());
    ctx.append_and_sync(vec![ctx.set_event(&closed)])?;
    println!(
        "{} closed; {} no longer waits on it",
        blocker_short(&closed.id.0),
        short_id(&ctx.world, &closed.task_id)
    );
    Ok(())
}
