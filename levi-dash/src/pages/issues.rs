//! The cross-project blocking graph (spec 2026-07-21, Surface 2). Subscribes
//! to one hub-computed report (`IssueGraphReport`) and renders the returned
//! nodes/edges — it never pulls raw Tasks / Dependencys / StatusChanges, and
//! never a CommitFact. Layout is deterministic (longest-path layers from the
//! report), so the picture is stable across renders.

use std::collections::BTreeMap;

use leptos::prelude::*;
use levi_core::graph::{IssueGraph, IssueGraphOut, IssueGraphReport};
use levi_core::resolve::Status;
use myko_leptos::live_report;
use pulse_leptos_ui::{EmptyState, Pane, tokens};

/// Categorical identity palette (dataviz skill, validated). Assigned to
/// projects in sorted order, never cycled; a 9th project folds to gray.
const PALETTE: [&str; 8] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#6b6a66",
];
const OTHER: &str = "#8a8984";

const COL_W: f64 = 210.0;
const ROW_H: f64 = 66.0;
const NODE_W: f64 = 176.0;
const NODE_H: f64 = 46.0;
const PAD: f64 = 32.0;

#[component]
pub fn Issues() -> impl IntoView {
    let graph = live_report::<_, IssueGraphOut>(|| IssueGraphReport {});

    let body = move || {
        let Some(out) = graph.get() else {
            return view! { <EmptyState message="loading the graph…".to_string() /> }.into_any();
        };
        let g = out.graph;
        if g.edges.is_empty() {
            let n = g.unconnected.len();
            return view! {
                <EmptyState message=format!(
                    "nothing is blocked — {n} task(s) across all projects, no dependencies between them"
                ) />
            }
            .into_any();
        }
        let colors = project_colors(&g);
        let svg = render_svg(&g, &colors);
        let legend = colors
            .iter()
            .map(|(project, color)| {
                let short = project[..8.min(project.len())].to_string();
                view! {
                    <div style="display:flex;align-items:center;gap:6px;">
                        <span style=format!(
                            "width:12px;height:12px;border-radius:3px;background:{color};display:inline-block;"
                        )></span>
                        <span style=format!("color:{};font-size:12px;", tokens::TEXT_PRIMARY)>{short}</span>
                    </div>
                }
            })
            .collect_view();

        let unconnected = g.unconnected.len();
        let cycle_note = (!g.broken_cycles.is_empty()).then(|| {
            let n = g.broken_cycles.len();
            view! {
                <div style=format!("color:{};font-size:12px;margin-top:8px;", tokens::ERROR)>
                    {format!("⚠ {n} dependency cycle(s) broken for layout")}
                </div>
            }
        });

        view! {
            <div>
                <div style="display:flex;gap:16px;flex-wrap:wrap;margin-bottom:12px;">{legend}</div>
                <div style="overflow:auto;" inner_html=svg></div>
                <div style=format!("color:{};font-size:12px;margin-top:8px;", tokens::TEXT_TERTIARY)>
                    {format!(
                        "{} task(s) in the graph · {unconnected} not blocked · dashed edge = blocker already closed",
                        g.nodes.len(),
                    )}
                </div>
                {cycle_note}
            </div>
        }
        .into_any()
    };

    view! { <Pane title="Blocking graph".to_string()>{body}</Pane> }
}

/// Sorted project -> categorical color (fixed order, never cycled).
fn project_colors(g: &IssueGraph) -> Vec<(String, String)> {
    let mut projects: Vec<String> = g.nodes.iter().map(|n| n.project_id.clone()).collect();
    projects.sort();
    projects.dedup();
    projects
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let c = PALETTE.get(i).copied().unwrap_or(OTHER);
            (p, c.to_string())
        })
        .collect()
}

/// Positions: x by layer, y by index within layer.
struct Pos {
    x: f64,
    y: f64,
}

fn render_svg(g: &IssueGraph, colors: &[(String, String)]) -> String {
    let color_of = |pid: &str| -> &str {
        colors
            .iter()
            .find(|(p, _)| p == pid)
            .map(|(_, c)| c.as_str())
            .unwrap_or(OTHER)
    };

    // Group nodes by layer (already layer-sorted) to assign row indices.
    let mut per_layer: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pos: BTreeMap<String, Pos> = BTreeMap::new();
    let mut max_rows = 0usize;
    let mut max_layer = 0usize;
    for n in &g.nodes {
        let row = per_layer.entry(n.layer).or_insert(0);
        pos.insert(
            n.task_id.clone(),
            Pos {
                x: PAD + n.layer as f64 * COL_W,
                y: PAD + *row as f64 * ROW_H,
            },
        );
        *row += 1;
        max_rows = max_rows.max(*row);
        max_layer = max_layer.max(n.layer);
    }
    let width = PAD * 2.0 + (max_layer as f64 + 1.0) * COL_W;
    let height = PAD * 2.0 + max_rows as f64 * ROW_H;

    let mut s = String::new();
    s.push_str(&format!(
        r#"<svg viewBox="0 0 {width:.0} {height:.0}" width="{width:.0}" height="{height:.0}" xmlns="http://www.w3.org/2000/svg" font-family="ui-sans-serif,system-ui,sans-serif">"#,
    ));
    s.push_str(
        r##"<defs><marker id="arw" markerWidth="9" markerHeight="9" refX="7" refY="3" orient="auto"><path d="M0,0 L6,3 L0,6 z" fill="#8a8984"/></marker></defs>"##,
    );

    // Edges first (under nodes). Blocker right-center -> blocked left-center.
    for e in &g.edges {
        let (Some(from), Some(to)) = (pos.get(&e.blocker_task_id), pos.get(&e.blocked_task_id))
        else {
            continue;
        };
        let x1 = from.x + NODE_W;
        let y1 = from.y + NODE_H / 2.0;
        let x2 = to.x;
        let y2 = to.y + NODE_H / 2.0;
        let mid = (x1 + x2) / 2.0;
        let dash = if e.resolved {
            r#" stroke-dasharray="5 4""#
        } else {
            ""
        };
        let stroke = if e.resolved { "#b8b7b2" } else { "#eb6834" };
        let via = e.via.as_deref().unwrap_or("");
        let title = if e.blocker_project_id.is_some() && !via.is_empty() {
            format!("<title>via: {}</title>", escape(via))
        } else {
            String::new()
        };
        s.push_str(&format!(
            r#"<path d="M{x1:.0} {y1:.0} C {mid:.0} {y1:.0}, {mid:.0} {y2:.0}, {x2:.0} {y2:.0}" fill="none" stroke="{stroke}" stroke-width="2"{dash} marker-end="url(#arw)">{title}</path>"#,
        ));
    }

    // Nodes.
    for n in &g.nodes {
        let p = &pos[&n.task_id];
        let color = color_of(&n.project_id);
        let short = format!("lv-{}", &n.task_id[..n.task_id.len().min(6)]);
        let (fill_op, text_dim) = if n.status == Status::Closed {
            (0.08, "opacity=\"0.6\"")
        } else {
            (0.16, "")
        };
        let stub_stroke = if n.stub {
            r#" stroke-dasharray="4 3""#
        } else {
            ""
        };
        let status_label = match n.status {
            Status::Open => "open",
            Status::Closed => "closed",
        };
        s.push_str(&format!(
            r#"<g {text_dim}><rect x="{x:.0}" y="{y:.0}" width="{w:.0}" height="{h:.0}" rx="6" fill="{color}" fill-opacity="{fill_op}" stroke="{color}" stroke-width="1.5"{stub_stroke}/>"#,
            x = p.x, y = p.y, w = NODE_W, h = NODE_H,
        ));
        s.push_str(&format!(
            r#"<text x="{tx:.0}" y="{ty1:.0}" font-size="11" font-weight="600" fill="{color}">{short}</text>"#,
            tx = p.x + 10.0, ty1 = p.y + 17.0,
        ));
        s.push_str(&format!(
            r#"<text x="{tx:.0}" y="{ty2:.0}" font-size="11" fill="{ink}">{title}</text>"#,
            tx = p.x + 10.0,
            ty2 = p.y + 34.0,
            ink = tokens::TEXT_PRIMARY,
            title = escape(&truncate(&n.title, 24)),
        ));
        s.push_str(&format!(
            r#"<title>{}  ·  {}  ·  {status_label}</title></g>"#,
            escape(&n.project_id[..8.min(n.project_id.len())]),
            escape(&n.title),
        ));
    }

    s.push_str("</svg>");
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
