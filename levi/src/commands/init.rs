use anyhow::{Result, bail};
use levi_core::Project;

use crate::ctx::LeviCtx;

pub fn run(ctx: &LeviCtx, name: Option<String>) -> Result<()> {
    if let Some(p) = &ctx.world.project {
        bail!(
            "levi project already initialized here: {} ({})",
            p.name,
            p.id.0
        );
    }
    let project = create_project(ctx, name)?;
    println!(
        "initialized levi project '{}' ({})",
        project.name, project.id.0
    );
    Ok(())
}

/// Mint the project id and append the first event (spec: stored in the first
/// event on the ref). Callers must have checked no project exists yet.
pub fn create_project(ctx: &LeviCtx, name: Option<String>) -> Result<Project> {
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
        name,
        created: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&project)])?;
    Ok(project)
}
