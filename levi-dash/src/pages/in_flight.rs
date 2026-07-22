use std::collections::BTreeMap;

use leptos::prelude::*;
use levi_core::*;
use myko_leptos::live_query;
use pulse_leptos_ui::{Badge, BadgeVariant, Disclosure, EmptyState, tokens};

use crate::resolve_client::claim_live;

#[component]
pub fn InFlight() -> impl IntoView {
    let claims = live_query(|| GetAllClaims {});
    let tasks = live_query(|| GetAllTasks {});

    let grouped = move || {
        let titles: std::collections::HashMap<String, String> = tasks
            .get()
            .iter()
            .map(|t| (t.id.to_string(), t.title.clone()))
            .collect();
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
            return view! { <EmptyState message="nothing in flight".to_string() /> }.into_any();
        }
        by_dev
            .into_iter()
            .map(|(dev, machines)| {
                let count: usize =
                    machines.values().flat_map(|w| w.values()).map(Vec::len).sum();
                let open = RwSignal::new(true);
                let dev_header = dev.clone();
                let titles = titles.clone();
                view! {
                    <Disclosure open=open header=ViewFn::from(move || view! {
                        <div style="display:flex;gap:8px;align-items:center;">
                            <span>{dev_header.clone()}</span>
                            <Badge variant=BadgeVariant::Primary>{count}</Badge>
                        </div>
                    }.into_any())>
                        {machines
                            .into_iter()
                            .map(|(machine, worktrees)| {
                                view! {
                                    <div style=format!(
                                        "margin-left:{};margin-bottom:{};",
                                        tokens::SPACING_MD, tokens::SPACING_SM,
                                    )>
                                        <div class="label">{machine}</div>
                                        {worktrees
                                            .into_iter()
                                            .map(|(worktree, claims)| {
                                                view! {
                                                    <div style=format!("margin-left:{};", tokens::SPACING_MD)>
                                                        {
                                                            let wt = worktree.clone();
                                                            view! {
                                                                <div class="text-muted trunc" title=worktree
                                                                     style=format!("font-size:{};", tokens::FONT_SIZE_2XS)>
                                                                    {wt}
                                                                </div>
                                                            }
                                                        }
                                                        {claims
                                                            .into_iter()
                                                            .map(|claim| {
                                                                let live = claim_live(&claim);
                                                                let title = titles
                                                                    .get(&*claim.task_id)
                                                                    .cloned()
                                                                    .unwrap_or_else(|| format!("lv-{}", &claim.task_id[..6.min(claim.task_id.len())]));
                                                                let time = claim.created.get(..19).unwrap_or("").replace('T', " ");
                                                                let tip = title.clone();
                                                                view! {
                                                                    <div class="levi-row" style=format!(
                                                                        "padding:{} 0;{}",
                                                                        tokens::SPACING_2XS,
                                                                        if live { "" } else { "opacity:.45;" }
                                                                    )>
                                                                        <span class="trunc" title=tip
                                                                              style="flex:1;">{title}</span>
                                                                        <span class="text-muted" style=format!(
                                                                            "font-family:{};font-size:{};white-space:nowrap;",
                                                                            tokens::FONT_MONO, tokens::FONT_SIZE_2XS,
                                                                        )>{time}</span>
                                                                        {(!live).then(|| view! {
                                                                            <Badge variant=BadgeVariant::Warning>"expired"</Badge>
                                                                        })}
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
                    </Disclosure>
                }
            })
            .collect_view()
            .into_any()
    };

    view! {
        <div style=format!("padding:{};height:100%;overflow-y:auto;", tokens::SPACING_MD)>
            {grouped}
        </div>
    }
}
