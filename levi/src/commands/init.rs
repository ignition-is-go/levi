use anyhow::{Result, bail};
use levi_core::Project;

use crate::ctx::LeviCtx;

pub fn run(ctx: &LeviCtx, name: Option<String>) -> Result<()> {
    if let Some(p) = &ctx.world.project {
        bail!("levi project already initialized here: {} ({})", p.name, p.id.0);
    }
    let name = name.unwrap_or_else(|| {
        ctx.store
            .repo()
            .workdir()
            .and_then(|w| w.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    });
    let project = Project {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        name: name.clone(),
        created_at: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&project)])?;
    println!("initialized levi project '{name}' ({})", project.id.0);
    Ok(())
}
