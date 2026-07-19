use anyhow::{Result, bail};
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::{Dependency, Priority, Task, dependency_id};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::SCHEMA_ADD;

#[allow(clippy::too_many_arguments)]
pub fn run(
    ctx: &LeviCtx,
    title: String,
    priority: Option<String>,
    body: Option<String>,
    labels: Vec<String>,
    deps: Vec<String>,
    json: bool,
) -> Result<()> {
    let project_id = ctx.project_id()?;
    let priority = match &priority {
        None => Priority::default(),
        Some(p) => match Priority::parse(p) {
            Some(p) => p,
            None => bail!("invalid priority '{p}' (use p0..p3)"),
        },
    };
    let task = Task {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id: project_id.clone(),
        title,
        body: body.unwrap_or_default(),
        priority,
        labels,
        created_by_dev: ctx.identity.dev.clone(),
        created_by_machine: ctx.identity.machine.clone(),
        created_at: LeviCtx::now(),
    };
    let task_id = task.id.to_string();
    let mut events = vec![ctx.set_event(&task)];
    for dep in &deps {
        let blocker = resolve_prefix(&ctx.world, dep)?.to_string();
        let dependency = Dependency {
            id: dependency_id(&blocker, &task_id).into(),
            project_id: project_id.clone(),
            blocker_task_id: blocker,
            blocked_task_id: task_id.clone(),
        };
        events.push(ctx.set_event(&dependency));
    }
    ctx.append_and_sync(events)?;
    let short = short_id(&ctx.world, &task_id);
    if json {
        println!(
            "{}",
            json!({"schema": SCHEMA_ADD, "id": task_id, "short": short})
        );
    } else {
        println!("{short} {task_id}");
    }
    Ok(())
}
