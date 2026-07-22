//! Project browser: `ls`-equivalent filters plus a branch selector fed by
//! RefFacts — view any project's tasks as resolved against any branch.

use leptos::prelude::*;
use levi_core::resolve::{Resolution, Status as TaskStatus};
use levi_core::*;
use myko_leptos::live_query;
use pulse_leptos_ui::{
    Badge, BadgeVariant, EmptyState, Modal, SearchField, Select, Status, StatusVariant, tokens,
};

use crate::resolve_client;

/// Sentinel project id for the "all projects" option.
const ALL_PROJECTS: &str = "__all__";

/// Priority → badge variant (P0 error, P1 warning, else neutral). Shared by
/// the list rows and the detail drawer so a priority always reads the same.
fn priority_badge(p: Priority) -> BadgeVariant {
    match p {
        Priority::P0 => BadgeVariant::Error,
        Priority::P1 => BadgeVariant::Warning,
        _ => BadgeVariant::Neutral,
    }
}

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

    // "all projects" sentinel — resolves every project against its own
    // default branch and shows them together.
    let project_options = Signal::derive(move || {
        let mut opts = vec![(ALL_PROJECTS.to_string(), "all projects".to_string())];
        opts.extend(
            projects
                .get()
                .iter()
                .map(|p| (p.id.to_string(), p.name.clone())),
        );
        opts
    });
    let project_names = Signal::derive(move || {
        projects
            .get()
            .iter()
            .map(|p| (p.id.to_string(), p.name.clone()))
            .collect::<std::collections::HashMap<_, _>>()
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
        let is_all = pid == ALL_PROJECTS;
        let tasks_now = tasks.get();
        let changes_now = changes.get();
        let cf_now = commit_facts.get();
        let refs_now = ref_facts.get();

        // Which (project, head) pairs to resolve. For "all", every project
        // against its own default branch; otherwise the single selection.
        let default_head = |ppid: &str| {
            refs_now
                .iter()
                .filter(|r| r.project_id == ppid)
                .min_by_key(|r| (r.branch != "main", r.branch.clone()))
                .map(|r| r.head.clone())
        };
        let scope: Vec<(String, Option<String>)> = if is_all {
            let mut ps: Vec<String> = tasks_now.iter().map(|t| t.project_id.clone()).collect();
            ps.sort();
            ps.dedup();
            ps.into_iter()
                .map(|p| {
                    let h = default_head(&p);
                    (p, h)
                })
                .collect()
        } else {
            vec![(pid.clone(), head())]
        };

        let mut statuses = std::collections::BTreeMap::new();
        for (ppid, phead) in &scope {
            statuses.extend(resolve_client::statuses(
                &tasks_now,
                &changes_now,
                &cf_now,
                ppid,
                phead.as_deref(),
            ));
        }
        let names = project_names.get();
        let in_scope = |t: &Task| is_all || t.project_id == pid;

        let all_ids: Vec<String> = tasks_now
            .iter()
            .filter(|t| in_scope(t))
            .map(|t| t.id.to_string())
            .collect();
        let want_status = filter_status.get();
        let needle = filter_text.get().to_lowercase();
        let mut rows: Vec<_> = tasks_now
            .iter()
            .filter(|t| in_scope(t))
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
                let project_name = if is_all {
                    names.get(&task.project_id).cloned().unwrap_or_default()
                } else {
                    String::new()
                };
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
                let priority_variant = priority_badge(priority);
                let title_attr = title.clone();
                view! {
                    <div
                        class="levi-row levi-rowlink"
                        on:click=open_drawer
                        style=format!(
                            "padding:{} {};border-bottom:1px solid {};",
                            tokens::SPACING_XS, tokens::SPACING_SM, tokens::BORDER,
                        )
                    >
                        {(!project_name.is_empty()).then(|| view! {
                            <span class="text-muted trunc" style=format!(
                                "font-size:{};width:6rem;white-space:nowrap;",
                                tokens::FONT_SIZE_XS,
                            )>{project_name}</span>
                        })}
                        <span class="text-muted" style=format!(
                            "font-family:{};font-size:{};white-space:nowrap;",
                            tokens::FONT_MONO, tokens::FONT_SIZE_XS,
                        )>
                            {resolve_client::short_id(&all_ids, &id)}
                        </span>
                        <Badge variant=priority_variant>{priority.label()}</Badge>
                        <Status variant=status_variant>{status_label}</Status>
                        <span class="trunc" title=title_attr style=format!(
                            "flex:1;font-size:{};", tokens::FONT_SIZE_SM,
                        )>{title}</span>
                        {(!labels.is_empty()).then(|| view! {
                            <span class="text-muted" style=format!(
                                "font-size:{};white-space:nowrap;",
                                tokens::FONT_SIZE_XS,
                            )>
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

        // Own the fields up front so the reactive child closures below don't
        // each try to move `task`.
        let priority = task.priority;
        let title = task.title.clone();
        let body = task.body.clone();
        let created_date = task.created.get(..10).unwrap_or("").to_string();
        let created_by = task.created_by_dev.clone();
        let meta_style = format!("font-size:{};", tokens::FONT_SIZE_XS);
        view! {
            <Modal open=drawer_open title=title>
                <div style=format!(
                    "display:flex;flex-direction:column;gap:{};",
                    tokens::SPACING_MD,
                )>
                    <div class="levi-row" style=format!("gap:{};", tokens::SPACING_SM)>
                        <Badge variant=priority_badge(priority)>{priority.label()}</Badge>
                        <span class="text-muted" style=meta_style.clone()>
                            {format!("created {created_date} by {created_by}")}
                        </span>
                    </div>

                    {(!body.is_empty()).then(|| view! {
                        <div style=format!(
                            "white-space:pre-wrap;color:{};font-size:{};",
                            tokens::TEXT_SECONDARY, tokens::FONT_SIZE_SM,
                        )>{body}</div>
                    })}

                    {(!blocked_by.is_empty() || !blocks.is_empty()).then(|| view! {
                        <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_2XS)>
                            {(!blocked_by.is_empty()).then(|| view! {
                                <div style=meta_style.clone()>
                                    <span class="label">"blocked by "</span>{blocked_by.join(", ")}
                                </div>
                            })}
                            {(!blocks.is_empty()).then(|| view! {
                                <div style=meta_style.clone()>
                                    <span class="label">"blocks "</span>{blocks.join(", ")}
                                </div>
                            })}
                        </div>
                    })}

                    <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_2XS)>
                        <div class="label">"status history"</div>
                        {history
                            .into_iter()
                            .map(|change| {
                                let anchor = change
                                    .anchor_commit
                                    .as_ref()
                                    .map(|a| format!(" @ {}", &a[..8.min(a.len())]))
                                    .unwrap_or_else(|| " (everywhere)".into());
                                let (label, variant) = match change.to_status {
                                    StatusKind::Closed => ("closed", StatusVariant::Neutral),
                                    StatusKind::Reopened => ("reopened", StatusVariant::Success),
                                };
                                view! {
                                    <div class="levi-row" style=format!("gap:{};", tokens::SPACING_SM)>
                                        <Status variant=variant>{label}</Status>
                                        <span class="text-muted" style=format!(
                                            "font-family:{};font-size:{};",
                                            tokens::FONT_MONO, tokens::FONT_SIZE_XS,
                                        )>
                                            {format!(
                                                "{}{anchor} by {}",
                                                change.created.get(..19).unwrap_or(""),
                                                change.by_dev,
                                            )}
                                        </span>
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>

                    {(!task_comments.is_empty()).then(|| view! {
                        <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_SM)>
                            <div class="label">"comments"</div>
                            {task_comments
                                .into_iter()
                                .map(|comment| {
                                    view! {
                                        <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_2XS)>
                                            <span class="text-muted" style=format!(
                                                "font-family:{};font-size:{};",
                                                tokens::FONT_MONO, tokens::FONT_SIZE_XS,
                                            )>
                                                {format!("{} · {}", comment.created.get(..19).unwrap_or(""), comment.by_dev)}
                                            </span>
                                            <span style=format!(
                                                "white-space:pre-wrap;font-size:{};",
                                                tokens::FONT_SIZE_SM,
                                            )>{comment.body.clone()}</span>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </div>
                    })}
                </div>
            </Modal>
        }
        .into_any()
    };

    view! {
        <div style=format!(
            "padding:{};height:100%;box-sizing:border-box;display:flex;flex-direction:column;gap:{};",
            tokens::SPACING_MD, tokens::SPACING_SM,
        )>
            <div style=format!(
                "flex:0 0 auto;display:flex;gap:{};flex-wrap:wrap;align-items:center;",
                tokens::SPACING_SM,
            )>
                <Select
                    value=Signal::derive(move || sel_project.get())
                    on_change=Callback::new(move |v| sel_project.set(v))
                    options=project_options
                    placeholder="project"
                    class="levi-select-wide"
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
            // No pane: the workspace already frames this. The list is a plain
            // scroll region; each row carries its own separator.
            <div style="flex:1;min-height:0;overflow-y:auto;">{rows}</div>
            {drawer}
        </div>
    }
}
