use std::process::Command;

use anyhow::{Context, Result, bail};
use levi_core::Project;

use crate::ctx::LeviCtx;

pub fn run(ctx: &mut LeviCtx, name: Option<String>) -> Result<()> {
    if let Some(p) = &ctx.world.project {
        bail!(
            "levi project already initialized here: {} ({})",
            p.name,
            p.id.0
        );
    }
    if adopt_from_remote(ctx)? {
        return Ok(());
    }
    let project = create_project(ctx, name)?;
    println!(
        "initialized levi project '{}' ({})",
        project.name, project.id.0
    );
    Ok(())
}

/// Probe the configured git remote for an existing events ref and adopt it
/// rather than minting a fork of the project (spec: safe init).
/// Ok(true): events fetched, world reloaded, "joined" printed.
/// Ok(false): nothing to adopt — no remote, ref absent, or `--no-sync`.
/// Err: remote unreachable, or a fetch that failed after a successful
/// probe. Never falls through to minting on Err: minting when we know (or
/// cannot know) whether events exist is the fork footgun this prevents.
fn adopt_from_remote(ctx: &mut LeviCtx) -> Result<bool> {
    if ctx.no_sync {
        return Ok(false);
    }
    let repo_dir = ctx
        .store
        .repo()
        .workdir()
        .unwrap_or_else(|| ctx.store.repo().git_dir())
        .to_path_buf();
    let remote = ctx.config.remote.clone();
    let has_remote = Command::new("git")
        .args(["remote", "get-url", &remote])
        .current_dir(&repo_dir)
        .output()
        .context("running git")?;
    if !has_remote.status.success() {
        return Ok(false);
    }
    let probe = Command::new("git")
        .args(["ls-remote", "--exit-code", &remote, crate::store::EVENTS_REF])
        .current_dir(&repo_dir)
        .output()
        .context("running git ls-remote")?;
    match probe.status.code() {
        // --exit-code: 2 = remote reachable, no matching ref.
        Some(2) => Ok(false),
        Some(0) => {
            crate::sync::git_leg(ctx)
                .context("remote has levi events but fetching them failed")?;
            ctx.reload()?;
            match &ctx.world.project {
                Some(p) => {
                    println!("joined existing levi project '{}' ({})", p.name, p.id.0);
                    Ok(true)
                }
                None => bail!(
                    "fetched events from '{remote}' but found no project event; \
                     refusing to mint a new project over them"
                ),
            }
        }
        _ => bail!(
            "cannot reach remote '{remote}' to check for an existing levi project: {}\n\
             retry when online, or pass --no-sync to initialize a standalone project",
            String::from_utf8_lossy(&probe.stderr).trim()
        ),
    }
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
