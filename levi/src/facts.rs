//! Facts leg (spec §Sync leg 3): publish CommitFacts (sha -> parents) for
//! ancestors of anchor commits and current branch heads, depth-capped, plus
//! RefFacts for branch tips. Published to the hub only (spec deviation 6);
//! deduped against `.git/levi/facts-published` (commits, immutable) and
//! `.git/levi/refs-published` (branch heads, which move).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use gix::ObjectId;
use levi_core::materialize::World;
use levi_core::{CommitFact, RefFact, ref_fact_id};
use myko::wire::MEvent;

use crate::ctx::LeviCtx;
use crate::hub_client::HubSession;

const DEPTH_CAP: usize = 2000;

/// Republish an unchanged branch head at most this often. A RefFact whose
/// head hasn't moved carries no new information, but `observed` feeds the
/// cross-project ladder's "closed as of X" display — freezing it forever
/// would make foreign status read staler than it is, so confirmations are
/// refreshed on this interval rather than on every sync (lv-ef44).
const REF_REFRESH: chrono::TimeDelta = chrono::TimeDelta::hours(1);

pub fn publish(ctx: &LeviCtx, world: &World, session: &HubSession) -> Result<usize> {
    let Some(project) = &world.project else {
        bail!("no levi project");
    };
    let project_id = project.id.to_string();
    let repo = ctx.store.repo();

    let cache_path = repo.common_dir().join("levi").join("facts-published-v2");
    let published: HashSet<String> = std::fs::read_to_string(&cache_path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

    // Roots: every anchor sha + every local branch head.
    let mut roots: Vec<ObjectId> = Vec::new();
    let mut heads = HashSet::new();
    let mut ref_facts: Vec<MEvent> = Vec::new();
    for change in &world.status_changes {
        if let Some(sha) = &change.anchor_commit
            && let Ok(oid) = ObjectId::from_hex(sha.as_bytes())
        {
            roots.push(oid);
        }
    }
    // Branch heads: walked as roots every run, but only *published* when
    // the head moved or the last confirmation aged out. Republishing every
    // branch on every sync appended N events per sync forever — with the
    // opportunistic sync that fires after each mutation, a 95-branch repo
    // wrote 95 events per `levi add` (lv-ef44).
    let refs_cache_path = repo.common_dir().join("levi").join("refs-published");
    let mut ref_state = read_ref_cache(&refs_cache_path);
    let mut ref_state_changed = false;
    let now = Utc::now();
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
                heads.insert(head.to_string());
                let head_hex = head.to_string();
                let confirmed = ref_state.get(&branch).is_some_and(|(cached, observed)| {
                    *cached == head_hex && is_recent(observed, now)
                });
                if confirmed {
                    continue;
                }
                let observed = LeviCtx::now();
                let fact = RefFact {
                    id: ref_fact_id(&project_id, &branch).into(),
                    project_id: project_id.clone(),
                    branch: branch.clone(),
                    head: head_hex.clone(),
                    observed: observed.clone(),
                };
                ref_facts.push(ctx.set_event(&fact));
                ref_state.insert(branch, (head_hex, observed));
                ref_state_changed = true;
            }
        }
    }

    // Walk ancestors, depth-capped, skipping already-published shas.
    let mut pending = Vec::new();
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
                pending.push((sha, parents.iter().map(|p| p.to_string()).collect()));
            }
            // Even for already-published commits keep walking: their parents
            // may be unpublished (e.g. cache from a shallow earlier run).
            next.extend(parents);
        }
        frontier = next;
        depth += 1;
    }

    let repo_dir = repo.workdir().unwrap_or_else(|| repo.git_dir());
    let unpublished = pending.iter().map(|(sha, _)| sha.clone()).collect();
    let anchors = world
        .status_changes
        .iter()
        .filter_map(|change| change.anchor_commit.as_deref());
    let patch_of = patch_ids_for_facts(
        repo_dir,
        &heads,
        anchors,
        &unpublished,
        ctx.config.patch_id_window,
    );
    let mut commit_facts: Vec<(MEvent, String)> = pending
        .into_iter()
        .map(|(sha, parents)| {
            let fact = CommitFact {
                id: sha.clone().into(),
                project_id: project_id.clone(),
                parents,
                patch_id: patch_of.get(&sha).cloned(),
            };
            (ctx.set_event(&fact), sha)
        })
        .collect();

    let fact_count = ref_facts.len() + commit_facts.len();
    session.send_events(ref_facts)?;
    // Record the confirmations only once the hub has consumed them (the
    // send barrier). Unlike commit facts there is no by-id query to verify
    // RefFacts against, so a lost one leaves the hub's head stale until the
    // branch moves or REF_REFRESH elapses — which is exactly what bounds it.
    if ref_state_changed {
        write_ref_cache(&refs_cache_path, &ref_state);
    }

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

fn patch_ids_for_facts<'a>(
    repo_dir: &Path,
    heads: &HashSet<String>,
    anchors: impl Iterator<Item = &'a str>,
    unpublished: &HashSet<String>,
    window: usize,
) -> HashMap<String, String> {
    use std::process::{Command, Stdio};

    if unpublished.is_empty() {
        return HashMap::new();
    }
    let anchor_candidates: HashSet<String> = anchors
        .filter(|sha| unpublished.contains(*sha))
        .map(str::to_string)
        .collect();
    let mut candidates = anchor_candidates.clone();
    for head in heads {
        let Ok(out) = Command::new("git")
            .args(["rev-list", "-n", &window.to_string(), head])
            .current_dir(repo_dir)
            .output()
        else {
            continue;
        };
        if out.status.success() {
            candidates.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|sha| unpublished.contains(*sha))
                    .map(str::to_string),
            );
        }
    }
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_unstable();
    let mut patches = HashMap::new();
    // SHA arguments are chunked to stay below Windows command-line limits.
    for chunk in candidates.chunks(128) {
        let Ok(mut log) = Command::new("git")
            .args(["log", "--no-walk", "-p", "--format=%H"])
            .args(chunk)
            .current_dir(repo_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let result = Command::new("git")
            .args(["patch-id", "--stable"])
            .current_dir(repo_dir)
            .stdin(log.stdout.take().expect("piped git stdout"))
            .output();
        let Ok(out) = result else {
            let _ = log.kill();
            let _ = log.wait();
            continue;
        };
        if !log.wait().is_ok_and(|status| status.success()) || !out.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.split_whitespace();
            if let (Some(patch), Some(sha)) = (parts.next(), parts.next()) {
                patches.insert(sha.to_string(), patch.to_string());
            }
        }
    }
    for sha in anchor_candidates {
        if !patches.contains_key(&sha)
            && let Some(patch) = crate::ancestors::patch_id_pub(repo_dir, &sha)
        {
            patches.insert(sha, patch);
        }
    }
    patches
}

/// Was this confirmation made recently enough to skip republishing?
/// An unparseable timestamp counts as stale, so a corrupted cache
/// republishes rather than silently withholding facts from the hub.
fn is_recent(observed: &str, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(observed)
        .map(|t| now.signed_duration_since(t.with_timezone(&Utc)) < REF_REFRESH)
        .unwrap_or(false)
}

/// branch -> (head sha, when we last published that pairing). Tab-separated;
/// git ref names cannot contain tabs or newlines, so the format is
/// unambiguous. A malformed line is dropped, costing one republish.
fn read_ref_cache(path: &Path) -> BTreeMap<String, (String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(branch), Some(head), Some(observed)) if !branch.is_empty() => {
                    Some((branch.to_string(), (head.to_string(), observed.to_string())))
                }
                _ => None,
            }
        })
        .collect()
}

/// Rewritten whole (one line per branch, so it stays small) rather than
/// appended. Best-effort: a write failure costs a republish next run.
fn write_ref_cache(path: &Path, state: &BTreeMap<String, (String, String)>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body: String = state
        .iter()
        .map(|(branch, (head, observed))| format!("{branch}\t{head}\t{observed}\n"))
        .collect();
    let _ = std::fs::write(path, body);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn commit(repo: &Path, name: &str) -> String {
        std::fs::write(repo.join(name), name).unwrap();
        git(repo, &["add", name]);
        git(repo, &["commit", "-qm", name]);
        git(repo, &["rev-parse", "HEAD"])
    }

    #[test]
    fn fact_patches_preserve_each_head_window_and_explicit_anchors() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "Test"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        let base = commit(repo, "base");
        let middle = commit(repo, "middle");
        let main = commit(repo, "main");
        git(repo, &["checkout", "-qb", "side", &middle]);
        let side = commit(repo, "side");
        let heads = HashSet::from([main.clone(), side.clone()]);
        let unpublished = HashSet::from([base.clone(), middle.clone(), main.clone(), side.clone()]);

        let patches =
            patch_ids_for_facts(repo, &heads, [base.as_str()].into_iter(), &unpublished, 1);
        assert_eq!(
            patches.keys().cloned().collect::<HashSet<_>>(),
            HashSet::from([base, main, side]),
            "each branch contributes its own window, while older anchors remain included"
        );
        let patches = patch_ids_for_facts(repo, &heads, std::iter::empty(), &unpublished, 0);
        assert!(
            patches.is_empty(),
            "a zero window excludes unanchored patch IDs"
        );
    }

    #[test]
    fn explicit_merge_anchor_retains_its_combined_diff_patch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "Test"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        commit(repo, "base");
        git(repo, &["branch", "side"]);
        std::fs::write(repo.join("base"), "main\n").unwrap();
        git(repo, &["commit", "-qam", "main"]);
        git(repo, &["checkout", "-q", "side"]);
        std::fs::write(repo.join("base"), "side\n").unwrap();
        git(repo, &["commit", "-qam", "side"]);
        git(repo, &["checkout", "-q", "main"]);
        let merge = std::process::Command::new("git")
            .args(["merge", "--no-commit", "side"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            !merge.status.success(),
            "fixture must produce a merge conflict"
        );
        std::fs::write(repo.join("base"), "resolved\n").unwrap();
        git(repo, &["add", "base"]);
        git(repo, &["commit", "-qm", "resolve conflict"]);
        let anchor = git(repo, &["rev-parse", "HEAD"]);
        let expected = crate::ancestors::patch_id_pub(repo, &anchor)
            .expect("resolved merge has a combined diff");
        let patches = patch_ids_for_facts(
            repo,
            &HashSet::from([anchor.clone()]),
            [anchor.as_str()].into_iter(),
            &HashSet::from([anchor.clone()]),
            1,
        );
        assert_eq!(patches.get(&anchor), Some(&expected));
    }
}
