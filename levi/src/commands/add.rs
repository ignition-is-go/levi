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
    project: Option<String>,
    priority: Option<String>,
    body: Option<String>,
    labels: Vec<String>,
    deps: Vec<String>,
    json: bool,
) -> Result<()> {
    let priority = match &priority {
        None => Priority::default(),
        Some(p) => match Priority::parse(p) {
            Some(p) => p,
            None => bail!("invalid priority '{p}' (use p0..p3)"),
        },
    };
    if let Some(target) = project {
        if !deps.is_empty() {
            bail!("--dep is not supported with --project (v1: create + comment only)");
        }
        return add_foreign(ctx, target, title, priority, body, labels, json);
    }
    let project_id = ctx.project_id()?;
    let task = Task {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id: project_id.clone(),
        title,
        body: body.unwrap_or_default(),
        priority,
        labels,
        created_by_dev: ctx.identity.dev.clone(),
        created_by_machine: ctx.identity.machine.clone(),
        created: LeviCtx::now(),
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
            blocker_project_id: None,
            blocker_ref: None,
            via: None,
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

/// File a task in another project. Hub-ack-then-forget: the event's first
/// authoritative home is the hub; a checkout of the foreign project pulls
/// it into its real ref on next sync (spec: cross-project write path).
fn add_foreign(
    ctx: &LeviCtx,
    target: String,
    title: String,
    priority: Priority,
    body: Option<String>,
    labels: Vec<String>,
    json: bool,
) -> Result<()> {
    let session = crate::foreign::connect(ctx)?;
    let (project_id, project_name) = crate::foreign::resolve_project(&session, &target)?;
    let task = Task {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id: project_id.clone(),
        title,
        body: body.unwrap_or_default(),
        priority,
        labels,
        created_by_dev: ctx.identity.dev.clone(),
        created_by_machine: ctx.identity.machine.clone(),
        created: LeviCtx::now(),
    };
    let task_id = task.id.to_string();
    let event = ctx.set_event(&task);
    session.push_events_verified(&project_id, &[event])?;
    session.close();
    let short = format!("{project_name}/lv-{}", &task_id[..4]);
    if json {
        println!(
            "{}",
            json!({
                "schema": SCHEMA_ADD,
                "id": task_id,
                "project_id": project_id,
                "project": project_name,
                "short": short,
            })
        );
    } else {
        println!("{short} {project_id}/{task_id}");
    }
    Ok(())
}
