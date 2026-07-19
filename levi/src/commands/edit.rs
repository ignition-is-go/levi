//! `levi edit` — metadata edits are ordinary SET events; concurrent edits
//! settle by LWW (created_at, ties by event id) on every node identically.

use anyhow::{Result, bail};
use levi_core::Priority;
use levi_core::ids::{resolve_prefix, short_id};

use crate::ctx::LeviCtx;

pub fn run(
    ctx: &LeviCtx,
    id_input: &str,
    priority: Option<String>,
    title: Option<String>,
    labels: Vec<String>,
    body: Option<String>,
) -> Result<()> {
    ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let mut task = ctx.world.tasks[&task_id].clone();

    let mut changed = false;
    if let Some(p) = &priority {
        task.priority = Priority::parse(p)
            .ok_or_else(|| anyhow::anyhow!("invalid priority '{p}' (use p0..p3)"))?;
        changed = true;
    }
    if let Some(t) = title {
        task.title = t;
        changed = true;
    }
    if let Some(b) = body {
        task.body = b;
        changed = true;
    }
    for label in labels {
        changed = true;
        if let Some(removed) = label.strip_prefix('-') {
            task.labels.retain(|l| l != removed);
        } else {
            let added = label.strip_prefix('+').unwrap_or(&label);
            if !task.labels.iter().any(|l| l == added) {
                task.labels.push(added.to_string());
            }
        }
    }
    if !changed {
        bail!("nothing to edit (use -p, --title, -b, or -l)");
    }
    ctx.append_and_sync(vec![ctx.set_event(&task)])?;
    println!("updated {}", short_id(&ctx.world, &task_id));
    Ok(())
}
