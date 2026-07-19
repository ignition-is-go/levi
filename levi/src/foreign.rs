//! Cross-project machinery (spec 2026-07-19): foreign target parsing, hub
//! resolution of projects/tasks, and the status resolution ladder —
//! sibling checkout (exact) → foreign-status cache (facts) → unknown
//! (treated open, partial). `ls`/`next` only ever touch the ladder's local
//! sources; the cache is refreshed by sync.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use levi_core::materialize::materialize;
use levi_core::resolve::{Resolution, Status, effective_status};
use levi_core::{Task, foreign_key};

use crate::ancestors::GixAncestors;
use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;
use crate::state::{ForeignStatus, ForeignStatusCache};
use crate::store::EventStore;

/// Connect to the configured hub or fail with the cross-project message.
pub fn connect(ctx: &LeviCtx) -> Result<HubSession> {
    let Some(addr) = ctx.config.hub.clone() else {
        bail!(
            "cross-project operations need a hub: run `levi onboard --hub <host:port>` \
             (writes .levi/config.toml)"
        );
    };
    HubSession::connect(&addr, Duration::from_secs(10))
}

/// `myko/lv-7c21[@release/2.x]` → project (name or id), task (prefix or
/// full id), optional consumption ref.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignTarget {
    pub project: String,
    pub task: String,
    pub refname: Option<String>,
}

pub fn parse_target(input: &str) -> Option<ForeignTarget> {
    let (project, rest) = input.split_once('/')?;
    if project.is_empty() || rest.is_empty() {
        return None;
    }
    let (task, refname) = match rest.split_once('@') {
        Some((t, r)) if !t.is_empty() && !r.is_empty() => (t, Some(r.to_string())),
        _ => (rest, None),
    };
    let task = task.strip_prefix("lv-").unwrap_or(task);
    Some(ForeignTarget {
        project: project.to_string(),
        task: task.to_string(),
        refname,
    })
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `<project-id>/<full-task-id>` needs no resolution and works offline.
pub fn offline_target(t: &ForeignTarget) -> Option<(String, String)> {
    (is_hex32(&t.project) && is_hex32(&t.task)).then(|| (t.project.clone(), t.task.clone()))
}

/// Resolve a project name-or-id to (id, name) via the hub registry.
pub fn resolve_project(session: &HubSession, name_or_id: &str) -> Result<(String, String)> {
    let timeout = Duration::from_secs(10);
    let count: levi_core::ProjectCount =
        session.report_once(levi_core::CountAllProjects {}, timeout)?;
    let projects = session.query_at_least(levi_core::GetAllProjects {}, count.count, timeout)?;
    if let Some(p) = projects.iter().find(|p| *p.id.0 == *name_or_id) {
        return Ok((p.id.to_string(), p.name.clone()));
    }
    let by_name: Vec<_> = projects.iter().filter(|p| p.name == name_or_id).collect();
    match by_name.len() {
        0 => bail!("no project named '{name_or_id}' on the hub"),
        1 => Ok((by_name[0].id.to_string(), by_name[0].name.clone())),
        _ => bail!(
            "project name '{name_or_id}' is ambiguous on the hub; use an id: {}",
            by_name
                .iter()
                .map(|p| p.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Resolve a task-id prefix within a foreign project via the hub.
pub fn resolve_foreign_task(
    session: &HubSession,
    project_id: &str,
    prefix: &str,
) -> Result<Arc<Task>> {
    let timeout = Duration::from_secs(10);
    let filter = levi_core::PartialTask {
        project_id: Some(project_id.to_string()),
        ..Default::default()
    };
    let count: levi_core::TaskCount =
        session.report_once(levi_core::CountTasks(filter.clone()), timeout)?;
    let tasks = session.query_at_least(levi_core::GetTasksByQuery(filter), count.count, timeout)?;
    let mut matches: Vec<_> = tasks
        .iter()
        .filter(|t| t.id.0.starts_with(prefix))
        .collect();
    match matches.len() {
        0 => bail!("no task matches '{prefix}' in that project"),
        1 => Ok(matches.remove(0).clone()),
        _ => bail!(
            "'{prefix}' is ambiguous in that project: {}",
            matches
                .iter()
                .map(|t| format!("lv-{}", &t.id.0[..8.min(t.id.0.len())]))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Resolved view of one foreign blocker.
#[derive(Debug, Clone)]
pub struct ForeignInfo {
    pub status: Status,
    pub resolution: Resolution,
    /// When the answer was produced (cache rung only).
    pub observed: Option<String>,
    pub title: Option<String>,
}

/// Resolve every distinct foreign blocker referenced by `world.deps`
/// through the ladder. Purely local: sibling checkouts and the cache.
pub fn resolve_ladder(ctx: &LeviCtx) -> BTreeMap<String, ForeignInfo> {
    let cache = ForeignStatusCache::load(ctx.store.repo());
    let own_workdir = ctx
        .store
        .repo()
        .workdir()
        .map(|w| w.to_path_buf())
        .unwrap_or_default();
    let mut out = BTreeMap::new();
    for dep in ctx.world.deps.values() {
        let Some(project_id) = &dep.blocker_project_id else {
            continue;
        };
        let key = foreign_key(project_id, &dep.blocker_task_id);
        if out.contains_key(&key) {
            continue;
        }
        let info = resolve_one(&cache, project_id, &dep.blocker_task_id, &own_workdir);
        out.insert(key, info);
    }
    out
}

fn resolve_one(
    cache: &ForeignStatusCache,
    project_id: &str,
    task_id: &str,
    own_workdir: &std::path::Path,
) -> ForeignInfo {
    // Rung 1: sibling checkout — resolve against the code this machine
    // actually consumes, exactly.
    if let Some(path) = crate::state::sibling_checkout(project_id, own_workdir)
        && let Ok(store) = EventStore::discover(&path)
        && let Ok(records) = store.read()
    {
        let world = materialize(records);
        if world.project.as_ref().map(|p| p.id.to_string()).as_deref() == Some(project_id) {
            let mut anc = GixAncestors::new(store.repo());
            let resolved =
                effective_status(&world.changes_for(task_id), &mut anc, Resolution::Exact);
            return ForeignInfo {
                status: resolved.status,
                resolution: resolved.resolution,
                observed: None,
                title: world.tasks.get(task_id).map(|t| t.title.clone()),
            };
        }
    }
    // Rung 2: cached hub-facts answer from the last sync.
    if let Some(entry) = cache.get(&foreign_key(project_id, task_id)) {
        let status = if entry.status == "closed" {
            Status::Closed
        } else {
            Status::Open
        };
        return ForeignInfo {
            status,
            resolution: Resolution::Facts,
            observed: Some(entry.observed.clone()),
            title: (!entry.title.is_empty()).then(|| entry.title.clone()),
        };
    }
    // Rung 3: unknown — treated open, never silently unblocked.
    ForeignInfo {
        status: Status::Open,
        resolution: Resolution::Partial,
        observed: None,
        title: None,
    }
}

/// The rank-facing view: key → status only.
pub fn status_map(ladder: &BTreeMap<String, ForeignInfo>) -> BTreeMap<String, Status> {
    ladder.iter().map(|(k, v)| (k.clone(), v.status)).collect()
}

/// Refresh the foreign-status cache from the hub (sync legs only). For each
/// foreign blocker: its StatusChanges, the foreign commit facts, and the
/// head of the consumption ref (dep's `blocker_ref`, else `main`, else the
/// most recently observed RefFact), resolved with `FactsAncestors`.
pub fn refresh_cache(
    ctx: &LeviCtx,
    world: &levi_core::materialize::World,
    session: &HubSession,
) -> Result<usize> {
    let timeout = Duration::from_secs(10);
    let mut cache = ForeignStatusCache::load(ctx.store.repo());
    let mut refreshed = 0usize;
    let mut seen = std::collections::HashSet::new();
    for dep in world.deps.values() {
        let Some(project_id) = &dep.blocker_project_id else {
            continue;
        };
        let key = foreign_key(project_id, &dep.blocker_task_id);
        if !seen.insert(key.clone()) {
            continue;
        }

        // Foreign status changes for the blocker.
        let sc_filter = levi_core::PartialStatusChange {
            task_id: Some(dep.blocker_task_id.clone()),
            ..Default::default()
        };
        let sc_count: levi_core::StatusChangeCount =
            session.report_once(levi_core::CountStatusChanges(sc_filter.clone()), timeout)?;
        let changes = session.query_at_least(
            levi_core::GetStatusChangesByQuery(sc_filter),
            sc_count.count,
            timeout,
        )?;

        // Foreign fact graph + branch heads.
        let cf_filter = levi_core::PartialCommitFact {
            project_id: Some(project_id.clone()),
            ..Default::default()
        };
        let cf_count: levi_core::CommitFactCount =
            session.report_once(levi_core::CountCommitFacts(cf_filter.clone()), timeout)?;
        let facts = session.query_at_least(
            levi_core::GetCommitFactsByQuery(cf_filter),
            cf_count.count,
            timeout,
        )?;
        let rf_filter = levi_core::PartialRefFact {
            project_id: Some(project_id.clone()),
            ..Default::default()
        };
        let rf_count: levi_core::RefFactCount =
            session.report_once(levi_core::CountRefFacts(rf_filter.clone()), timeout)?;
        let refs = session.query_at_least(
            levi_core::GetRefFactsByQuery(rf_filter),
            rf_count.count,
            timeout,
        )?;

        let wanted_ref = dep.blocker_ref.as_deref().unwrap_or("main");
        let head = refs
            .iter()
            .find(|r| r.branch == wanted_ref)
            .or_else(|| refs.iter().max_by(|a, b| a.observed.cmp(&b.observed)))
            .map(|r| r.head.clone());

        let fact_map: BTreeMap<String, levi_core::CommitFact> = facts
            .iter()
            .map(|f| (f.id.to_string(), (**f).clone()))
            .collect();
        let mut sorted: Vec<&levi_core::StatusChange> = changes.iter().map(|c| &**c).collect();
        sorted.sort_by(|a, b| (a.created.as_str(), &*a.id.0).cmp(&(b.created.as_str(), &*b.id.0)));

        let resolved = match &head {
            Some(head) => {
                let mut anc = levi_core::resolve::FactsAncestors::new(&fact_map, head);
                effective_status(&sorted, &mut anc, Resolution::Facts)
            }
            None => {
                // No branch knowledge: only unanchored changes can apply.
                struct NoHead;
                impl levi_core::resolve::AncestorSet for NoHead {
                    fn contains(&mut self, _: &str) -> levi_core::resolve::Ancestry {
                        levi_core::resolve::Ancestry::Unknown
                    }
                }
                effective_status(&sorted, &mut NoHead, Resolution::Partial)
            }
        };

        // Title (best effort).
        let title_filter = levi_core::PartialTask {
            id: Some(dep.blocker_task_id.clone().into()),
            ..Default::default()
        };
        let title = session
            .report_once::<levi_core::CountTasks, levi_core::TaskCount>(
                levi_core::CountTasks(title_filter.clone()),
                timeout,
            )
            .ok()
            .filter(|c| c.count > 0)
            .and_then(|c| {
                session
                    .query_at_least(levi_core::GetTasksByQuery(title_filter), c.count, timeout)
                    .ok()
            })
            .and_then(|tasks| tasks.first().map(|t| t.title.clone()));

        cache.put(
            key,
            ForeignStatus {
                status: resolved.status.label().to_string(),
                resolution: "facts".into(),
                observed: LeviCtx::now(),
                title: title.unwrap_or_default(),
            },
        );
        refreshed += 1;
    }
    if refreshed > 0 {
        cache.save()?;
    }
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        let t = parse_target("myko/lv-7c21").unwrap();
        assert_eq!(t.project, "myko");
        assert_eq!(t.task, "7c21");
        assert_eq!(t.refname, None);
        let t = parse_target("myko/7c21@release/2.x").unwrap();
        assert_eq!(t.refname.as_deref(), Some("release/2.x"));
        assert!(parse_target("no-slash").is_none());
        assert!(parse_target("/task").is_none());
        let full = parse_target(&format!("{}/{}", "a".repeat(32), "b".repeat(32))).unwrap();
        assert!(offline_target(&full).is_some());
        assert!(offline_target(&parse_target("myko/7c21").unwrap()).is_none());
    }
}
