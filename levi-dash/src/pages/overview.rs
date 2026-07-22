use leptos::prelude::*;
use levi_core::resolve::Status as TaskStatus;
use levi_core::*;
use myko_leptos::live_query;
use pulse_leptos_ui::{Badge, BadgeVariant, EmptyState, Pane, Status, StatusVariant, tokens};

use crate::resolve_client;

/// A plain section heading — not everything needs a bordered pane. Matches
/// the pane-header weight without the box.
fn heading(text: &str) -> impl IntoView {
    let style = format!(
        "color:{};font-size:{};font-weight:590;letter-spacing:-0.01em;",
        tokens::TEXT_PRIMARY,
        tokens::FONT_SIZE_SM,
    );
    view! { <div style=style>{text.to_string()}</div> }
}

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
                let p0_count = p0.len();
                view! {
                    <Pane title=project.name.clone()>
                        <div style=format!(
                            "display:flex;gap:{};align-items:baseline;",
                            tokens::SPACING_LG,
                        )>
                            <div>
                                <div class="value">{open}</div>
                                <div class="label">"open"</div>
                            </div>
                            <div>
                                <div class="value text-muted">{closed}</div>
                                <div class="label">"closed"</div>
                            </div>
                            <Badge variant=BadgeVariant::Neutral>{head_badge}</Badge>
                        </div>
                        {(p0_count > 0).then(|| view! {
                            <div style=format!(
                                "margin-top:{};display:flex;flex-direction:column;gap:{};",
                                tokens::SPACING_SM,
                                tokens::SPACING_2XS,
                            )>
                                <Status variant=StatusVariant::Error color_text=true>
                                    {format!("{p0_count} P0 open")}
                                </Status>
                                {p0.into_iter()
                                    .take(3)
                                    .map(|t| { let tip = t.clone(); view! {
                                        <div
                                            class="trunc"
                                            title=tip
                                            style=format!(
                                                "color:{};font-size:{};padding-left:{};",
                                                tokens::TEXT_SECONDARY,
                                                tokens::FONT_SIZE_XS,
                                                tokens::SPACING_MD,
                                            )
                                        >{t}</div>
                                    }})
                                    .collect_view()}
                                {(p0_count > 3).then(|| view! {
                                    <div class="text-muted" style=format!(
                                        "font-size:{};padding-left:{};",
                                        tokens::FONT_SIZE_XS,
                                        tokens::SPACING_MD,
                                    )>{format!("+{} more", p0_count - 3)}</div>
                                })}
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
                let (kind, what) = entry
                    .unwrap_event()
                    .map(|ev| {
                        let what = ev
                            .item
                            .get("title")
                            .or_else(|| ev.item.get("body"))
                            .and_then(|v| v.as_str())
                            // One line only: the body can be a full paragraph.
                            .map(|s| s.lines().next().unwrap_or(s).to_string())
                            .unwrap_or_default();
                        (ev.item_type.to_string(), what)
                    })
                    .unwrap_or_else(|_| ("event".into(), "undecodable".into()));
                let time = entry.created.get(..19).unwrap_or("").replace('T', " ");
                let tip = what.clone();
                view! {
                    // Fixed columns: time | type | text — so the type chip never
                    // shifts where the text starts.
                    <div style=format!(
                        "display:grid;grid-template-columns:auto 7rem 1fr;gap:{};\
                         align-items:baseline;padding:{} 0;min-width:0;",
                        tokens::SPACING_SM,
                        tokens::SPACING_2XS,
                    )>
                        <span class="text-muted" style=format!(
                            "font-family:{};font-size:{};white-space:nowrap;",
                            tokens::FONT_MONO,
                            tokens::FONT_SIZE_2XS,
                        )>{time}</span>
                        <span style="justify-self:start;">
                            <Badge variant=BadgeVariant::Neutral>{kind}</Badge>
                        </span>
                        <span class="trunc" title=tip style=format!(
                            "font-size:{};",
                            tokens::FONT_SIZE_SM,
                        )>{what}</span>
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    view! {
        <div style=format!(
            "padding:{};height:100%;box-sizing:border-box;display:flex;flex-direction:column;\
             gap:{};min-height:0;",
            tokens::SPACING_MD,
            tokens::SPACING_MD,
        )>
            <div style=format!(
                "flex:0 0 auto;display:grid;\
                 grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:{};",
                tokens::SPACING_MD,
            )>
                {cards}
            </div>
            <div style=format!(
                "flex:1;min-height:0;display:flex;flex-direction:column;gap:{};",
                tokens::SPACING_SM,
            )>
                {heading("Activity")}
                <div style="flex:1;min-height:0;overflow-y:auto;">{feed}</div>
            </div>
        </div>
    }
}
