//! `levi request-read` / `levi withdraw-read` — ask for a read of a task at
//! a commit before it's merged, or take the ask back. A later review of the
//! task by anyone answers it; until then `check-claims --require-review`
//! fails. One request per requester per task: asking again replaces it.

use anyhow::{Context, Result, bail};
use levi_core::ReadRequest;
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::read_request_id;

use crate::ctx::LeviCtx;

fn my_request_id(ctx: &LeviCtx, task_id: &str) -> String {
    let me = &ctx.identity;
    let machine = if me.machine_id.is_empty() {
        &me.machine
    } else {
        &me.machine_id
    };
    read_request_id(task_id, &me.dev, machine, &me.worktree)
}

pub fn request(ctx: &LeviCtx, id_input: &str, commit: Option<&str>) -> Result<()> {
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);
    let commit = match commit {
        Some(spec) => ctx.rev(spec)?.to_string(),
        None => ctx
            .head_commit()
            .context("HEAD has no commits; name the commit to read with --commit")?
            .to_string(),
    };
    let request = ReadRequest {
        id: my_request_id(ctx, &task_id).into(),
        project_id,
        task_id,
        commit: commit.clone(),
        by_dev: ctx.identity.dev.clone(),
        by_machine: ctx.identity.machine.clone(),
        by_machine_id: ctx.identity.machine_id.clone(),
        by_worktree: ctx.identity.worktree.clone(),
        created: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&request)])?;
    println!("read requested on {short} @ {}", &commit[..8]);
    Ok(())
}

pub fn withdraw(ctx: &LeviCtx, id_input: &str) -> Result<()> {
    ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);
    let Some(request) = ctx.world.read_requests.get(&my_request_id(ctx, &task_id)) else {
        bail!("you have no read request on {short} from this worktree");
    };
    ctx.append_and_sync(vec![ctx.del_event(request)])?;
    println!("read request on {short} withdrawn");
    Ok(())
}
