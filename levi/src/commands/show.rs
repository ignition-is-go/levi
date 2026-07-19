use anyhow::Result;
use chrono::Utc;
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::resolve::{Ancestry, AncestorSet, Resolution, effective_status};
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::{SCHEMA_SHOW, claim_json};

pub fn run(ctx: &LeviCtx, id_input: &str, json: bool) -> Result<()> {
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let task = &ctx.world.tasks[&task_id];
    let now = Utc::now();
    let mut anc = ctx.ancestors_for(None)?;

    let changes = ctx.world.changes_for(&task_id);
    let resolved = effective_status(&changes, &mut anc, Resolution::Exact);

    // Per-change view: does each change apply on this checkout?
    let history: Vec<_> = changes
        .iter()
        .map(|c| {
            let applies = match &c.anchor_commit {
                None => "yes",
                Some(sha) => match anc.contains(sha) {
                    Ancestry::Yes => "yes",
                    Ancestry::No => "no",
                    Ancestry::Unknown => "unknown",
                },
            };
            json!({
                "to": match c.to_status { levi_core::StatusKind::Closed => "closed", levi_core::StatusKind::Reopened => "reopened" },
                "anchor": c.anchor_commit,
                "at": c.at,
                "by": c.by_dev,
                "applies_here": applies,
            })
        })
        .collect();

    let comments: Vec<_> = ctx
        .world
        .comments
        .iter()
        .filter(|c| &*c.task_id == task_id)
        .map(|c| json!({"body": c.body, "by": c.by_dev, "at": c.at}))
        .collect();

    let mut statuses_cache = std::collections::BTreeMap::new();
    let mut status_of = |ctx: &LeviCtx, anc: &mut dyn AncestorSet, id: &str| -> String {
        statuses_cache
            .entry(id.to_string())
            .or_insert_with(|| {
                let mut anc = anc;
                effective_status(&ctx.world.changes_for(id), &mut anc, Resolution::Exact)
                    .status
                    .label()
                    .to_string()
            })
            .clone()
    };

    let mut blocked_by = Vec::new();
    let mut blocks = Vec::new();
    for dep in ctx.world.deps.values() {
        if &*dep.blocked_task_id == task_id && ctx.world.tasks.contains_key(&*dep.blocker_task_id)
        {
            let status = status_of(ctx, &mut anc, &dep.blocker_task_id);
            blocked_by.push(json!({
                "id": dep.blocker_task_id,
                "short": short_id(&ctx.world, &dep.blocker_task_id),
                "status": status,
            }));
        }
        if &*dep.blocker_task_id == task_id && ctx.world.tasks.contains_key(&*dep.blocked_task_id)
        {
            let status = status_of(ctx, &mut anc, &dep.blocked_task_id);
            blocks.push(json!({
                "id": dep.blocked_task_id,
                "short": short_id(&ctx.world, &dep.blocked_task_id),
                "status": status,
            }));
        }
    }

    let claim = claim_json(&ctx.world, &task_id, now);
    if json {
        println!(
            "{}",
            json!({
                "schema": SCHEMA_SHOW,
                "id": task_id,
                "short": short_id(&ctx.world, &task_id),
                "title": task.title,
                "body": task.body,
                "status": resolved.status.label(),
                "resolution": resolved.resolution.label(),
                "priority": task.priority.label(),
                "labels": task.labels,
                "created_at": task.created_at,
                "created_by": task.created_by_dev,
                "claim": claim,
                "blocked_by": blocked_by,
                "blocks": blocks,
                "comments": comments,
                "history": history,
            })
        );
        return Ok(());
    }

    println!(
        "{} {} [{}] {}",
        short_id(&ctx.world, &task_id),
        task.priority.label(),
        resolved.status.label(),
        task.title
    );
    if !task.body.is_empty() {
        println!("\n{}", task.body);
    }
    if !task.labels.is_empty() {
        println!("labels: {}", task.labels.join(", "));
    }
    if !claim.is_null() {
        println!("claimed by {} on {} ({})", claim["dev"], claim["machine"], claim["worktree"]);
    }
    if !blocked_by.is_empty() {
        println!("blocked by:");
        for b in &blocked_by {
            println!("  {} [{}]", b["short"].as_str().unwrap_or_default(), b["status"].as_str().unwrap_or_default());
        }
    }
    if !blocks.is_empty() {
        println!("blocks:");
        for b in &blocks {
            println!("  {} [{}]", b["short"].as_str().unwrap_or_default(), b["status"].as_str().unwrap_or_default());
        }
    }
    if !history.is_empty() {
        println!("history:");
        for h in &history {
            let anchor = h["anchor"].as_str().map(|a| format!(" @ {}", &a[..a.len().min(8)])).unwrap_or_default();
            println!(
                "  {} by {} at {}{anchor} (applies here: {})",
                h["to"].as_str().unwrap_or_default(),
                h["by"].as_str().unwrap_or_default(),
                h["at"].as_str().unwrap_or_default(),
                h["applies_here"].as_str().unwrap_or_default(),
            );
        }
    }
    if !comments.is_empty() {
        println!("comments:");
        for c in &comments {
            println!("  [{}] {}: {}", c["at"].as_str().unwrap_or_default(), c["by"].as_str().unwrap_or_default(), c["body"].as_str().unwrap_or_default());
        }
    }
    Ok(())
}
