//! The cross-project blocking graph (spec
//! 2026-07-21-cross-project-graph-design, Surface 2).
//!
//! Only tasks that participate in a dependency become nodes — with ~200
//! tasks and a handful of edges, drawing every task would be a cloud of
//! unconnected dots. Everything unconnected is returned separately for the
//! backlog list. Layout is a deterministic longest-path layering so a sparse
//! DAG renders identically every time instead of jittering.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use myko::prelude::*;

use crate::entities::{Dependency, Priority, Task};
use crate::resolve::{ResolvedStatus, Status};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphNode {
    pub task_id: String,
    pub project_id: String,
    pub title: String,
    pub priority: Priority,
    pub status: Status,
    /// 0 = blocked by nothing in the graph; deepest-blocker + 1 otherwise.
    pub layer: usize,
    /// True when the task is referenced by an edge but the hub has never seen
    /// it (a foreign blocker not yet synced) — rendered as a stub.
    pub stub: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphEdge {
    pub blocker_task_id: String,
    pub blocked_task_id: String,
    /// None = same project as the blocked task.
    pub blocker_project_id: Option<String>,
    pub via: Option<String>,
    /// The blocker is closed: this edge is the actionable "verify and start".
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct IssueGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Task ids with no dependency edge — the backlog, not drawn in the graph.
    pub unconnected: Vec<String>,
    /// (blocker, blocked) edges dropped to break a cycle, for the UI to flag.
    pub broken_cycles: Vec<(String, String)>,
}

/// Report output: the whole graph, computed on the hub and sent to the
/// dashboard so it never pulls raw entities. Defined here (not in the
/// wasm-gated `hub` module) so the wasm client can call and deserialize it.
#[myko_report_output]
pub struct IssueGraphOut {
    pub graph: IssueGraph,
}

/// The cross-project blocking graph report. The `compute` handler is
/// hub-only (below); constructing and deserializing the report is
/// client-side, which is why the struct lives here.
#[myko_report(IssueGraphOut)]
pub struct IssueGraphReport {}

/// Build the graph. `statuses` maps task id -> resolved status (any precision;
/// the graph only reads the open/closed bit). Tasks referenced by an edge but
/// absent from `tasks` become stub nodes rather than dropped edges.
pub fn build(
    tasks: &BTreeMap<String, Task>,
    deps: &BTreeMap<String, Dependency>,
    statuses: &BTreeMap<String, ResolvedStatus>,
) -> IssueGraph {
    // Distinct edges (dedup: the same block can be recorded more than once).
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut edge_keys: BTreeSet<(String, String)> = BTreeSet::new();
    for dep in deps.values() {
        let key = (dep.blocker_task_id.clone(), dep.blocked_task_id.clone());
        if !edge_keys.insert(key) {
            continue;
        }
        let resolved = statuses
            .get(&dep.blocker_task_id)
            .is_some_and(|s| s.status == Status::Closed);
        edges.push(GraphEdge {
            blocker_task_id: dep.blocker_task_id.clone(),
            blocked_task_id: dep.blocked_task_id.clone(),
            blocker_project_id: dep.blocker_project_id.clone(),
            via: dep.via.clone(),
            resolved,
        });
    }

    // Node set: every task touched by an edge.
    let mut in_graph: BTreeSet<String> = BTreeSet::new();
    for e in &edges {
        in_graph.insert(e.blocker_task_id.clone());
        in_graph.insert(e.blocked_task_id.clone());
    }

    // Adjacency (blocker -> blocked) and in-degree, for layering.
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut indeg: BTreeMap<String, usize> = in_graph.iter().map(|id| (id.clone(), 0)).collect();
    for e in &edges {
        children
            .entry(e.blocker_task_id.clone())
            .or_default()
            .push(e.blocked_task_id.clone());
        *indeg.get_mut(&e.blocked_task_id).unwrap() += 1;
    }

    let (layer, broken_cycles) = layer_nodes(&in_graph, &children, indeg, &edges);

    let mut nodes: Vec<GraphNode> = in_graph
        .iter()
        .map(|id| match tasks.get(id) {
            Some(t) => GraphNode {
                task_id: id.clone(),
                project_id: t.project_id.clone(),
                title: t.title.clone(),
                priority: t.priority,
                status: statuses.get(id).map(|s| s.status).unwrap_or(Status::Open),
                layer: layer[id],
                stub: false,
            },
            // Edge references a task the hub hasn't seen: keep it as a stub.
            None => GraphNode {
                task_id: id.clone(),
                project_id: blocker_project_of(id, deps).unwrap_or_default(),
                title: format!("lv-{}", &id[..id.len().min(8)]),
                priority: Priority::P2,
                status: Status::Open,
                layer: layer[id],
                stub: true,
            },
        })
        .collect();
    // Stable render order: layer, then project, then priority, then id.
    nodes.sort_by(|a, b| {
        (a.layer, &a.project_id, a.priority.rank(), &a.task_id).cmp(&(
            b.layer,
            &b.project_id,
            b.priority.rank(),
            &b.task_id,
        ))
    });

    let unconnected: Vec<String> = tasks
        .keys()
        .filter(|id| !in_graph.contains(*id))
        .cloned()
        .collect();

    IssueGraph {
        nodes,
        edges,
        unconnected,
        broken_cycles,
    }
}

/// Longest-path layering via Kahn's algorithm. Any edge still unresolved when
/// the queue drains sits on a cycle; it is dropped (recorded) and its target
/// released, so the layout terminates and the cycle is surfaced.
fn layer_nodes(
    in_graph: &BTreeSet<String>,
    children: &BTreeMap<String, Vec<String>>,
    mut indeg: BTreeMap<String, usize>,
    edges: &[GraphEdge],
) -> (BTreeMap<String, usize>, Vec<(String, String)>) {
    let mut layer: BTreeMap<String, usize> = in_graph.iter().map(|id| (id.clone(), 0)).collect();
    let mut queue: VecDeque<String> = in_graph
        .iter()
        .filter(|id| indeg[*id] == 0)
        .cloned()
        .collect();
    let mut settled: BTreeSet<String> = BTreeSet::new();

    while let Some(id) = queue.pop_front() {
        settled.insert(id.clone());
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                let next = layer[&id] + 1;
                if next > layer[kid] {
                    layer.insert(kid.clone(), next);
                }
                let d = indeg.get_mut(kid).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push_back(kid.clone());
                }
            }
        }
    }

    // Anything not settled is on a cycle. Report the edges into the unsettled
    // set; their layers keep whatever longest path reached them.
    let mut broken_cycles: Vec<(String, String)> = edges
        .iter()
        .filter(|e| !settled.contains(&e.blocked_task_id))
        .map(|e| (e.blocker_task_id.clone(), e.blocked_task_id.clone()))
        .collect();
    broken_cycles.sort();
    broken_cycles.dedup();
    (layer, broken_cycles)
}

fn blocker_project_of(task_id: &str, deps: &BTreeMap<String, Dependency>) -> Option<String> {
    deps.values()
        .find(|d| d.blocker_task_id == task_id)
        .and_then(|d| d.blocker_project_id.clone())
}

/// Hub-side computation: join the live Tasks / Dependencys / StatusChanges,
/// resolve unanchored status per project, and fold into the graph. No
/// CommitFacts are touched, so the output stays small regardless of history.
///
/// Not cfg-gated: `ReportParams` (which the client needs to *call* the
/// report) requires `ReportHandler`, so this impl must exist on wasm too —
/// the client never invokes `compute`, it asks the hub to.
impl myko::report::ReportHandler for IssueGraphReport {
    type Output = IssueGraphOut;

    fn compute(
        &self,
        ctx: myko::report::ReportContext,
    ) -> impl myko::hyphae::MaterializeDefinite<std::sync::Arc<Self::Output>> {
        use myko::hyphae::JoinExt;
        let tasks = ctx.query_map(crate::GetAllTasks {}).items();
        let deps = ctx.query_map(crate::GetAllDependencys {}).items();
        let changes = ctx.query_map(crate::GetAllStatusChanges {}).items();
        tasks
            .join(&deps)
            .join(&changes)
            .map(|((tasks, deps), changes)| {
                std::sync::Arc::new(IssueGraphOut {
                    graph: build_from_live(tasks, deps, changes),
                })
            })
    }
}

/// Fold live (Arc-wrapped) entities into the graph: unanchored status per
/// project, then [`build`]. Shared by the report and its test.
pub fn build_from_live(
    tasks: &[std::sync::Arc<Task>],
    deps: &[std::sync::Arc<Dependency>],
    changes: &[std::sync::Arc<crate::StatusChange>],
) -> IssueGraph {
    let tasks: Vec<Task> = tasks.iter().map(|t| (**t).clone()).collect();
    let changes: Vec<crate::StatusChange> = changes.iter().map(|c| (**c).clone()).collect();

    let projects: BTreeSet<&str> = tasks.iter().map(|t| t.project_id.as_str()).collect();
    let mut statuses = BTreeMap::new();
    for pid in projects {
        statuses.extend(crate::crossproject::statuses_unanchored(
            &tasks, &changes, pid,
        ));
    }
    let task_map: BTreeMap<String, Task> =
        tasks.into_iter().map(|t| (t.id.to_string(), t)).collect();
    let dep_map: BTreeMap<String, Dependency> = deps
        .iter()
        .map(|d| (d.id.to_string(), (**d).clone()))
        .collect();
    build(&task_map, &dep_map, &statuses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::StatusKind;
    use crate::resolve::Resolution;

    fn task(id: &str, project: &str) -> Task {
        Task {
            id: id.into(),
            project_id: project.into(),
            title: format!("task {id}"),
            body: String::new(),
            priority: Default::default(),
            labels: vec![],
            created_by_dev: "d".into(),
            created_by_machine: "m".into(),
            created: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn dep(id: &str, blocker: &str, blocked: &str) -> Dependency {
        Dependency {
            id: id.into(),
            project_id: "p".into(),
            blocker_task_id: blocker.into(),
            blocked_task_id: blocked.into(),
            blocker_project_id: None,
            blocker_ref: None,
            via: None,
        }
    }

    fn open(id: &str) -> (String, ResolvedStatus) {
        (
            id.into(),
            ResolvedStatus {
                status: Status::Open,
                resolution: Resolution::Exact,
            },
        )
    }

    fn tasks_of(ts: Vec<Task>) -> BTreeMap<String, Task> {
        ts.into_iter().map(|t| (t.id.to_string(), t)).collect()
    }
    fn deps_of(ds: Vec<Dependency>) -> BTreeMap<String, Dependency> {
        ds.into_iter().map(|d| (d.id.to_string(), d)).collect()
    }

    #[test]
    fn chain_layers_0_1_2() {
        let tasks = tasks_of(vec![task("a", "p"), task("b", "p"), task("c", "p")]);
        let deps = deps_of(vec![dep("d1", "a", "b"), dep("d2", "b", "c")]);
        let statuses = [open("a"), open("b"), open("c")].into_iter().collect();
        let g = build(&tasks, &deps, &statuses);
        let layer = |id: &str| g.nodes.iter().find(|n| n.task_id == id).unwrap().layer;
        assert_eq!(layer("a"), 0);
        assert_eq!(layer("b"), 1);
        assert_eq!(layer("c"), 2);
        assert!(g.unconnected.is_empty());
        assert!(g.broken_cycles.is_empty());
    }

    #[test]
    fn diamond_takes_longest_path() {
        // a->b, a->c, b->d, c->d : d must be layer 2 (via either arm).
        let tasks = tasks_of(vec![
            task("a", "p"),
            task("b", "p"),
            task("c", "p"),
            task("d", "p"),
        ]);
        let deps = deps_of(vec![
            dep("1", "a", "b"),
            dep("2", "a", "c"),
            dep("3", "b", "d"),
            dep("4", "c", "d"),
        ]);
        let statuses = ["a", "b", "c", "d"].iter().map(|i| open(i)).collect();
        let g = build(&tasks, &deps, &statuses);
        let layer = |id: &str| g.nodes.iter().find(|n| n.task_id == id).unwrap().layer;
        assert_eq!(layer("d"), 2);
    }

    #[test]
    fn cycle_is_broken_and_recorded_without_hanging() {
        let tasks = tasks_of(vec![task("a", "p"), task("b", "p")]);
        let deps = deps_of(vec![dep("1", "a", "b"), dep("2", "b", "a")]);
        let statuses = [open("a"), open("b")].into_iter().collect();
        let g = build(&tasks, &deps, &statuses);
        assert!(!g.broken_cycles.is_empty(), "cycle must be reported");
        assert_eq!(g.nodes.len(), 2, "both nodes still present");
    }

    #[test]
    fn unconnected_tasks_are_excluded_from_nodes() {
        let tasks = tasks_of(vec![task("a", "p"), task("b", "p"), task("lonely", "p")]);
        let deps = deps_of(vec![dep("1", "a", "b")]);
        let statuses = ["a", "b", "lonely"].iter().map(|i| open(i)).collect();
        let g = build(&tasks, &deps, &statuses);
        assert!(!g.nodes.iter().any(|n| n.task_id == "lonely"));
        assert_eq!(g.unconnected, vec!["lonely".to_string()]);
    }

    #[test]
    fn closed_blocker_marks_edge_resolved() {
        let tasks = tasks_of(vec![task("a", "p"), task("b", "p")]);
        let deps = deps_of(vec![dep("1", "a", "b")]);
        let statuses = [
            (
                "a".to_string(),
                ResolvedStatus {
                    status: Status::Closed,
                    resolution: Resolution::Facts,
                },
            ),
            open("b"),
        ]
        .into_iter()
        .collect();
        let g = build(&tasks, &deps, &statuses);
        assert!(g.edges[0].resolved);
    }

    #[test]
    fn cross_project_edge_carries_project_and_via() {
        let tasks = tasks_of(vec![task("local", "downstream")]);
        let mut d = dep("1", "foreign", "local");
        d.blocker_project_id = Some("upstream".into());
        d.via = Some("cargo: crates.io myko >=4.24.4".into());
        let deps = deps_of(vec![d]);
        let statuses = [open("local")].into_iter().collect();
        let g = build(&tasks, &deps, &statuses);
        assert_eq!(g.edges[0].blocker_project_id.as_deref(), Some("upstream"));
        assert_eq!(
            g.edges[0].via.as_deref(),
            Some("cargo: crates.io myko >=4.24.4")
        );
        // The foreign blocker isn't a known task -> stub node.
        let stub = g.nodes.iter().find(|n| n.task_id == "foreign").unwrap();
        assert!(stub.stub);
        assert_eq!(stub.project_id, "upstream");
    }

    #[test]
    fn no_dependencies_yields_empty_graph_all_unconnected() {
        let tasks = tasks_of(vec![task("a", "p"), task("b", "p")]);
        let g = build(
            &tasks,
            &deps_of(vec![]),
            &[open("a"), open("b")].into_iter().collect(),
        );
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
        assert_eq!(g.unconnected.len(), 2);
    }

    // Silence unused-import warnings when StatusKind isn't otherwise used.
    #[allow(dead_code)]
    fn _uses_status_kind() -> StatusKind {
        StatusKind::Closed
    }
}
