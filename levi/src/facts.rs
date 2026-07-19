//! Facts leg (spec §Sync leg 3): publish CommitFacts (sha -> parents) for
//! ancestors of anchor commits and current branch heads, depth-capped, plus
//! RefFacts for branch tips. Published to the hub only (spec deviation 6);
//! deduped against `.git/levi/facts-published`.

use std::collections::HashSet;
use std::io::Write;
use std::time::Duration;

use anyhow::Result;
use gix::ObjectId;
use levi_core::{CommitFact, RefFact, ref_fact_id};
use myko::wire::MEvent;

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;

const DEPTH_CAP: usize = 2000;

pub fn publish(ctx: &LeviCtx, session: &HubSession) -> Result<usize> {
    let project_id = ctx.project_id()?;
    let repo = ctx.store.repo();

    let cache_path = repo.common_dir().join("levi").join("facts-published");
    let published: HashSet<String> = std::fs::read_to_string(&cache_path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

    // Roots: every anchor sha + every local branch head.
    let mut roots: Vec<ObjectId> = Vec::new();
    let mut ref_facts: Vec<MEvent> = Vec::new();
    for change in &ctx.world.status_changes {
        if let Some(sha) = &change.anchor_commit
            && let Ok(oid) = ObjectId::from_hex(sha.as_bytes())
        {
            roots.push(oid);
        }
    }
    if let Ok(refs) = repo.references()
        && let Ok(iter) = refs.prefixed("refs/heads/")
    {
        for reference in iter.flatten() {
            let name = reference.name().as_bstr().to_string();
            let branch = name.trim_start_matches("refs/heads/").to_string();
            let mut reference = reference;
            if let Ok(id) = reference.peel_to_id() {
                let head = id.detach();
                roots.push(head);
                let fact = RefFact {
                    id: ref_fact_id(&project_id, &branch).into(),
                    project_id: project_id.clone(),
                    branch,
                    head: head.to_string(),
                    observed_at: LeviCtx::now(),
                };
                ref_facts.push(ctx.set_event(&fact));
            }
        }
    }

    // Walk ancestors, depth-capped, skipping already-published shas.
    let mut new_shas: Vec<String> = Vec::new();
    let mut events: Vec<MEvent> = ref_facts;
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut frontier = roots;
    let mut depth = 0usize;
    while !frontier.is_empty() && depth < DEPTH_CAP {
        let mut next = Vec::new();
        for oid in frontier.drain(..) {
            if !seen.insert(oid) {
                continue;
            }
            let sha = oid.to_string();
            let Ok(object) = repo.find_object(oid) else { continue };
            let Ok(commit) = object.try_into_commit() else { continue };
            let parents: Vec<ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
            if !published.contains(&sha) {
                let fact = CommitFact {
                    id: sha.clone().into(),
                    project_id: project_id.clone(),
                    parents: parents.iter().map(|p| p.to_string()).collect(),
                };
                events.push(ctx.set_event(&fact));
                new_shas.push(sha);
            }
            // Even for already-published commits keep walking: their parents
            // may be unpublished (e.g. cache from a shallow earlier run).
            next.extend(parents);
        }
        frontier = next;
        depth += 1;
    }

    let fact_count = events.len();
    session.send_events(events)?;

    if !new_shas.is_empty() {
        if let Some(dir) = cache_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) =
            std::fs::OpenOptions::new().create(true).append(true).open(&cache_path)
        {
            for sha in &new_shas {
                let _ = writeln!(f, "{sha}");
            }
        }
    }
    Ok(fact_count)
}

/// Small helper for `watch`/tests: a default per-leg timeout.
pub fn leg_timeout() -> Duration {
    Duration::from_secs(10)
}
