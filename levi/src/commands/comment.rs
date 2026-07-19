use anyhow::Result;
use levi_core::Comment;
use levi_core::ids::{resolve_prefix, short_id};

use crate::ctx::LeviCtx;

pub fn run(ctx: &LeviCtx, id_input: &str, text: &str) -> Result<()> {
    if let Some(target) = crate::foreign::parse_target(id_input) {
        return comment_foreign(ctx, &target, text);
    }
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let comment = Comment {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id,
        task_id: task_id.clone(),
        body: text.to_string(),
        by_dev: ctx.identity.dev.clone(),
        created: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&comment)])?;
    println!("commented on {}", short_id(&ctx.world, &task_id));
    Ok(())
}

/// Comment on a task in another project (hub write-through, like
/// `add --project`).
fn comment_foreign(
    ctx: &LeviCtx,
    target: &crate::foreign::ForeignTarget,
    text: &str,
) -> Result<()> {
    let session = crate::foreign::connect(ctx)?;
    let (project_id, task_id, name) = match crate::foreign::offline_target(target) {
        Some((p, t)) => (p.clone(), t, p),
        None => {
            let (project_id, name) = crate::foreign::resolve_project(&session, &target.project)?;
            let task = crate::foreign::resolve_foreign_task(&session, &project_id, &target.task)?;
            (project_id, task.id.to_string(), name)
        }
    };
    let comment = Comment {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id: project_id.clone(),
        task_id: task_id.clone(),
        body: text.to_string(),
        by_dev: ctx.identity.dev.clone(),
        created: LeviCtx::now(),
    };
    let event = ctx.set_event(&comment);
    session.push_events_verified(&project_id, &[event])?;
    session.close();
    println!(
        "commented on {name}/lv-{}",
        &task_id[..4.min(task_id.len())]
    );
    Ok(())
}
