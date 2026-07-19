//! Project browser: `ls`-equivalent filters plus a branch selector fed by
//! RefFacts — view any project's tasks as resolved against any branch.

use leptos::prelude::*;
use levi_core::resolve::{Resolution, Status as TaskStatus};
use levi_core::*;
use myko_leptos::live_query;
use pulse_leptos_ui::{
    Badge, BadgeVariant, EmptyState, Modal, Pane, SearchField, Select, Status, StatusVariant,
    tokens,
};

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

    let sel_project = RwSignal::new(String::new());
    let sel_branch = RwSignal::new(String::new());
    let filter_status = RwSignal::new("open".to_string());
    let filter_text = RwSignal::new(String::new());
    let drawer_task = RwSignal::new(String::new());
    let drawer_open = RwSignal::new(false);

    // Default to the first project once data arrives.
    Effect::new(move || {
        if sel_project.get().is_empty()
            && let Some(p) = projects.get().first()
        {
            sel_project.set(p.id.to_string());
        }
    });

    let project_options = Signal::derive(move || {
        projects
            .get()
            .iter()
            .map(|p| (p.id.to_string(), p.name.clone()))
            .collect::<Vec<_>>()
    });

    let branches = move || {
        let pid = sel_project.get();
        let mut branches: Vec<String> = ref_facts
            .get()
            .iter()
            .filter(|r| r.project_id == pid)
            .map(|r| r.branch.clone())
            .collect();
        branches.sort_by_key(|b| (b != "main", b.clone()));
        branches
    };
    let branch_options = Signal::derive(move || {
        branches()
            .into_iter()
            .map(|b| (b.clone(), b))
            .collect::<Vec<_>>()
    });

    let head = move || {
        let pid = sel_project.get();
        let branch = {
            let chosen = sel_branch.get();
            if chosen.is_empty() {
                branches().first().cloned()?
            } else {
                chosen
            }
        };
        ref_facts
            .get()
            .iter()
            .find(|r| r.project_id == pid && r.branch == branch)
            .map(|r| r.head.clone())
    };

    let rows = move || {
        let pid = sel_project.get();
        let tasks_now = tasks.get();
        let statuses = resolve_client::statuses(
            &tasks_now,
            &changes.get(),
            &commit_facts.get(),
            &pid,
            head().as_deref(),
        );
        let all_ids: Vec<String> = tasks_now
            .iter()
            .filter(|t| t.project_id == pid)
            .map(|t| t.id.to_string())
            .collect();
        let want_status = filter_status.get();
        let needle = filter_text.get().to_lowercase();
        let mut rows: Vec<_> = tasks_now
            .iter()
            .filter(|t| t.project_id == pid)
            .filter(|t| {
                let resolved = &statuses[&*t.id.0];
                match want_status.as_str() {
                    "open" => resolved.status == TaskStatus::Open,
                    "closed" => resolved.status == TaskStatus::Closed,
                    _ => true,
                }
            })
            .filter(|t| {
                needle.is_empty()
                    || t.title.to_lowercase().contains(&needle)
                    || t.labels.iter().any(|l| l.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            (a.priority.rank(), a.created.as_str()).cmp(&(b.priority.rank(), b.created.as_str()))
        });
        if rows.is_empty() {
            return view! { <EmptyState message="no matching tasks".to_string() /> }.into_any();
        }
        rows.into_iter()
            .map(|task| {
                let id = task.id.to_string();
                let resolved = statuses[&id];
                let title = task.title.clone();
                let labels = task.labels.clone();
                let priority = task.priority;
                let open_drawer = {
                    let id = id.clone();
                    move |_| {
                        drawer_task.set(id.clone());
                        drawer_open.set(true);
                    }
                };
                let (status_variant, status_label) = match resolved.status {
                    TaskStatus::Open => (StatusVariant::Success, "open"),
                    TaskStatus::Closed => (StatusVariant::Neutral, "closed"),
                };
                let priority_variant = match priority {
                    Priority::P0 => BadgeVariant::Error,
                    Priority::P1 => BadgeVariant::Warning,
                    _ => BadgeVariant::Neutral,
                };
                view! {
                    <div
                        on:click=open_drawer
                        style=format!(
                            "display:flex;gap:10px;align-items:center;padding:5px 8px;\
                             cursor:pointer;border-bottom:1px solid {};",
                            tokens::BORDER
                        )
                    >
                        <span class="text-muted" style="font-family:var(--font-mono);font-size:11px;">
                            {resolve_client::short_id(&all_ids, &id)}
                        </span>
                        <Badge variant=priority_variant>{priority.label()}</Badge>
                        <Status variant=status_variant>{status_label}</Status>
                        <span style="flex:1;">{title}</span>
                        {(!labels.is_empty()).then(|| view! {
                            <span class="text-muted" style="font-size:11px;">
                                {format!("[{}]", labels.join(", "))}
                            </span>
                        })}
                        {(resolved.resolution == Resolution::Partial).then(|| view! {
                            <Badge variant=BadgeVariant::Warning>"unknown anchor"</Badge>
                        })}
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    let drawer = move || {
        let task_id = drawer_task.get();
        let tasks_now = tasks.get();
        let Some(task) = tasks_now.iter().find(|t| *t.id.0 == task_id) else {
            return ().into_any();
        };
        let task = task.clone();
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
            .map(|d| {
                let short = &d.blocker_task_id[..8.min(d.blocker_task_id.len())];
                match &d.blocker_project_id {
                    // Cross-project blocker: name the project and carry the
                    // agent-authored consumption note.
                    Some(project) => {
                        let via = d
                            .via
                            .as_deref()
                            .map(|v| format!(" (via: {v})"))
                            .unwrap_or_default();
                        format!("{}/{short}{via}", &project[..8.min(project.len())])
                    }
                    None => short.to_string(),
                }
            })
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

        view! {
            <Modal open=drawer_open title=task.title.clone()>
                <div class="text-muted" style="margin-bottom:8px;">
                    {format!(
                        "{} · created {} by {}",
                        task.priority.label(),
                        task.created.get(..10).unwrap_or(""),
                        task.created_by_dev
                    )}
                </div>
                {(!task.body.is_empty()).then(|| view! { <div style="margin-bottom:10px;">{task.body.clone()}</div> })}
                {(!blocked_by.is_empty()).then(|| view! {
                    <div><span class="label">"blocked by "</span>{blocked_by.join(", ")}</div>
                })}
                {(!blocks.is_empty()).then(|| view! {
                    <div><span class="label">"blocks "</span>{blocks.join(", ")}</div>
                })}
                <div class="label" style="margin-top:10px;">"status history"</div>
                {history
                    .into_iter()
                    .map(|change| {
                        let anchor = change
                            .anchor_commit
                            .as_ref()
                            .map(|a| format!(" @ {}", &a[..8.min(a.len())]))
                            .unwrap_or_else(|| " (everywhere)".into());
                        view! {
                            <div style="display:flex;gap:8px;padding:2px 0;">
                                <span>{match change.to_status {
                                    StatusKind::Closed => "closed",
                                    StatusKind::Reopened => "reopened",
                                }}</span>
                                <span class="text-muted" style="font-size:11px;">
                                    {format!(
                                        "{}{anchor} by {}",
                                        change.created.get(..19).unwrap_or(""),
                                        change.by_dev
                                    )}
                                </span>
                            </div>
                        }
                    })
                    .collect_view()}
                {(!task_comments.is_empty()).then(|| view! {
                    <div class="label" style="margin-top:10px;">"comments"</div>
                })}
                {task_comments
                    .into_iter()
                    .map(|comment| {
                        view! {
                            <div style="padding:2px 0;">
                                <span class="text-muted" style="font-size:11px;">
                                    {format!("{} {} ", comment.created.get(..19).unwrap_or(""), comment.by_dev)}
                                </span>
                                <span>{comment.body.clone()}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </Modal>
        }
        .into_any()
    };

    view! {
        <div style="padding:12px;height:100%;overflow-y:auto;display:flex;flex-direction:column;gap:10px;">
            <div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
                <Select
                    value=Signal::derive(move || sel_project.get())
                    on_change=Callback::new(move |v| sel_project.set(v))
                    options=project_options
                    placeholder="project"
                />
                <Select
                    value=Signal::derive(move || sel_branch.get())
                    on_change=Callback::new(move |v| sel_branch.set(v))
                    options=branch_options
                    placeholder="branch"
                />
                <Select
                    value=Signal::derive(move || filter_status.get())
                    on_change=Callback::new(move |v| filter_status.set(v))
                    options=Signal::derive(|| {
                        vec![
                            ("open".to_string(), "Open".to_string()),
                            ("closed".to_string(), "Closed".to_string()),
                            ("all".to_string(), "All".to_string()),
                        ]
                    })
                    placeholder="status"
                />
                <SearchField value=filter_text placeholder="filter by title or label" />
            </div>
            <Pane>{rows}</Pane>
            {drawer}
        </div>
    }
}
