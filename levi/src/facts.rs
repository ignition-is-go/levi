//! Facts leg (spec §Sync leg 3): publish CommitFacts (sha -> parents) for
//! ancestors of anchor commits and current branch heads, depth-capped, plus
//! RefFacts for branch tips. Published to the hub only (spec deviation 6);
//! deduped against `.git/levi/facts-published`.

use std::collections::HashSet;
use std::io::Write;
use std::time::Duration;

use anyhow::{Result, bail};
use gix::ObjectId;
use levi_core::materialize::World;
use levi_core::{CommitFact, RefFact, ref_fact_id};
use myko::wire::MEvent;

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;

const DEPTH_CAP: usize = 2000;

pub fn publish(ctx: &LeviCtx, world: &World, session: &HubSession) -> Result<usize> {
    let Some(project) = &world.project else {
        bail!("no levi project");
    };
    let project_id = project.id.to_string();
    let repo = ctx.store.repo();

    let cache_path = repo.common_dir().join("levi").join("facts-published");
    let published: HashSet<String> = std::fs::read_to_string(&cache_path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

    // Roots: every anchor sha + every local branch head.
    let mut roots: Vec<ObjectId> = Vec::new();
    let mut ref_facts: Vec<MEvent> = Vec::new();
    for change in &world.status_changes {
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
                    observed: LeviCtx::now(),
                };
                ref_facts.push(ctx.set_event(&fact));
            }
        }
    }

    // Walk ancestors, depth-capped, skipping already-published shas.
    let mut commit_facts: Vec<(MEvent, String)> = Vec::new();
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
            let Ok(object) = repo.find_object(oid) else {
                continue;
            };
            let Ok(commit) = object.try_into_commit() else {
                continue;
            };
            let parents: Vec<ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
            if !published.contains(&sha) {
                let fact = CommitFact {
                    id: sha.clone().into(),
                    project_id: project_id.clone(),
                    parents: parents.iter().map(|p| p.to_string()).collect(),
                };
                commit_facts.push((ctx.set_event(&fact), sha));
            }
            // Even for already-published commits keep walking: their parents
            // may be unpublished (e.g. cache from a shallow earlier run).
            next.extend(parents);
        }
        frontier = next;
        depth += 1;
    }

    let fact_count = ref_facts.len() + commit_facts.len();
    session.send_events(ref_facts)?;

    // Publish commit facts chunk by chunk: send, verify the hub holds every
    // sha, record the chunk in the dedup cache — then the next chunk. A
    // failure mid-way keeps the verified chunks cached, so a retry resumes
    // where this run stopped instead of re-sending the whole history
    // (lv-b69e: a first publication from a large repo never survived the
    // all-or-nothing batch). Presence is checked by id (not by count) so
    // facts concurrently published by another machine don't confuse the
    // check; a dropped chunk stays out of the cache and is re-sent next run.
    while !commit_facts.is_empty() {
        let take = commit_facts.len().min(crate::hub_client::SEND_CHUNK);
        let (events, shas): (Vec<MEvent>, Vec<String>) = commit_facts.drain(..take).unzip();
        session.send_events(events)?;
        session
            .query_at_least(
                levi_core::GetCommitFactsByIds {
                    ids: shas.iter().map(|s| s.as_str().into()).collect(),
                },
                shas.len(),
                Duration::from_secs(10),
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "hub did not acknowledge {} commit facts; \
                     not recording them as published: {e}",
                    shas.len()
                )
            })?;
        append_cache(&cache_path, &shas);
    }
    Ok(fact_count)
}

/// Append verified shas to the dedup cache. Best-effort (matching the
/// original behavior): a cache write failure only costs a re-send later.
fn append_cache(cache_path: &std::path::Path, shas: &[String]) {
    if let Some(dir) = cache_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cache_path)
    {
        for sha in shas {
            let _ = writeln!(f, "{sha}");
        }
    }
}

/// Small helper for `watch`/tests: a default per-leg timeout.
pub fn leg_timeout() -> Duration {
    Duration::from_secs(10)
}
