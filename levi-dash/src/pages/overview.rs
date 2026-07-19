use leptos::prelude::*;
use levi_core::resolve::Status as TaskStatus;
use levi_core::*;
use myko_leptos::live_query;
use pulse_leptos_ui::{Badge, BadgeVariant, EmptyState, Pane, tokens};

use crate::resolve_client;

#[component]
pub fn Overview() -> impl IntoView {
    let projects = live_query(|| GetAllProjects {});
    let tasks = live_query(|| GetAllTasks {});
    let changes = live_query(|| GetAllStatusChanges {});
    let commit_facts = live_query(|| GetAllCommitFacts {});
    let ref_facts = live_query(|| GetAllRefFacts {});
    let entries = live_query(|| GetAllLogEntrys {});

    let cards = move || {
        let tasks = tasks.get();
        let changes = changes.get();
        let commit_facts = commit_facts.get();
        let ref_facts = ref_facts.get();
        let projects = projects.get();
        if projects.is_empty() {
            return view! { <EmptyState message="no projects synced yet".to_string() /> }
                .into_any();
        }
        projects
            .into_iter()
            .map(|project| {
                let pid = project.id.to_string();
                let head = ref_facts
                    .iter()
                    .filter(|r| r.project_id == pid)
                    .min_by_key(|r| (r.branch != "main", r.branch.clone()))
                    .map(|r| r.head.clone());
                let statuses = resolve_client::statuses(
                    &tasks,
                    &changes,
                    &commit_facts,
                    &pid,
                    head.as_deref(),
                );
                let open = statuses.values().filter(|s| s.status == TaskStatus::Open).count();
                let closed = statuses.len() - open;
                let p0: Vec<String> = tasks
                    .iter()
                    .filter(|t| {
                        t.project_id == pid
                            && t.priority == Priority::P0
                            && statuses
                                .get(&*t.id.0)
                                .map(|s| s.status == TaskStatus::Open)
                                .unwrap_or(false)
                    })
                    .map(|t| t.title.clone())
                    .collect();
                let head_badge = head
                    .map(|h| format!("@ {}", &h[..8.min(h.len())]))
                    .unwrap_or_else(|| "no branch facts".into());
                view! {
                    <Pane title=project.name.clone()>
                        <div style="display:flex;gap:16px;align-items:baseline;">
                            <div>
                                <div class="value" style=format!("color:{};", tokens::SUCCESS)>{open}</div>
                                <div class="label">"open"</div>
                            </div>
                            <div>
                                <div class="value text-muted">{closed}</div>
                                <div class="label">"closed"</div>
                            </div>
                            <Badge variant=BadgeVariant::Neutral>{head_badge}</Badge>
                        </div>
                        {(!p0.is_empty()).then(|| view! {
                            <div style="margin-top:10px;">
                                <div class="label" style=format!("color:{};", tokens::ERROR)>"P0 open"</div>
                                {p0.into_iter()
                                    .map(|t| view! {
                                        <div style=format!("color:{};", tokens::ERROR)>{t}</div>
                                    })
                                    .collect_view()}
                            </div>
                        })}
                    </Pane>
                }
            })
            .collect_view()
            .into_any()
    };

    let feed = move || {
        let mut entries = entries.get();
        entries.sort_by(|a, b| b.created.cmp(&a.created));
        if entries.is_empty() {
            return view! { <EmptyState message="no activity yet".to_string() /> }.into_any();
        }
        entries
            .into_iter()
            .take(25)
            .map(|entry| {
                let line = entry
                    .unwrap_event()
                    .map(|ev| {
                        let what = ev
                            .item
                            .get("title")
                            .or_else(|| ev.item.get("body"))
                            .and_then(|v| v.as_str())
                            .map(|s| format!(" — {s}"))
                            .unwrap_or_default();
                        format!("{} {:?}{}", ev.item_type, ev.change_type, what)
                    })
                    .unwrap_or_else(|_| "undecodable event".into());
                view! {
                    <div style="display:flex;gap:10px;padding:3px 0;align-items:baseline;">
                        <span class="text-muted" style="font-size:11px;white-space:nowrap;">
                            {entry.created.get(..19).unwrap_or("").to_string()}
                        </span>
                        <span>{line}</span>
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    view! {
        <div style="padding:12px;height:100%;overflow-y:auto;display:flex;flex-direction:column;gap:12px;">
            <div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:12px;">
                {cards}
            </div>
            <Pane title="Activity".to_string()>{feed}</Pane>
        </div>
    }
}
