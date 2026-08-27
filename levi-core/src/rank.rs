//! Ranking for `levi next` (spec §Ranking).
//!
//! Eligible = open on this checkout ∧ every blocker closed on this checkout ∧
//! no live claim by someone else. Order: priority (P0 first) → transitive
//! unblock count (closing this frees the most open work) → age (oldest
//! first) → task id. Deterministic and explainable.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::entities::Priority;
use crate::materialize::World;
use crate::resolve::{ResolvedStatus, Status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub dev: String,
    pub machine: String,
    /// Minted per-machine UUID (see levi's state layer). Empty = unknown.
    pub machine_id: String,
    pub worktree: String,
}

/// Is this claim held by `me`? Compares by machine_id when both sides have
/// one; falls back to the display name for legacy events (pre-machine-id).
pub fn claim_is(claim: &crate::entities::Claim, me: &Identity) -> bool {
    let machine_matches = if !claim.machine_id.is_empty() && !me.machine_id.is_empty() {
        claim.machine_id == me.machine_id
    } else {
        claim.machine == me.machine
    };
    claim.dev == me.dev && machine_matches && claim.worktree == me.worktree
}

#[derive(Debug, Clone)]
pub struct RankedTask {
    pub task_id: String,
    pub priority: Priority,
    pub unblocks: usize,
    pub created: String,
    pub reason: String,
}

/// Rank all eligible tasks, best first. `statuses` must contain a resolved
/// status for every task in `world`; `foreign` maps
/// "{project_id}/{task_id}" to the resolved status of cross-project
/// blockers (missing key = unknown = still blocking, spec: never silently
/// unblocked).
pub fn rank_next(
    world: &World,
    statuses: &BTreeMap<String, ResolvedStatus>,
    foreign: &BTreeMap<String, Status>,
    now: DateTime<Utc>,
    me: &Identity,
) -> Vec<RankedTask> {
    let is_open = |id: &str| {
        statuses
            .get(id)
            .map(|r| r.status == Status::Open)
            .unwrap_or(false)
    };

    // blocker -> directly blocked open tasks
    let mut blocks: HashMap<&str, Vec<&str>> = HashMap::new();
    // blocked -> blockers
    let mut blocked_by: HashMap<&str, Vec<&str>> = HashMap::new();
    // blocked -> unmet cross-project blocker keys
    let mut foreign_blocked: HashMap<&str, bool> = HashMap::new();
    for dep in world.deps.values() {
        if !world.tasks.contains_key(&*dep.blocked_task_id) {
            continue;
        }
        if let Some(blocker_project) = &dep.blocker_project_id {
            // Cross-project: satisfied iff the ladder says Closed.
            let key = crate::entities::foreign_key(blocker_project, &dep.blocker_task_id);
            let closed = foreign.get(&key) == Some(&Status::Closed);
            let entry = foreign_blocked.entry(&dep.blocked_task_id).or_insert(false);
            *entry |= !closed;
            continue;
        }
        // Same-project deps referencing deleted tasks are ignored.
        if !world.tasks.contains_key(&*dep.blocker_task_id) {
            continue;
        }
        blocks
            .entry(&dep.blocker_task_id)
            .or_default()
            .push(&dep.blocked_task_id);
        blocked_by
            .entry(&dep.blocked_task_id)
            .or_default()
            .push(&dep.blocker_task_id);
    }

    let mut candidates: Vec<RankedTask> = Vec::new();
    let mut eligible_count = 0usize;
    for (id, task) in &world.tasks {
        if !is_open(id) {
            continue;
        }
        let all_blockers_closed = blocked_by
            .get(id.as_str())
            .map(|bs| bs.iter().all(|b| !is_open(b)))
            .unwrap_or(true);
        if !all_blockers_closed {
            continue;
        }
        if foreign_blocked.get(id.as_str()).copied().unwrap_or(false) {
            continue;
        }
        if let Some(claim) = world.live_claim(id, now) {
            let mine =
                claim.dev == me.dev && claim.machine == me.machine && claim.worktree == me.worktree;
            if !mine {
                continue;
            }
        }
        eligible_count += 1;
        candidates.push(RankedTask {
            task_id: id.clone(),
            priority: task.priority,
            unblocks: transitive_unblocks(id, &blocks, &is_open),
            created: task.created.clone(),
            reason: String::new(),
        });
    }

    candidates.sort_by(|a, b| {
        (
            a.priority.rank(),
            std::cmp::Reverse(a.unblocks),
            a.created.as_str(),
            a.task_id.as_str(),
        )
            .cmp(&(
                b.priority.rank(),
                std::cmp::Reverse(b.unblocks),
                b.created.as_str(),
                b.task_id.as_str(),
            ))
    });

    for (i, c) in candidates.iter_mut().enumerate() {
        let mut parts = vec![c.priority.label().to_string()];
        if c.unblocks > 0 {
            parts.push(format!(
                "unblocks {} open task{}",
                c.unblocks,
                if c.unblocks == 1 { "" } else { "s" }
            ));
        }
        parts.push(if i == 0 {
            format!("ranked 1st of {eligible_count} eligible")
        } else {
            format!("ranked {} of {eligible_count} eligible", ordinal(i + 1))
        });
        c.reason = parts.join("; ");
    }
    candidates
}

/// Count open tasks transitively blocked by `id` (cycle-safe).
fn transitive_unblocks(
    id: &str,
    blocks: &HashMap<&str, Vec<&str>>,
    is_open: &impl Fn(&str) -> bool,
) -> usize {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = blocks.get(id).cloned().unwrap_or_default();
    let mut count = 0;
    while let Some(next) = stack.pop() {
        if next == id || !seen.insert(next) {
            continue;
        }
        if is_open(next) {
            count += 1;
        }
        if let Some(more) = blocks.get(next) {
            stack.extend(more);
        }
    }
    count
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::*;
    use crate::ids::{PrefixError, resolve_prefix, short_id};
    use crate::resolve::Resolution;

    fn me() -> Identity {
        Identity {
            dev: "me@x".into(),
            machine: "m1".into(),
            machine_id: String::new(),
            worktree: "/w1".into(),
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-07-18T12:00:00Z".parse().unwrap()
    }

    fn world_with(tasks: &[(&str, Priority, &str)]) -> (World, BTreeMap<String, ResolvedStatus>) {
        let mut w = World::default();
        let mut statuses = BTreeMap::new();
        for (id, priority, created_at) in tasks {
            w.tasks.insert(
                id.to_string(),
                Task {
                    id: (*id).into(),
                    project_id: "p".into(),
                    title: format!("task {id}"),
                    body: String::new(),
                    priority: *priority,
                    labels: vec![],
                    created_by_dev: "d".into(),
                    created_by_machine: "m".into(),
                    created: created_at.to_string(),
                },
            );
            statuses.insert(
                id.to_string(),
                ResolvedStatus {
                    status: Status::Open,
                    resolution: Resolution::Exact,
                },
            );
        }
        (w, statuses)
    }

    fn dep(w: &mut World, blocker: &str, blocked: &str) {
        let id = dependency_id(blocker, blocked);
        w.deps.insert(
            id.clone(),
            Dependency {
                id: id.into(),
                project_id: "p".into(),
                blocker_task_id: blocker.into(),
                blocked_task_id: blocked.into(),
                blocker_project_id: None,
                blocker_ref: None,
                via: None,
            },
        );
    }

    fn close(statuses: &mut BTreeMap<String, ResolvedStatus>, id: &str) {
        statuses.insert(
            id.to_string(),
            ResolvedStatus {
                status: Status::Closed,
                resolution: Resolution::Exact,
            },
        );
    }

    fn claim(w: &mut World, task: &str, dev: &str, machine: &str, worktree: &str, at: &str) {
        w.claims.insert(
            task.to_string(),
            Claim {
                id: task.into(),
                project_id: "p".into(),
                task_id: task.into(),
                dev: dev.into(),
                machine: machine.into(),
                worktree: worktree.into(),
                machine_id: String::new(),
                git_ref: "refs/heads/test".into(),
                created: at.into(),
                ttl_secs: 86400,
            },
        );
    }

    #[test]
    fn priority_beats_unblock_count() {
        let (mut w, statuses) = world_with(&[
            ("aaaa", Priority::P1, "2026-07-01T00:00:00Z"),
            ("bbbb", Priority::P0, "2026-07-02T00:00:00Z"),
            ("cccc", Priority::P2, "2026-07-03T00:00:00Z"),
            ("dddd", Priority::P2, "2026-07-04T00:00:00Z"),
        ]);
        // aaaa unblocks two tasks; bbbb unblocks none but is P0.
        dep(&mut w, "aaaa", "cccc");
        dep(&mut w, "aaaa", "dddd");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert_eq!(ranked[0].task_id, "bbbb");
        assert_eq!(ranked[1].task_id, "aaaa");
        assert!(ranked[0].reason.starts_with("P0"));
    }

    #[test]
    fn unblock_count_beats_age() {
        let (mut w, statuses) = world_with(&[
            ("aaaa", Priority::P2, "2026-07-01T00:00:00Z"), // older
            ("bbbb", Priority::P2, "2026-07-02T00:00:00Z"), // newer but unblocks
            ("cccc", Priority::P2, "2026-07-03T00:00:00Z"),
        ]);
        dep(&mut w, "bbbb", "cccc");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert_eq!(ranked[0].task_id, "bbbb");
        assert!(ranked[0].reason.contains("unblocks 1 open task"));
    }

    #[test]
    fn transitive_unblocks_counts_chain_and_survives_cycles() {
        let (mut w, mut statuses) = world_with(&[
            ("aaaa", Priority::P2, "2026-07-01T00:00:00Z"),
            ("bbbb", Priority::P2, "2026-07-02T00:00:00Z"),
            ("cccc", Priority::P2, "2026-07-03T00:00:00Z"),
            ("dddd", Priority::P2, "2026-07-04T00:00:00Z"),
        ]);
        // aaaa -> bbbb -> cccc, and a cycle cccc -> aaaa; dddd closed.
        dep(&mut w, "aaaa", "bbbb");
        dep(&mut w, "bbbb", "cccc");
        dep(&mut w, "cccc", "aaaa");
        dep(&mut w, "aaaa", "dddd");
        close(&mut statuses, "dddd");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        // Everything is blocked by an open blocker except… aaaa is blocked by
        // cccc (open), bbbb by aaaa (open), cccc by bbbb (open): cycle makes
        // all ineligible. Break it: close cccc.
        assert!(ranked.is_empty());
        close(&mut statuses, "cccc");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert_eq!(ranked[0].task_id, "aaaa");
        // aaaa unblocks bbbb (open) transitively; cccc and dddd are closed.
        assert_eq!(ranked[0].unblocks, 1);
    }

    #[test]
    fn blocked_task_ineligible_until_blocker_closed() {
        let (mut w, mut statuses) = world_with(&[
            ("aaaa", Priority::P2, "2026-07-01T00:00:00Z"),
            ("bbbb", Priority::P0, "2026-07-02T00:00:00Z"),
        ]);
        dep(&mut w, "aaaa", "bbbb");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert_eq!(
            ranked
                .iter()
                .map(|r| r.task_id.as_str())
                .collect::<Vec<_>>(),
            ["aaaa"]
        );
        close(&mut statuses, "aaaa");
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert_eq!(ranked[0].task_id, "bbbb");
    }

    #[test]
    fn foreign_live_claim_excludes_but_own_or_expired_does_not() {
        let (mut w, statuses) = world_with(&[
            ("aaaa", Priority::P2, "2026-07-01T00:00:00Z"),
            ("bbbb", Priority::P2, "2026-07-02T00:00:00Z"),
            ("cccc", Priority::P2, "2026-07-03T00:00:00Z"),
        ]);
        claim(
            &mut w,
            "aaaa",
            "other@x",
            "m2",
            "/w2",
            "2026-07-18T11:00:00Z",
        ); // foreign, live
        claim(&mut w, "bbbb", "me@x", "m1", "/w1", "2026-07-18T11:00:00Z"); // mine
        claim(
            &mut w,
            "cccc",
            "other@x",
            "m2",
            "/w2",
            "2026-07-01T00:00:00Z",
        ); // expired
        let ranked: Vec<_> = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me())
            .into_iter()
            .map(|r| r.task_id)
            .collect();
        assert_eq!(ranked, ["bbbb", "cccc"]);
    }

    #[test]
    fn short_ids_and_prefix_resolution() {
        let (w, _) = world_with(&[
            ("3f2a99aabbcc", Priority::P2, "2026-07-01T00:00:00Z"),
            ("3f2b00ddeeff", Priority::P2, "2026-07-02T00:00:00Z"),
        ]);
        assert_eq!(short_id(&w, "3f2a99aabbcc"), "lv-3f2a");
        assert_eq!(resolve_prefix(&w, "lv-3f2a").unwrap(), "3f2a99aabbcc");
        assert_eq!(resolve_prefix(&w, "3f2b").unwrap(), "3f2b00ddeeff");
        assert!(matches!(
            resolve_prefix(&w, "3f2"),
            Err(PrefixError::Ambiguous(..))
        ));
        assert!(matches!(
            resolve_prefix(&w, "9999"),
            Err(PrefixError::NotFound(..))
        ));
    }

    #[test]
    fn short_id_lengthens_on_collision() {
        let (w, _) = world_with(&[
            ("3f2a99aabbcc", Priority::P2, "2026-07-01T00:00:00Z"),
            ("3f2a99ffeedd", Priority::P2, "2026-07-02T00:00:00Z"),
        ]);
        assert_eq!(short_id(&w, "3f2a99aabbcc"), "lv-3f2a99aa");
    }

    #[test]
    fn foreign_dep_blocks_until_ladder_says_closed() {
        let (mut w, statuses) = world_with(&[("aaaa", Priority::P0, "2026-07-01T00:00:00Z")]);
        w.deps.insert(
            "projB/ffff->aaaa".into(),
            Dependency {
                id: "projB/ffff->aaaa".into(),
                project_id: "p".into(),
                blocker_task_id: "ffff".into(),
                blocked_task_id: "aaaa".into(),
                blocker_project_id: Some("projB".into()),
                blocker_ref: None,
                via: Some("cargo: crates.io".into()),
            },
        );
        // Unknown foreign status (missing key): blocked.
        let ranked = rank_next(&w, &statuses, &BTreeMap::new(), now(), &me());
        assert!(ranked.is_empty());
        // Foreign open: still blocked.
        let mut foreign = BTreeMap::new();
        foreign.insert("projB/ffff".to_string(), Status::Open);
        assert!(rank_next(&w, &statuses, &foreign, now(), &me()).is_empty());
        // Foreign closed: eligible.
        foreign.insert("projB/ffff".to_string(), Status::Closed);
        let ranked = rank_next(&w, &statuses, &foreign, now(), &me());
        assert_eq!(ranked[0].task_id, "aaaa");
    }

    #[test]
    fn claim_is_prefers_machine_id_with_legacy_fallback() {
        let claim = |machine: &str, machine_id: &str| Claim {
            id: "t".into(),
            project_id: "p".into(),
            task_id: "t".into(),
            dev: "me@x".into(),
            machine: machine.into(),
            machine_id: machine_id.into(),
            worktree: "/w1".into(),
            git_ref: "refs/heads/test".into(),
            created: "2026-07-01T00:00:00Z".into(),
            ttl_secs: 86400,
        };
        let me_with = |machine_id: &str| Identity {
            dev: "me@x".into(),
            machine: "m1".into(),
            machine_id: machine_id.into(),
            worktree: "/w1".into(),
        };
        // Both have ids: ids decide, names ignored.
        assert!(claim_is(&claim("other-name", "id-1"), &me_with("id-1")));
        assert!(!claim_is(&claim("m1", "id-2"), &me_with("id-1")));
        // Either side legacy (empty id): display name decides.
        assert!(claim_is(&claim("m1", ""), &me_with("id-1")));
        assert!(claim_is(&claim("m1", "id-1"), &me_with("")));
        assert!(!claim_is(&claim("m2", ""), &me_with("id-1")));
    }
}
