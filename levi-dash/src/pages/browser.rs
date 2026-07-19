//! Project browser: `ls`-equivalent filters plus a branch selector fed by
//! RefFacts — view any project's tasks as resolved against any branch.

use leptos::prelude::*;
use levi_core::resolve::{Resolution, Status};
use levi_core::*;
use myko_leptos::live_query;

use crate::resolve_client;

#[component]
pub fn Browser() -> impl IntoView {
    let projects = live_query(|| GetAllProjects {});
    let tasks = live_query(|| GetAllTasks {});
    let changes = live_query(|| GetAllStatusChanges {});
    let commit_facts = live_query(|| GetAllCommitFacts {});
    let ref_facts = live_query(|| GetAllRefFacts {});
    let comments = live_query(|| GetAllComments {});
    let deps = live_query(|| GetAllDependencys {});

    let sel_project: RwSignal<Option<String>> = RwSignal::new(None);
    let sel_branch: RwSignal<Option<String>> = RwSignal::new(None);
    let filter_status = RwSignal::new("open".to_string());
    let filter_label = RwSignal::new(String::new());
    let drawer: RwSignal<Option<String>> = RwSignal::new(None);

    // Default to the first project once data arrives.
    Effect::new(move || {
        if sel_project.get().is_none()
            && let Some(p) = projects.get().first()
        {
            sel_project.set(Some(p.id.to_string()));
        }
    });

    let branches = move || {
        let pid = sel_project.get().unwrap_or_default();
        let mut branches: Vec<String> = ref_facts
            .get()
            .iter()
            .filter(|r| r.project_id == pid)
            .map(|r| r.branch.clone())
            .collect();
        branches.sort_by_key(|b| (b != "main", b.clone()));
        branches
    };

    let head = move || {
        let pid = sel_project.get().unwrap_or_default();
        let branch = sel_branch.get().or_else(|| branches().first().cloned())?;
        ref_facts
            .get()
            .iter()
            .find(|r| r.project_id == pid && r.branch == branch)
            .map(|r| r.head.clone())
    };

    let rows = move || {
        let pid = sel_project.get().unwrap_or_default();
        let tasks_now = tasks.get();
        let statuses = resolve_client::statuses(
            &tasks_now,
            &changes.get(),
            &commit_facts.get(),
            &pid,
            head().as_deref(),
        );
        let want_status = filter_status.get();
        let want_label = filter_label.get();
        let mut rows: Vec<_> = tasks_now
            .iter()
            .filter(|t| t.project_id == pid)
            .filter(|t| {
                let resolved = &statuses[&*t.id.0];
                match want_status.as_str() {
                    "open" => resolved.status == Status::Open,
                    "closed" => resolved.status == Status::Closed,
                    _ => true,
                }
            })
            .filter(|t| want_label.is_empty() || t.labels.iter().any(|l| l.contains(&want_label)))
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            (a.priority.rank(), a.created.as_str()).cmp(&(b.priority.rank(), b.created.as_str()))
        });
        rows.into_iter()
            .map(|task| {
                let id = task.id.to_string();
                let resolved = statuses[&id];
                let open_drawer = {
                    let id = id.clone();
                    move |_| drawer.set(Some(id.clone()))
                };
                view! {
                    <div class="row clickable" on:click=open_drawer>
                        <span class="badge">{format!("lv-{}", &id[..4.min(id.len())])}</span>
                        <span class=match task.priority {
                            Priority::P0 => "p0",
                            Priority::P1 => "p1",
                            _ => "muted",
                        }>{task.priority.label()}</span>
                        <span class=if resolved.status == Status::Open { "open" } else { "closed" }>
                            {resolved.status.label()}
                        </span>
                        <span>{task.title.clone()}</span>
                        {(!task.labels.is_empty())
                            .then(|| view! { <span class="muted">{format!("[{}]", task.labels.join(", "))}</span> })}
                        {(resolved.resolution == Resolution::Partial)
                            .then(|| view! { <span class="warn">"⚠ unknown anchor"</span> })}
                    </div>
                }
            })
            .collect_view()
    };

    let drawer_view = move || {
        let task_id = drawer.get()?;
        let task = tasks.get().iter().find(|t| *t.id.0 == task_id)?.clone();
        let mut task_comments: Vec<_> = comments
            .get()
            .iter()
            .filter(|c| *c.task_id == task_id)
            .cloned()
            .collect();
        task_comments.sort_by(|a, b| a.created.cmp(&b.created));
        let deps_now = deps.get();
        let blocked_by: Vec<String> = deps_now
            .iter()
            .filter(|d| *d.blocked_task_id == task_id)
            .map(|d| d.blocker_task_id[..8.min(d.blocker_task_id.len())].to_string())
            .collect();
        let blocks: Vec<String> = deps_now
            .iter()
            .filter(|d| *d.blocker_task_id == task_id)
            .map(|d| d.blocked_task_id[..8.min(d.blocked_task_id.len())].to_string())
            .collect();
        let mut history: Vec<_> = changes
            .get()
            .iter()
            .filter(|c| *c.task_id == task_id)
            .cloned()
            .collect();
        history.sort_by(|a, b| (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0)));

        Some(view! {
            <div class="drawer">
                <button class="close" on:click=move |_| drawer.set(None)>"✕"</button>
                <h2>{task.title.clone()}</h2>
                <div class="muted">{format!("{} · created {} by {}", task.priority.label(),
                    task.created.get(..10).unwrap_or(""), task.created_by_dev)}</div>
                {(!task.body.is_empty()).then(|| view! { <div class="section">{task.body.clone()}</div> })}
                {(!blocked_by.is_empty()).then(|| view! {
                    <div class="section"><h4>"blocked by"</h4>{blocked_by.join(", ")}</div>
                })}
                {(!blocks.is_empty()).then(|| view! {
                    <div class="section"><h4>"blocks"</h4>{blocks.join(", ")}</div>
                })}
                <div class="section">
                    <h4>"status history"</h4>
                    {history
                        .into_iter()
                        .map(|change| {
                            let anchor = change
                                .anchor_commit
                                .as_ref()
                                .map(|a| format!(" @ {}", &a[..8.min(a.len())]))
                                .unwrap_or_else(|| " (everywhere)".into());
                            view! {
                                <div class="row">
                                    <span>{match change.to_status {
                                        StatusKind::Closed => "closed",
                                        StatusKind::Reopened => "reopened",
                                    }}</span>
                                    <span class="muted">{format!("{}{anchor} by {}",
                                        change.created.get(..19).unwrap_or(""), change.by_dev)}</span>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="section">
                    <h4>"comments"</h4>
                    {task_comments
                        .into_iter()
                        .map(|comment| {
                            view! {
                                <div class="row">
                                    <span class="muted">{format!("{} {}", comment.created.get(..19).unwrap_or(""), comment.by_dev)}</span>
                                    <span>{comment.body.clone()}</span>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
        })
    };

    view! {
        <div class="toolbar">
            <select on:change=move |ev| sel_project.set(Some(event_target_value(&ev)))>
                {move || {
                    let selected = sel_project.get();
                    projects
                        .get()
                        .into_iter()
                        .map(|p| {
                            let id = p.id.to_string();
                            let is_selected = selected.as_deref() == Some(&*id);
                            view! { <option value=id.clone() selected=is_selected>{p.name.clone()}</option> }
                        })
                        .collect_view()
                }}
            </select>
            <select on:change=move |ev| sel_branch.set(Some(event_target_value(&ev)))>
                {move || {
                    let selected = sel_branch.get();
                    branches()
                        .into_iter()
                        .map(|branch| {
                            let is_selected = selected.as_deref() == Some(branch.as_str())
                                || (selected.is_none() && branch == "main");
                            view! { <option value=branch.clone() selected=is_selected>{branch.clone()}</option> }
                        })
                        .collect_view()
                }}
            </select>
            <select on:change=move |ev| filter_status.set(event_target_value(&ev))>
                <option value="open" selected>"open"</option>
                <option value="closed">"closed"</option>
                <option value="all">"all"</option>
            </select>
            <input
                placeholder="filter by label"
                on:input=move |ev| filter_label.set(event_target_value(&ev))
            />
        </div>
        <div>{rows}</div>
        {drawer_view}
    }
}
