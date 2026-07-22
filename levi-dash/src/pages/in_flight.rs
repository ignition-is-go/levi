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
                let all: Vec<_> = machines.values().flat_map(|w| w.values()).flatten().collect();
                let live = all.iter().filter(|c| claim_live(c)).count();
                let expired = all.len() - live;
                let open = RwSignal::new(live > 0);
                let dev_header = dev.clone();
                let titles = titles.clone();
                view! {
                    <Disclosure open=open header=ViewFn::from(move || view! {
                        <div class="levi-row" style=format!("gap:{};", tokens::SPACING_SM)>
                            <span style=format!("font-size:{};", tokens::FONT_SIZE_SM)>
                                {dev_header.clone()}
                            </span>
                            {(live > 0).then(|| view! {
                                <Badge variant=BadgeVariant::Success>{format!("{live} live")}</Badge>
                            })}
                            {(expired > 0).then(|| view! {
                                <span class="text-muted" style=format!("font-size:{};", tokens::FONT_SIZE_XS)>
                                    {format!("{expired} expired")}
                                </span>
                            })}
                        </div>
                    }.into_any())>
                        {machines
                            .into_iter()
                            .map(|(machine, worktrees)| machine_block(machine, worktrees, titles.clone()))
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

type Claims = Vec<std::sync::Arc<Claim>>;

fn machine_block(
    machine: String,
    worktrees: BTreeMap<String, Claims>,
    titles: std::collections::HashMap<String, String>,
) -> impl IntoView {
    view! {
        <div style=format!("margin:{} 0 {} {};", tokens::SPACING_SM, tokens::SPACING_SM, tokens::SPACING_SM)>
            <div class="label">{machine}</div>
            {worktrees
                .into_iter()
                .map(|(worktree, claims)| worktree_block(worktree, claims, titles.clone()))
                .collect_view()}
        </div>
    }
}

fn worktree_block(
    worktree: String,
    mut claims: Claims,
    titles: std::collections::HashMap<String, String>,
) -> impl IntoView {
    // Live claims first, then expired; newest within each.
    claims.sort_by(|a, b| {
        (!claim_live(a), std::cmp::Reverse(a.created.clone()))
            .cmp(&(!claim_live(b), std::cmp::Reverse(b.created.clone())))
    });
    let wt = worktree.clone();
    view! {
        // A subtle rail shows the worktree owns these claims.
        <div style=format!(
            "margin-left:{};padding-left:{};border-left:1px solid {};",
            tokens::SPACING_XS, tokens::SPACING_MD, tokens::BORDER,
        )>
            <div class="text-muted trunc" title=worktree style=format!(
                "font-family:{};font-size:{};margin-bottom:{};",
                tokens::FONT_MONO, tokens::FONT_SIZE_XS, tokens::SPACING_2XS,
            )>{wt}</div>
            {claims
                .into_iter()
                .map(move |claim| {
                    let title = titles.get(&*claim.task_id).cloned();
                    claim_row(claim, title)
                })
                .collect_view()}
        </div>
    }
}

fn claim_row(claim: std::sync::Arc<Claim>, title: Option<String>) -> impl IntoView {
    let live = claim_live(&claim);
    let short = format!("lv-{}", &claim.task_id[..6.min(claim.task_id.len())]);
    let title = title.unwrap_or_else(|| short.clone());
    let tip = title.clone();
    let time = claim.created.get(..19).unwrap_or("").replace('T', " ");
    // Live = a filled success dot; expired = an outline dot (keeps the column
    // aligned and reads as inactive rather than alarming).
    let dot = if live {
        format!("background:{};", tokens::SUCCESS)
    } else {
        format!("border:1px solid {};", tokens::TEXT_QUATERNARY)
    };
    view! {
        <div class="levi-row" style=format!(
            "padding:{} 0;{}",
            tokens::SPACING_2XS,
            if live { "" } else { "opacity:.5;" },
        )>
            <span style=format!(
                "width:0.5rem;height:0.5rem;border-radius:624rem;flex:0 0 auto;{dot}"
            )></span>
            <span class="text-muted" style=format!(
                "font-family:{};font-size:{};white-space:nowrap;",
                tokens::FONT_MONO, tokens::FONT_SIZE_XS,
            )>{short}</span>
            <span class="trunc" title=tip style=format!(
                "flex:1;font-size:{};", tokens::FONT_SIZE_SM,
            )>{title}</span>
            <span class="text-muted" style=format!(
                "font-family:{};font-size:{};white-space:nowrap;",
                tokens::FONT_MONO, tokens::FONT_SIZE_XS,
            )>{time}</span>
            {(!live).then(|| view! {
                <Badge variant=BadgeVariant::Neutral>"expired"</Badge>
            })}
        </div>
    }
}
