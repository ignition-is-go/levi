//! `levi check-claims` — CI policy for branch-owned tasks.
//!
//! A claim is durable evidence that a branch took responsibility for a task,
//! even after the live advisory claim is dropped. The check groups historical
//! claim SETs by their recorded symbolic Git ref, then resolves every matching
//! task against HEAD or an explicit `--at` revision.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use levi_core::ids::short_id;
use levi_core::resolve::{Resolution, Status};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::SCHEMA_CHECK_CLAIMS;

pub fn run(
    ctx: &LeviCtx,
    git_ref: Option<String>,
    at: Option<String>,
    json_output: bool,
) -> Result<()> {
    ctx.project_id()?;
    let git_ref = match git_ref {
        Some(name) => normalize_branch_ref(&name)?,
        None => ctx.current_git_ref()?,
    };

    let task_ids: BTreeSet<&str> = ctx
        .world
        .claim_history
        .iter()
        .filter(|claim| claim.git_ref == git_ref)
        .map(|claim| claim.task_id.as_ref())
        .collect();

    let tested_commit = ctx.rev(at.as_deref().unwrap_or("HEAD"))?.to_string();
    let mut anc = ctx.ancestors_for(Some(&tested_commit))?;
    let statuses = ctx.statuses(&mut anc, Resolution::Exact);
    let mut tasks = Vec::with_capacity(task_ids.len());
    let mut open = Vec::new();
    for task_id in task_ids {
        // A selected historical claim may reference a task this binary
        // cannot materialize. Treat that as a policy failure, never a silent
        // pass.
        let Some(task) = ctx.world.tasks.get(task_id) else {
            open.push(task_id.to_string());
            tasks.push(json!({
                "id": task_id,
                "short": format!("lv-{}", task_id.chars().take(4).collect::<String>()),
                "title": null,
                "status": "unknown",
                "resolution": "partial",
            }));
            continue;
        };
        let status = &statuses[task_id];
        if status.status != Status::Closed || status.resolution == Resolution::Partial {
            open.push(task_id.to_string());
        }
        tasks.push(json!({
            "id": task_id,
            "short": short_id(&ctx.world, task_id),
            "title": task.title,
            "status": status.status.label(),
            "resolution": status.resolution.label(),
        }));
    }

    if json_output {
        println!(
            "{}",
            json!({
                "schema": SCHEMA_CHECK_CLAIMS,
                "git_ref": git_ref,
                "tested_commit": tested_commit,
                "ok": open.is_empty(),
                "tasks": tasks,
            })
        );
    } else if open.is_empty() {
        println!(
            "all {} task(s) claimed by {git_ref} are closed at {}",
            tasks.len(),
            &tested_commit[..8]
        );
    } else {
        eprintln!(
            "tasks claimed by {git_ref} that are not closed at {}:",
            &tested_commit[..8]
        );
        for task in tasks
            .iter()
            .filter(|task| task["status"] != "closed" || task["resolution"] == "partial")
        {
            eprintln!(
                "  {} {:<7} {}",
                task["short"].as_str().unwrap_or("lv-????"),
                task["status"].as_str().unwrap_or("unknown"),
                task["title"].as_str().unwrap_or("<unknown task>")
            );
        }
    }

    if !open.is_empty() {
        bail!(
            "{} task(s) claimed by {git_ref} are not closed at {}",
            open.len(),
            &tested_commit[..8]
        );
    }
    Ok(())
}

fn normalize_branch_ref(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Git ref cannot be empty");
    }
    if name == "refs/heads" || name.starts_with("refs/heads//") {
        bail!("'{name}' is not a branch ref");
    }
    if name.starts_with("refs/") && !name.starts_with("refs/heads/") {
        bail!("'{name}' is not a branch ref; expected refs/heads/<branch>");
    }
    let full = if name.starts_with("refs/heads/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    };
    if full == "refs/heads/" {
        bail!("'{name}' is not a branch ref");
    }
    let _: gix::refs::FullName = full
        .as_str()
        .try_into()
        .with_context(|| format!("'{name}' is not a valid Git branch ref"))?;
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_short_branch_names() {
        assert_eq!(
            normalize_branch_ref("feature/x").unwrap(),
            "refs/heads/feature/x"
        );
        assert_eq!(
            normalize_branch_ref("refs/heads/feature/x").unwrap(),
            "refs/heads/feature/x"
        );
        assert!(normalize_branch_ref("refs/tags/v1").is_err());
        assert!(normalize_branch_ref("refs/heads/").is_err());
        assert!(normalize_branch_ref("bad branch").is_err());
        assert!(normalize_branch_ref("").is_err());
    }
}
