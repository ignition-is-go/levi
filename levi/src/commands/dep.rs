use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::{Dependency, dependency_id};

use crate::ctx::LeviCtx;

pub fn add(ctx: &LeviCtx, blocked_input: &str, blocker_input: &str) -> Result<()> {
    let project_id = ctx.project_id()?;
    let blocked = resolve_prefix(&ctx.world, blocked_input)?.to_string();
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
        blocks.entry(&dep.blocker_task_id).or_default().push(&dep.blocked_task_id);
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
