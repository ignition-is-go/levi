//! The cross-project blocking graph (spec 2026-07-21, Surface 2). Subscribes
//! to one hub-computed report (`IssueGraphReport`) and renders the returned
//! nodes/edges — it never pulls raw Tasks / Dependencys / StatusChanges, and
//! never a CommitFact. Layout is deterministic (longest-path layers from the
//! report), so the picture is stable across renders.
//!
//! Instead of one global canvas (where independent dependency webs interleave
//! into the same longest-path columns and their edges cross everything), the
//! graph is split into its weakly-connected components — one *section* per
//! independent blocking web, stacked vertically and titled by its base
//! blockers. Within a section, rows are ordered by a barycenter sweep so a
//! chain reads top-to-bottom with minimal crossings.
//!
//! Nodes are HTML cards styled with the design system's tokens (so text is
//! the app font and scale); only the edges are SVG. Project identity is an
//! accent color on the card border/tint — text stays in ink tokens.

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use levi_core::GetAllProjects;
use levi_core::graph::{GraphNode, IssueGraph, IssueGraphOut, IssueGraphReport};
use levi_core::resolve::Status;
use myko_leptos::{live_query, live_report};
use pulse_leptos_ui::{Badge, BadgeVariant, EmptyState, Toggle, tokens};

/// project id -> human name, for the legend/labels. The report ships only the
/// project *uuid* on each node; the tiny Projects list (already synced to the
/// hub, one row per project) gives us the name without touching raw tasks.
type Names = BTreeMap<String, String>;

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
    let projects = live_query(|| GetAllProjects {});
    // Closed tasks are hidden by default so the graph shows open work still
    // blocking; the toggle brings them back (dimmed, with dashed resolved edges).
    let (show_closed, set_show_closed) = signal(false);

    let body = move || {
        let Some(out) = graph.get() else {
            return view! { <EmptyState message="Loading the graph…".to_string() /> }.into_any();
        };
        let names: Names = projects
            .get()
            .into_iter()
            .map(|p| (p.id.to_string(), p.name.clone()))
            .collect();
        let mut g = out.graph;
        if !show_closed.get() {
            g.nodes.retain(|n| n.status != Status::Closed);
            let keep: BTreeSet<String> = g.nodes.iter().map(|n| n.task_id.clone()).collect();
            g.edges
                .retain(|e| keep.contains(&e.blocker_task_id) && keep.contains(&e.blocked_task_id));
        }
        let colors = project_colors(&g);
        // Legend + toggle stay visible even when the filtered graph is empty, so
        // hiding closed can always be undone.
        let content = if g.edges.is_empty() {
            let n = g.unconnected.len();
            view! {
                <EmptyState message=format!(
                    "Nothing is blocked — {n} tasks across all projects, no dependencies between them."
                ) />
            }
            .into_any()
        } else {
            view! {
                <GraphCanvas graph=g.clone() colors=colors.clone() names=names.clone() />
                <Summary graph=g />
            }
            .into_any()
        };
        view! {
            <div style=format!("display:flex;flex-direction:column;gap:{};", tokens::SPACING_MD)>
                <div style=format!(
                    "display:flex;align-items:center;gap:{};flex-wrap:wrap;", tokens::SPACING_MD,
                )>
                    <Legend colors=colors names=names />
                    <div style="margin-left:auto;">
                        <Toggle
                            checked=show_closed
                            on_change=Callback::new(move |v| set_show_closed.set(v))
                            label="Show closed".to_string()
                        />
                    </div>
                </div>
                {content}
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
fn Legend(colors: Vec<(String, String)>, names: Names) -> impl IntoView {
    view! {
        <div style=format!(
            "display:flex;gap:{};flex-wrap:wrap;align-items:center;",
            tokens::SPACING_LG,
        )>
            {colors
                .into_iter()
                .map(|(project, color)| {
                    let label = project_label(&project, &names);
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
fn GraphCanvas(
    graph: IssueGraph,
    colors: Vec<(String, String)>,
    names: Names,
) -> impl IntoView {
    // Index the report's nodes, then split into weakly-connected components and
    // lay each one out on its own (rebased columns + barycenter row order).
    let node_by_id: BTreeMap<String, GraphNode> =
        graph.nodes.iter().map(|n| (n.task_id.clone(), n.clone())).collect();
    let layer_of: BTreeMap<String, usize> =
        graph.nodes.iter().map(|n| (n.task_id.clone(), n.layer)).collect();
    let prio_of: BTreeMap<String, u8> =
        graph.nodes.iter().map(|n| (n.task_id.clone(), n.priority.rank())).collect();
    let node_ids: Vec<String> = graph.nodes.iter().map(|n| n.task_id.clone()).collect();
    let edges: Vec<(String, String)> = graph
        .edges
        .iter()
        .map(|e| (e.blocker_task_id.clone(), e.blocked_task_id.clone()))
        .collect();

    let sections = build_sections(&node_ids, &edges, &layer_of, &prio_of);

    let section_views = sections
        .into_iter()
        .map(|sec| {
            let ids: BTreeSet<&String> = sec.node_ids.iter().collect();

            // Edges internal to this section, positioned in section-local space.
            let mut edge_svg = String::new();
            for e in &graph.edges {
                if !(ids.contains(&e.blocker_task_id) && ids.contains(&e.blocked_task_id)) {
                    continue;
                }
                let (Some(&(fx, fy)), Some(&(tx, ty))) =
                    (sec.pos.get(&e.blocker_task_id), sec.pos.get(&e.blocked_task_id))
                else {
                    continue;
                };
                let x1 = PAD + fx + NODE_W;
                let y1 = PAD + fy + NODE_H / 2.0;
                let x2 = PAD + tx;
                let y2 = PAD + ty + NODE_H / 2.0;
                let mid = (x1 + x2) / 2.0;
                let dash = if e.resolved { r#" stroke-dasharray="5 4""# } else { "" };
                let stroke = if e.resolved { "#4a4a48" } else { "#6b6a66" };
                edge_svg.push_str(&format!(
                    r#"<path d="M{x1:.0} {y1:.0} C {mid:.0} {y1:.0}, {mid:.0} {y2:.0}, {x2:.0} {y2:.0}" fill="none" stroke="{stroke}" stroke-width="1.5"{dash} marker-end="url(#arw)"/>"#,
                ));
            }
            let w = sec.width + PAD * 2.0;
            let h = sec.height + PAD * 2.0;
            let svg = format!(
                r##"<svg width="{w:.0}" height="{h:.0}" style="position:absolute;top:0;left:0;pointer-events:none;"><defs><marker id="arw" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto"><path d="M0,0 L6,3 L0,6 z" fill="#6b6a66"/></marker></defs>{edge_svg}</svg>"##,
            );

            let cards = sec
                .node_ids
                .iter()
                .filter_map(|id| node_by_id.get(id).cloned())
                .map(|n| {
                    let (x, y) = sec.pos[&n.task_id];
                    let color = color_for(&colors, &n.project_id);
                    node_card(n, PAD + x, PAD + y, color)
                })
                .collect_view();

            // Header data (owned, so the view captures no borrows of locals).
            const SHOWN: usize = 3;
            let extra = sec.root_ids.len().saturating_sub(SHOWN);
            let roots: Vec<(String, String)> = sec
                .root_ids
                .iter()
                .take(SHOWN)
                .filter_map(|id| node_by_id.get(id))
                .map(|n| (short_id(&n.task_id), color_for(&colors, &n.project_id)))
                .collect();
            let mut projs: Vec<String> = sec
                .node_ids
                .iter()
                .filter_map(|id| node_by_id.get(id))
                .map(|n| project_label(&n.project_id, &names))
                .collect();
            projs.sort();
            projs.dedup();
            let header = section_header(sec.node_ids.len(), roots, extra, projs.join(", "));

            view! {
                <div style=format!(
                    "display:flex;flex-direction:column;gap:{};", tokens::SPACING_XS,
                )>
                    {header}
                    <div style="overflow-x:auto;">
                        <div style=format!("position:relative;width:{w:.0}px;height:{h:.0}px;")
                             inner_html=svg></div>
                        <div style=format!(
                            "position:relative;width:{w:.0}px;height:{h:.0}px;margin-top:-{h:.0}px;"
                        )>{cards}</div>
                    </div>
                </div>
            }
        })
        .collect_view();

    view! {
        <div style=format!(
            "display:flex;flex-direction:column;gap:{};", tokens::SPACING_XL,
        )>{section_views}</div>
    }
}

/// One section header: the base blockers (col-0 roots) that gate the whole
/// web, plus a task count. Reads as "close these to unblock N tasks". Takes
/// owned data so the returned view captures no borrows of `GraphCanvas` locals.
fn section_header(
    count: usize,
    roots: Vec<(String, String)>,
    extra: usize,
    projs_label: String,
) -> impl IntoView {
    view! {
        <div style=format!(
            "display:flex;align-items:baseline;gap:{};flex-wrap:wrap;\
             padding-bottom:{};border-bottom:1px solid {};",
            tokens::SPACING_SM,
            tokens::SPACING_2XS,
            tokens::BORDER,
        )>
            <span style=format!(
                "color:{};font-size:{};font-weight:600;", tokens::TEXT_PRIMARY, tokens::FONT_SIZE_SM,
            )>{format!("{count} task{}", if count == 1 { "" } else { "s" })}</span>
            <span style=format!(
                "color:{};font-size:{};", tokens::TEXT_TERTIARY, tokens::FONT_SIZE_XS,
            )>"·  unblocked by"</span>
            {roots
                .into_iter()
                .map(|(short, color)| {
                    view! {
                        <span style=format!(
                            "color:{color};font-size:{};font-family:{};",
                            tokens::FONT_SIZE_XS,
                            tokens::FONT_MONO,
                        )>{short}</span>
                    }
                })
                .collect_view()}
            {(extra > 0)
                .then(|| {
                    view! {
                        <span style=format!(
                            "color:{};font-size:{};", tokens::TEXT_TERTIARY, tokens::FONT_SIZE_XS,
                        )>{format!("+{extra} more")}</span>
                    }
                })}
            <span style=format!(
                "margin-left:auto;color:{};font-size:{};", tokens::TEXT_TERTIARY, tokens::FONT_SIZE_XS,
            )>{projs_label}</span>
        </div>
    }
}

/// One node card (HTML — inherits the app font + scale). Extracted so every
/// section renders identical cards. `color` is passed owned so the view holds
/// no borrow of the caller's palette.
fn node_card(n: GraphNode, x: f64, y: f64, color: String) -> impl IntoView {
    let short = short_id(&n.task_id);
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
                    tokens::FONT_SIZE_XS,
                    tokens::FONT_MONO,
                )>{short}</span>
            </div>
            <span style=format!(
                "color:{};font-size:{};white-space:nowrap;overflow:hidden;text-overflow:ellipsis;",
                tokens::TEXT_PRIMARY,
                tokens::FONT_SIZE_SM,
            )>{n.title.clone()}</span>
        </div>
    }
}

fn short_id(task_id: &str) -> String {
    format!("lv-{}", &task_id[..task_id.len().min(6)])
}

fn color_for(colors: &[(String, String)], pid: &str) -> String {
    colors
        .iter()
        .find(|(p, _)| p == pid)
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| OTHER.to_string())
}

/// Human project name when known, else the short uuid (foreign/unsynced).
fn project_label(pid: &str, names: &Names) -> String {
    names
        .get(pid)
        .cloned()
        .unwrap_or_else(|| pid[..8.min(pid.len())].to_string())
}

// ---- layout: weakly-connected components + barycenter row ordering ----

/// A laid-out cluster: one weakly-connected component of the blocking graph.
struct Section {
    /// Node ids in this component.
    node_ids: Vec<String>,
    /// Section-local top-left of each card (origin at 0,0, before PAD).
    pos: BTreeMap<String, (f64, f64)>,
    width: f64,
    height: f64,
    /// Column-0 nodes: blocked by nothing in the graph — the base blockers.
    root_ids: Vec<String>,
    /// Best (lowest) priority rank present, for section ordering.
    best_rank: u8,
}

/// Union-find over node ids to find weakly-connected components.
fn components(node_ids: &[String], edges: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut parent: BTreeMap<String, String> = node_ids.iter().map(|i| (i.clone(), i.clone())).collect();
    fn find(parent: &mut BTreeMap<String, String>, x: &str) -> String {
        let p = parent[x].clone();
        if p == x {
            return p;
        }
        let r = find(parent, &p);
        parent.insert(x.to_string(), r.clone());
        r
    }
    for (a, b) in edges {
        if !parent.contains_key(a) || !parent.contains_key(b) {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in node_ids {
        let r = find(&mut parent, id);
        groups.entry(r).or_default().push(id.clone());
    }
    groups
}

/// Lay out every component, biggest / most-important first.
fn build_sections(
    node_ids: &[String],
    edges: &[(String, String)],
    layer_of: &BTreeMap<String, usize>,
    prio_of: &BTreeMap<String, u8>,
) -> Vec<Section> {
    let mut sections: Vec<Section> = components(node_ids, edges)
        .into_values()
        .map(|ids| layout_component(ids, edges, layer_of, prio_of))
        .collect();
    sections.sort_by(|a, b| {
        b.node_ids
            .len()
            .cmp(&a.node_ids.len())
            .then(a.best_rank.cmp(&b.best_rank))
            .then(a.node_ids.first().cmp(&b.node_ids.first()))
    });
    sections
}

fn layout_component(
    ids: Vec<String>,
    edges: &[(String, String)],
    layer_of: &BTreeMap<String, usize>,
    prio_of: &BTreeMap<String, u8>,
) -> Section {
    let idset: BTreeSet<&String> = ids.iter().collect();
    let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (a, b) in edges {
        if idset.contains(a) && idset.contains(b) {
            parents.entry(b.clone()).or_default().push(a.clone());
            children.entry(a.clone()).or_default().push(b.clone());
        }
    }
    // Dense-rank the layers actually present in this component into contiguous
    // columns (0,1,2,…). Rebasing alone would leave gaps when intermediate
    // layers were filtered out (e.g. closed nodes hidden), wasting a column and
    // stretching edges across empty space; dense ranks preserve order without
    // the holes.
    let mut present: Vec<usize> = ids.iter().map(|id| layer_of[id]).collect();
    present.sort_unstable();
    present.dedup();
    let col_of: BTreeMap<usize, usize> =
        present.iter().enumerate().map(|(i, &l)| (l, i)).collect();

    // Columns keyed by dense-ranked layer; seed each with a deterministic order.
    let mut cols: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for id in &ids {
        cols.entry(col_of[&layer_of[id]]).or_default().push(id.clone());
    }
    for v in cols.values_mut() {
        v.sort_by(|a, b| (prio_of[a], a).cmp(&(prio_of[b], b)));
    }

    // Barycenter sweeps: order each column by the mean row of its neighbours in
    // the already-placed direction (parents forward, children backward).
    let colkeys: Vec<usize> = cols.keys().copied().collect();
    for sweep in 0..6 {
        let forward = sweep % 2 == 0;
        let order: Vec<usize> = if forward {
            colkeys.clone()
        } else {
            colkeys.iter().rev().copied().collect()
        };
        let mut row: BTreeMap<String, f64> = BTreeMap::new();
        for v in cols.values() {
            for (i, id) in v.iter().enumerate() {
                row.insert(id.clone(), i as f64);
            }
        }
        let neigh = if forward { &parents } else { &children };
        for c in order {
            let mut v = cols.remove(&c).unwrap();
            v.sort_by(|a, b| {
                barycenter(a, neigh, &row)
                    .partial_cmp(&barycenter(b, neigh, &row))
                    .unwrap()
            });
            cols.insert(c, v);
        }
    }

    let mut pos = BTreeMap::new();
    let (mut max_col, mut max_row) = (0usize, 0usize);
    for (&c, v) in &cols {
        max_col = max_col.max(c);
        for (i, id) in v.iter().enumerate() {
            pos.insert(id.clone(), (c as f64 * COL_W, i as f64 * ROW_H));
            max_row = max_row.max(i);
        }
    }
    let mut root_ids = cols.get(&0).cloned().unwrap_or_default();
    root_ids.sort_by(|a, b| (prio_of[a], a).cmp(&(prio_of[b], b)));
    let best_rank = ids.iter().map(|id| prio_of[id]).min().unwrap_or(u8::MAX);

    Section {
        pos,
        width: max_col as f64 * COL_W + NODE_W,
        height: max_row as f64 * ROW_H + NODE_H,
        root_ids,
        best_rank,
        node_ids: ids,
    }
}

/// Mean row of a node's neighbours on the placed side; if it has none, keep it
/// at its current row so it doesn't jump.
fn barycenter(id: &str, neigh: &BTreeMap<String, Vec<String>>, row: &BTreeMap<String, f64>) -> f64 {
    match neigh.get(id) {
        Some(ns) if !ns.is_empty() => {
            ns.iter().filter_map(|n| row.get(n)).sum::<f64>() / ns.len() as f64
        }
        _ => row.get(id).copied().unwrap_or(0.0),
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
