//! The cross-project blocking graph (spec 2026-07-21, Surface 2). Subscribes
//! to one hub-computed report (`IssueGraphReport`) and renders the returned
//! nodes/edges — it never pulls raw Tasks / Dependencys / StatusChanges, and
//! never a CommitFact. Layout is deterministic (longest-path layers from the
//! report), so the picture is stable across renders.
//!
//! Nodes are HTML cards styled with the design system's tokens (so text is
//! the app font and scale); only the edges are SVG. Project identity is an
//! accent color on the card border/tint — text stays in ink tokens.

use std::collections::BTreeMap;

use leptos::prelude::*;
use levi_core::graph::{IssueGraph, IssueGraphOut, IssueGraphReport};
use levi_core::resolve::Status;
use myko_leptos::live_report;
use pulse_leptos_ui::{Badge, BadgeVariant, EmptyState, tokens};

/// Categorical identity palette (dataviz skill, validated). Assigned to
/// projects in sorted order, never cycled; a 9th project folds to gray. Used
/// only as an accent mark on cards/edges — never for text.
const PALETTE: [&str; 8] = [
    "#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#4f9e2f", "#9085e9", "#7c7a86",
];
const OTHER: &str = "#7c7a86";

const COL_W: f64 = 224.0;
const ROW_H: f64 = 72.0;
const NODE_W: f64 = 184.0;
const NODE_H: f64 = 52.0;
const PAD: f64 = 8.0;

#[component]
pub fn Issues() -> impl IntoView {
    let graph = live_report::<_, IssueGraphOut>(|| IssueGraphReport {});

    let body = move || {
        let Some(out) = graph.get() else {
            return view! { <EmptyState message="Loading the graph…".to_string() /> }.into_any();
        };
        let g = out.graph;
        if g.edges.is_empty() {
            let n = g.unconnected.len();
            return view! {
                <EmptyState message=format!(
                    "Nothing is blocked — {n} tasks across all projects, no dependencies between them."
                ) />
            }
            .into_any();
        }
        let colors = project_colors(&g);
        view! {
            <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_MD)>
                <Legend colors=colors.clone() />
                <div style="overflow:auto;">
                    <GraphCanvas graph=g.clone() colors=colors.clone() />
                </div>
                <Summary graph=g />
            </div>
        }
        .into_any()
    };

    // No inner pane: the workspace pane already frames and titles this view.
    view! {
        <div style=format!("padding:{};height:100%;box-sizing:border-box;overflow:auto;", tokens::SPACING_MD)>
            {body}
        </div>
    }
}

#[component]
fn Legend(colors: Vec<(String, String)>) -> impl IntoView {
    view! {
        <div style=format!(
            "display:flex;gap:{};flex-wrap:wrap;align-items:center;",
            tokens::SPACING_LG,
        )>
            {colors
                .into_iter()
                .map(|(project, color)| {
                    let label = project[..8.min(project.len())].to_string();
                    view! {
                        <div style=format!("display:flex;align-items:center;gap:{};", tokens::SPACING_XS)>
                            <span style=format!(
                                "width:0.625rem;height:0.625rem;border-radius:{};background:{color};",
                                tokens::RADIUS_SM,
                            )></span>
                            <span style=format!(
                                "color:{};font-size:{};",
                                tokens::TEXT_SECONDARY,
                                tokens::FONT_SIZE_XS,
                            )>{label}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn Summary(graph: IssueGraph) -> impl IntoView {
    let cycles = graph.broken_cycles.len();
    view! {
        <div style=format!(
            "display:flex;flex-direction:column;gap:{};color:{};font-size:{};",
            tokens::SPACING_2XS,
            tokens::TEXT_TERTIARY,
            tokens::FONT_SIZE_XS,
        )>
            <span>
                {format!(
                    "{} tasks in the graph · {} not blocked · dashed edge = blocker already closed",
                    graph.nodes.len(),
                    graph.unconnected.len(),
                )}
            </span>
            {(cycles > 0)
                .then(|| {
                    view! {
                        <span style=format!("color:{};", tokens::WARNING)>
                            {format!("{cycles} dependency cycle(s) broken for layout")}
                        </span>
                    }
                })}
        </div>
    }
}

#[component]
fn GraphCanvas(graph: IssueGraph, colors: Vec<(String, String)>) -> impl IntoView {
    let color_of = {
        let colors = colors.clone();
        move |pid: &str| -> String {
            colors
                .iter()
                .find(|(p, _)| p == pid)
                .map(|(_, c)| c.clone())
                .unwrap_or_else(|| OTHER.to_string())
        }
    };

    // Positions: x by layer, y by index within layer.
    let mut per_layer: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pos: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let mut max_rows = 0usize;
    let mut max_layer = 0usize;
    for n in &graph.nodes {
        let row = per_layer.entry(n.layer).or_insert(0);
        pos.insert(
            n.task_id.clone(),
            (PAD + n.layer as f64 * COL_W, PAD + *row as f64 * ROW_H),
        );
        *row += 1;
        max_rows = max_rows.max(*row);
        max_layer = max_layer.max(n.layer);
    }
    let width = PAD * 2.0 + (max_layer as f64 + 1.0) * COL_W - (COL_W - NODE_W);
    let height = PAD * 2.0 + max_rows as f64 * ROW_H;

    // Edges (SVG, under the cards).
    let mut edge_svg = String::new();
    for e in &graph.edges {
        let (Some(&(fx, fy)), Some(&(tx, ty))) =
            (pos.get(&e.blocker_task_id), pos.get(&e.blocked_task_id))
        else {
            continue;
        };
        let x1 = fx + NODE_W;
        let y1 = fy + NODE_H / 2.0;
        let x2 = tx;
        let y2 = ty + NODE_H / 2.0;
        let mid = (x1 + x2) / 2.0;
        let dash = if e.resolved { r#" stroke-dasharray="5 4""# } else { "" };
        let stroke = if e.resolved { "#4a4a48" } else { "#6b6a66" };
        edge_svg.push_str(&format!(
            r#"<path d="M{x1:.0} {y1:.0} C {mid:.0} {y1:.0}, {mid:.0} {y2:.0}, {x2:.0} {y2:.0}" fill="none" stroke="{stroke}" stroke-width="1.5"{dash} marker-end="url(#arw)"/>"#,
        ));
    }
    let svg = format!(
        r##"<svg width="{width:.0}" height="{height:.0}" style="position:absolute;top:0;left:0;pointer-events:none;"><defs><marker id="arw" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto"><path d="M0,0 L6,3 L0,6 z" fill="#6b6a66"/></marker></defs>{edge_svg}</svg>"##,
    );

    // Node cards (HTML — inherit the app font + scale).
    let cards = graph
        .nodes
        .into_iter()
        .map(|n| {
            let (x, y) = pos[&n.task_id];
            let color = color_of(&n.project_id);
            let short = format!("lv-{}", &n.task_id[..n.task_id.len().min(6)]);
            let closed = n.status == Status::Closed;
            let card_style = format!(
                "position:absolute;left:{x:.0}px;top:{y:.0}px;width:{w:.0}px;height:{h:.0}px;\
                 box-sizing:border-box;display:flex;flex-direction:column;justify-content:center;\
                 gap:{gap};padding:0 {px};border-radius:{radius};background:{bg};\
                 border:1px solid {color};{border_style}{dim}",
                w = NODE_W,
                h = NODE_H,
                gap = tokens::SPACING_2XS,
                px = tokens::SPACING_SM,
                radius = tokens::RADIUS_MD,
                bg = tokens::BASE_200,
                border_style = if n.stub { "border-style:dashed;" } else { "" },
                dim = if closed { "opacity:0.6;" } else { "" },
            );
            let variant = priority_variant(n.priority);
            view! {
                <div style=card_style title=n.title.clone()>
                    <div style=format!("display:flex;align-items:center;gap:{};", tokens::SPACING_XS)>
                        <Badge variant=variant>{n.priority.label().to_string()}</Badge>
                        <span style=format!(
                            "color:{color};font-size:{};font-family:{};",
                            tokens::FONT_SIZE_2XS,
                            tokens::FONT_MONO,
                        )>{short}</span>
                    </div>
                    <span style=format!(
                        "color:{};font-size:{};white-space:nowrap;overflow:hidden;text-overflow:ellipsis;",
                        tokens::TEXT_PRIMARY,
                        tokens::FONT_SIZE_XS,
                    )>{n.title.clone()}</span>
                </div>
            }
        })
        .collect_view();

    view! {
        <div style=format!("position:relative;width:{width:.0}px;height:{height:.0}px;")
             inner_html=svg></div>
        <div style=format!(
            "position:relative;width:{width:.0}px;height:{height:.0}px;margin-top:-{height:.0}px;"
        )>{cards}</div>
    }
}

/// Sorted project -> categorical accent color (fixed order, never cycled).
fn project_colors(g: &IssueGraph) -> Vec<(String, String)> {
    let mut projects: Vec<String> = g.nodes.iter().map(|n| n.project_id.clone()).collect();
    projects.sort();
    projects.dedup();
    projects
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p, PALETTE.get(i).copied().unwrap_or(OTHER).to_string()))
        .collect()
}

fn priority_variant(p: levi_core::Priority) -> BadgeVariant {
    match p {
        levi_core::Priority::P0 => BadgeVariant::Error,
        levi_core::Priority::P1 => BadgeVariant::Warning,
        levi_core::Priority::P2 => BadgeVariant::Neutral,
        levi_core::Priority::P3 => BadgeVariant::Neutral,
    }
}
