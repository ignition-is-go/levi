//! `levi ls --project <p>` / `--all-projects`: list tasks across projects
//! via the hub (spec 2026-07-21-cross-project-graph-design, Surface 1).
//!
//! Unanchored resolution only — Tasks + StatusChanges, never CommitFacts —
//! so listing is cheap regardless of history. Each JSON row carries `ref`
//! (`<project>/lv-xxxx`), exactly the form `levi dep add --on` accepts.

use std::time::Duration;

use anyhow::Result;
use levi_core::crossproject::statuses_unanchored;
use levi_core::resolve::{Resolution, Status};
use serde_json::json;

use crate::ctx::LeviCtx;

const SCHEMA: &str = "levi.ls/1";
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct ForeignLsOpts {
    pub project: Option<String>,
    pub all_projects: bool,
    pub json: bool,
    pub all: bool,
    pub closed: bool,
    pub label: Option<String>,
}

struct Row {
    project_id: String,
    project: String,
    task_id: String,
    short: String,
    title: String,
    priority: levi_core::Priority,
    status: Status,
    resolution: Resolution,
}

pub fn run(ctx: &LeviCtx, opts: ForeignLsOpts) -> Result<()> {
    let session = crate::foreign::connect(ctx)?;

    // Project registry: name for display, and the id filter for --project.
    let pcount: levi_core::ProjectCount =
        session.report_once(levi_core::CountAllProjects {}, TIMEOUT)?;
    let projects = session.query_at_least(levi_core::GetAllProjects {}, pcount.count, TIMEOUT)?;

    let wanted: Vec<(String, String)> = match &opts.project {
        Some(name_or_id) => {
            let (id, name) = crate::foreign::resolve_project(&session, name_or_id)?;
            vec![(id, name)]
        }
        None => projects
            .iter()
            .map(|p| (p.id.to_string(), p.name.clone()))
            .collect(),
    };

    // Pull tasks + status changes once, hub-wide, then resolve per project.
    let tcount: levi_core::TaskCount = session.report_once(
        levi_core::CountTasks(levi_core::PartialTask::default()),
        TIMEOUT,
    )?;
    let tasks: Vec<levi_core::Task> = session
        .query_at_least(
            levi_core::GetTasksByQuery(levi_core::PartialTask::default()),
            tcount.count,
            TIMEOUT,
        )?
        .iter()
        .map(|t| (**t).clone())
        .collect();
    let ccount: levi_core::StatusChangeCount = session.report_once(
        levi_core::CountStatusChanges(levi_core::PartialStatusChange::default()),
        TIMEOUT,
    )?;
    let changes: Vec<levi_core::StatusChange> = session
        .query_at_least(
            levi_core::GetStatusChangesByQuery(levi_core::PartialStatusChange::default()),
            ccount.count,
            TIMEOUT,
        )?
        .iter()
        .map(|c| (**c).clone())
        .collect();

    let mut rows: Vec<Row> = Vec::new();
    for (project_id, project_name) in &wanted {
        let statuses = statuses_unanchored(&tasks, &changes, project_id);
        for task in tasks.iter().filter(|t| t.project_id == *project_id) {
            let id = task.id.to_string();
            let resolved = statuses[&id];
            let show = if opts.all {
                true
            } else if opts.closed {
                resolved.status == Status::Closed
            } else {
                resolved.status == Status::Open
            };
            if !show {
                continue;
            }
            if let Some(l) = &opts.label
                && !task.labels.contains(l)
            {
                continue;
            }
            rows.push(Row {
                project_id: project_id.clone(),
                project: project_name.clone(),
                task_id: id.clone(),
                short: format!("lv-{}", &id[..id.len().min(8)]),
                title: task.title.clone(),
                priority: task.priority,
                status: resolved.status,
                resolution: resolved.resolution,
            });
        }
    }
    rows.sort_by(|a, b| {
        (a.priority.rank(), a.project.as_str(), a.task_id.as_str()).cmp(&(
            b.priority.rank(),
            b.project.as_str(),
            b.task_id.as_str(),
        ))
    });

    if opts.json {
        let tasks: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.task_id,
                    "short": r.short,
                    "project": r.project,
                    "project_id": r.project_id,
                    "ref": format!("{}/{}", r.project, r.short),
                    "title": r.title,
                    "priority": r.priority.label(),
                    "status": r.status.label(),
                    "resolution": r.resolution.label(),
                })
            })
            .collect();
        println!("{}", json!({"schema": SCHEMA, "tasks": tasks}));
        return Ok(());
    }
    if rows.is_empty() {
        eprintln!("no matching tasks");
        return Ok(());
    }
    for r in &rows {
        // "closed (somewhere)" when a close exists but its branch reachability
        // was not established (Partial); plain status otherwise.
        let status = match (r.status, r.resolution) {
            (Status::Closed, Resolution::Partial) => "closed (somewhere)".to_string(),
            (s, _) => s.label().to_string(),
        };
        println!(
            "{}/{}  {} {:<18} {}",
            r.project,
            r.short,
            r.priority.label(),
            status,
            r.title
        );
    }
    Ok(())
}
