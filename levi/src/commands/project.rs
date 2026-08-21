use anyhow::{Context, Result, bail};

use crate::ctx::LeviCtx;

/// Rename the current project by appending a new SET event with the same
/// stable project id. Existing task history and cross-project references remain
/// intact; the hub registry materializes the latest project name.
pub fn rename(ctx: &LeviCtx, name: &str) -> Result<()> {
    let mut project = ctx
        .world
        .project
        .clone()
        .context("no levi project here; run this from the registered repository")?;
    let name = name.trim();
    if name.is_empty() {
        bail!("project name must not be empty");
    }
    if project.name == name {
        println!(
            "project already named '{}' ({})",
            project.name, project.id.0
        );
        return Ok(());
    }
    let old = project.name.clone();
    project.name = name.to_owned();
    ctx.append_and_sync(vec![ctx.set_event(&project)])?;
    println!(
        "renamed project '{}' to '{}' ({})",
        old, project.name, project.id.0
    );
    Ok(())
}
