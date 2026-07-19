use anyhow::Result;
use levi_core::Comment;
use levi_core::ids::{resolve_prefix, short_id};

use crate::ctx::LeviCtx;

pub fn run(ctx: &LeviCtx, id_input: &str, text: &str) -> Result<()> {
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
