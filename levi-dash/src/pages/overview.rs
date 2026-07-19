use leptos::prelude::*;
use levi_core::resolve::Status;
use levi_core::*;
use myko_leptos::live_query;

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
        projects
            .get()
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
                let open = statuses.values().filter(|s| s.status == Status::Open).count();
                let closed = statuses.len() - open;
                let p0: Vec<String> = tasks
                    .iter()
                    .filter(|t| {
                        t.project_id == pid
                            && t.priority == Priority::P0
                            && statuses.get(&*t.id.0).map(|s| s.status == Status::Open).unwrap_or(false)
                    })
                    .map(|t| t.title.clone())
                    .collect();
                view! {
                    <div class="card">
                        <h3>{project.name.clone()}</h3>
                        <div class="row">
                            <span class="num open">{open}</span><span class="muted">"open"</span>
                            <span class="num closed">{closed}</span><span class="muted">"closed"</span>
                            <span class="badge">{head.map(|h| format!("@ {}", &h[..8.min(h.len())])).unwrap_or_else(|| "no branch facts".into())}</span>
                        </div>
                        {(!p0.is_empty()).then(|| view! {
                            <div class="section">
                                <h4 class="p0">"P0 open"</h4>
                                {p0.into_iter().map(|t| view! { <div class="row p0">{t}</div> }).collect_view()}
                            </div>
                        })}
                    </div>
                }
            })
            .collect_view()
    };

    let feed = move || {
        let mut entries = entries.get();
        entries.sort_by(|a, b| b.created.cmp(&a.created));
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
                    <div class="row">
                        <span class="muted">{entry.created.get(..19).unwrap_or("").to_string()}</span>
                        <span>{line}</span>
                    </div>
                }
            })
            .collect_view()
    };

    view! {
        <div class="cards">{cards}</div>
        <div class="feed">
            <h4 class="muted">"activity"</h4>
            {feed}
        </div>
    }
}
