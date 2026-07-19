use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::{Dependency, dependency_id};

use crate::ctx::LeviCtx;

pub fn add(
    ctx: &LeviCtx,
    blocked_input: &str,
    blocker_input: &str,
    via: Option<String>,
) -> Result<()> {
    let project_id = ctx.project_id()?;
    let blocked = resolve_prefix(&ctx.world, blocked_input)?.to_string();
    if let Some(target) = crate::foreign::parse_target(blocker_input) {
        return add_foreign(ctx, project_id, blocked, &target, via);
    }
    if via.is_some() {
        bail!("--via is for cross-project deps (`--on project/lv-xxxx`)");
    }
    let blocker = resolve_prefix(&ctx.world, blocker_input)?.to_string();
    if blocked == blocker {
        bail!("a task cannot block itself");
    }
    if would_cycle(ctx, &blocker, &blocked) {
        eprintln!(
            "warning: this creates a dependency cycle; the tasks involved will never be eligible \
             for `levi next` until the cycle is broken"
        );
    }
    let dep = Dependency {
        id: dependency_id(&blocker, &blocked).into(),
        project_id,
        blocker_task_id: blocker.clone(),
        blocked_task_id: blocked.clone(),
        blocker_project_id: None,
        blocker_ref: None,
        via: None,
    };
    ctx.append_and_sync(vec![ctx.set_event(&dep)])?;
    println!(
        "{} is blocked by {}",
        short_id(&ctx.world, &blocked),
        short_id(&ctx.world, &blocker)
    );
    Ok(())
}

pub fn rm(ctx: &LeviCtx, blocked_input: &str, blocker_input: &str) -> Result<()> {
    ctx.project_id()?;
    let blocked = resolve_prefix(&ctx.world, blocked_input)?.to_string();
    if let Some(target) = crate::foreign::parse_target(blocker_input) {
        return rm_foreign(ctx, blocked, &target);
    }
    let blocker = resolve_prefix(&ctx.world, blocker_input)?.to_string();
    let id = dependency_id(&blocker, &blocked);
    let Some(dep) = ctx.world.deps.get(&id) else {
        bail!(
            "{} is not blocked by {}",
            short_id(&ctx.world, &blocked),
            short_id(&ctx.world, &blocker)
        );
    };
    ctx.append_and_sync(vec![ctx.del_event(dep)])?;
    println!(
        "{} is no longer blocked by {}",
        short_id(&ctx.world, &blocked),
        short_id(&ctx.world, &blocker)
    );
    Ok(())
}

/// Would blocker -> blocked close a cycle? True iff `blocker` is reachable
/// from `blocked` along existing blocks edges.
fn would_cycle(ctx: &LeviCtx, blocker: &str, blocked: &str) -> bool {
    let mut blocks: HashMap<&str, Vec<&str>> = HashMap::new();
    for dep in ctx.world.deps.values() {
        blocks
            .entry(&dep.blocker_task_id)
            .or_default()
            .push(&dep.blocked_task_id);
    }
    let mut seen = HashSet::new();
    let mut stack = vec![blocked];
    while let Some(next) = stack.pop() {
        if next == blocker {
            return true;
        }
        if seen.insert(next)
            && let Some(more) = blocks.get(next)
        {
            stack.extend(more);
        }
    }
    false
}

/// Cross-project dep: the event lives in the *blocked* task's project log
/// only; the blocker is identified by (project id, task id) and resolved
/// through the ladder from then on.
fn add_foreign(
    ctx: &LeviCtx,
    project_id: String,
    blocked: String,
    target: &crate::foreign::ForeignTarget,
    via: Option<String>,
) -> Result<()> {
    let (blocker_project, blocker_task, display) = match crate::foreign::offline_target(target) {
        Some((p, t)) => (p.clone(), t, p),
        None => {
            let session = crate::foreign::connect(ctx)?;
            let (pid, name) = crate::foreign::resolve_project(&session, &target.project)?;
            let task = crate::foreign::resolve_foreign_task(&session, &pid, &target.task)?;
            let out = (pid, task.id.to_string(), name);
            session.close();
            out
        }
    };
    let dep = Dependency {
        id: levi_core::foreign_dependency_id(&blocker_project, &blocker_task, &blocked).into(),
        project_id,
        blocker_task_id: blocker_task.clone(),
        blocked_task_id: blocked.clone(),
        blocker_project_id: Some(blocker_project),
        blocker_ref: target.refname.clone(),
        via,
    };
    ctx.append_and_sync(vec![ctx.set_event(&dep)])?;
    println!(
        "{} is blocked by {display}/lv-{}",
        short_id(&ctx.world, &blocked),
        &blocker_task[..4.min(blocker_task.len())]
    );
    Ok(())
}

fn rm_foreign(
    ctx: &LeviCtx,
    blocked: String,
    target: &crate::foreign::ForeignTarget,
) -> Result<()> {
    // Find the dep by blocked + foreign task prefix (no hub needed).
    let matches: Vec<_> = ctx
        .world
        .deps
        .values()
        .filter(|d| {
            *d.blocked_task_id == blocked
                && d.blocker_project_id.is_some()
                && d.blocker_task_id.starts_with(&target.task)
        })
        .collect();
    match matches.len() {
        0 => bail!(
            "{} has no matching cross-project dep",
            short_id(&ctx.world, &blocked)
        ),
        1 => {
            let dep = matches[0].clone();
            ctx.append_and_sync(vec![ctx.del_event(&dep)])?;
            println!(
                "{} is no longer blocked by it",
                short_id(&ctx.world, &blocked)
            );
            Ok(())
        }
        _ => bail!(
            "'{}' matches multiple cross-project deps; use the full id",
            target.task
        ),
    }
}
