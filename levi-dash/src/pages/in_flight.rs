use std::collections::BTreeMap;

use leptos::prelude::*;
use levi_core::*;
use myko_leptos::live_query;

use crate::resolve_client::claim_live;

#[component]
pub fn InFlight() -> impl IntoView {
    let claims = live_query(|| GetAllClaims {});
    let tasks = live_query(|| GetAllTasks {});

    let grouped = move || {
        let tasks = tasks.get();
        let title = |task_id: &str| {
            tasks
                .iter()
                .find(|t| &*t.id.0 == task_id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| task_id[..8.min(task_id.len())].to_string())
        };
        // dev -> machine -> worktree -> claims
        let mut by_dev: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<_>>>> =
            BTreeMap::new();
        for claim in claims.get() {
            by_dev
                .entry(claim.dev.clone())
                .or_default()
                .entry(claim.machine.clone())
                .or_default()
                .entry(claim.worktree.clone())
                .or_default()
                .push(claim);
        }
        if by_dev.is_empty() {
            return view! { <div class="muted">"nothing in flight"</div> }.into_any();
        }
        by_dev
            .into_iter()
            .map(|(dev, machines)| {
                view! {
                    <div class="group">
                        <div class="head">{dev}</div>
                        {machines
                            .into_iter()
                            .map(|(machine, worktrees)| {
                                view! {
                                    <div class="indent">
                                        <div class="muted">{machine}</div>
                                        {worktrees
                                            .into_iter()
                                            .map(|(worktree, claims)| {
                                                view! {
                                                    <div class="indent">
                                                        <div class="badge">{worktree}</div>
                                                        {claims
                                                            .into_iter()
                                                            .map(|claim| {
                                                                let live = claim_live(&claim);
                                                                view! {
                                                                    <div class="row" class:stale=!live>
                                                                        <span>{title(&claim.task_id)}</span>
                                                                        <span class="muted">{claim.at.get(..19).unwrap_or("").to_string()}</span>
                                                                        {(!live).then(|| view! { <span class="badge">"expired"</span> })}
                                                                    </div>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </div>
                                                }
                                            })
                                            .collect_view()}
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    view! { <div>{grouped}</div> }
}
