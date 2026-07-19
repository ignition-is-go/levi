//! Exact ancestry answers from the real repository via gix merge-base
//! (spec §Status resolution: "Locally: ancestry via gix merge-base against
//! the real repo (exact)").

use std::collections::HashMap;

use gix::ObjectId;
use levi_core::resolve::{AncestorSet, Ancestry};

pub struct GixAncestors<'r> {
    repo: &'r gix::Repository,
    head: Option<ObjectId>,
    cache: HashMap<String, Ancestry>,
}

impl<'r> GixAncestors<'r> {
    /// Resolve against the current checkout's HEAD (None on unborn HEAD).
    pub fn new(repo: &'r gix::Repository) -> Self {
        let head = repo.head_id().ok().map(|id| id.detach());
        Self::at(repo, head)
    }

    /// Resolve against an arbitrary head (e.g. a branch tip for `--branch`).
    pub fn at(repo: &'r gix::Repository, head: Option<ObjectId>) -> Self {
        Self {
            repo,
            head,
            cache: HashMap::new(),
        }
    }

    fn lookup(&self, sha: &str) -> Ancestry {
        let Some(head) = self.head else {
            // Unborn HEAD: nothing is an ancestor, and we know it exactly.
            return Ancestry::No;
        };
        let Ok(anchor) = ObjectId::from_hex(sha.as_bytes()) else {
            return Ancestry::Unknown;
        };
        // An anchor we don't have locally can't be judged: unfetched history.
        if self.repo.try_find_object(anchor).ok().flatten().is_none() {
            return Ancestry::Unknown;
        }
        if anchor == head {
            return Ancestry::Yes;
        }
        match self.repo.merge_base(head, anchor) {
            Ok(base) => {
                if base.detach() == anchor {
                    Ancestry::Yes
                } else {
                    Ancestry::No
                }
            }
            // No common ancestor at all.
            Err(_) => Ancestry::No,
        }
    }
}

impl AncestorSet for GixAncestors<'_> {
    fn contains(&mut self, sha: &str) -> Ancestry {
        if let Some(a) = self.cache.get(sha) {
            return *a;
        }
        let a = self.lookup(sha);
        self.cache.insert(sha.to_string(), a);
        a
    }
}

/// An orphaned anchor, with the rewritten commit it likely became when a
/// patch-id match is found in recent history (spec §Anchoring rules:
/// "patch-id match against rewritten commits where cheap").
#[derive(Debug, Clone)]
pub struct OrphanedAnchor {
    pub sha: String,
    pub rewritten_as: Option<String>,
}

/// Orphaned-anchor detection (spec §Anchoring rules): a rebase/cherry-pick
/// moves shas, leaving an anchor unreachable from every ref — the task then
/// looks open on the rewritten history. Correct per the model, but
/// surprising, so the CLI warns and suggests a re-close. Checks only the
/// anchors passed in (the tasks being displayed) to stay cheap; patch-id
/// comparison runs only once an orphan is actually found.
pub fn orphaned_anchors(
    repo: &gix::Repository,
    anchors: impl IntoIterator<Item = String>,
) -> Vec<OrphanedAnchor> {
    // Collect ref tips once (branches, tags, remotes — not refs/levi/*).
    let mut tips: Vec<ObjectId> = Vec::new();
    if let Ok(platform) = repo.references()
        && let Ok(iter) = platform.all()
    {
        for reference in iter.flatten() {
            let name = reference.name().as_bstr().to_string();
            if name.starts_with("refs/levi/") {
                continue;
            }
            let mut reference = reference;
            if let Ok(id) = reference.peel_to_id() {
                tips.push(id.detach());
            }
        }
    }
    let mut orphaned = Vec::new();
    let mut seen = HashMap::new();
    for sha in anchors {
        if seen.contains_key(&sha) {
            continue;
        }
        let Ok(anchor) = ObjectId::from_hex(sha.as_bytes()) else {
            continue;
        };
        // Missing objects are "unfetched", not orphaned — different warning.
        if repo.try_find_object(anchor).ok().flatten().is_none() {
            seen.insert(sha, false);
            continue;
        }
        let reachable = tips.iter().any(|tip| {
            *tip == anchor
                || repo
                    .merge_base(*tip, anchor)
                    .map(|b| b.detach() == anchor)
                    .unwrap_or(false)
        });
        seen.insert(sha.clone(), !reachable);
        if !reachable {
            orphaned.push(sha);
        }
    }
    if orphaned.is_empty() {
        return Vec::new();
    }
    // Match orphans against recent history by patch-id: same diff under a
    // new sha is almost certainly the rebased/reworded descendant.
    let repo_dir = repo
        .workdir()
        .unwrap_or_else(|| repo.git_dir())
        .to_path_buf();
    let recent = recent_patch_ids(&repo_dir, 300);
    orphaned
        .into_iter()
        .map(|sha| {
            let rewritten_as = patch_id(&repo_dir, &sha).and_then(|pid| {
                recent
                    .iter()
                    .find(|(candidate_pid, candidate_sha)| {
                        *candidate_pid == pid && *candidate_sha != sha
                    })
                    .map(|(_, candidate_sha)| candidate_sha.clone())
            });
            OrphanedAnchor { sha, rewritten_as }
        })
        .collect()
}

/// `git patch-id --stable` of one commit; None for empty diffs.
fn patch_id(repo_dir: &std::path::Path, sha: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let show = Command::new("git")
        .args(["show", sha])
        .current_dir(repo_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = Command::new("git")
        .args(["patch-id", "--stable"])
        .current_dir(repo_dir)
        .stdin(show.stdout?)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().next().map(str::to_string)
}

/// (patch-id, sha) pairs for the last `limit` commits on HEAD.
fn recent_patch_ids(repo_dir: &std::path::Path, limit: usize) -> Vec<(String, String)> {
    use std::process::{Command, Stdio};
    let Ok(log) = Command::new("git")
        .args(["log", "-p", "--format=%H", "-n", &limit.to_string()])
        .current_dir(repo_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(stdout) = log.stdout else {
        return Vec::new();
    };
    let Ok(out) = Command::new("git")
        .args(["patch-id", "--stable"])
        .current_dir(repo_dir)
        .stdin(stdout)
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
}

/// A close that exists in the log but doesn't apply on this checkout — the
/// fix is real, it just lives on ancestry we don't have. Agents should merge
/// or cherry-pick it rather than re-implement (lv-98bf).
#[derive(Debug, Clone)]
pub struct ElsewhereClose {
    pub anchor: String,
    pub at: String,
    pub by: String,
    /// Local branches whose tips contain the anchor.
    pub branches: Vec<String>,
}

/// Local branch tips, collected once per invocation for containment checks.
pub struct BranchTips {
    tips: Vec<(String, ObjectId)>,
}

impl BranchTips {
    pub fn new(repo: &gix::Repository) -> Self {
        let mut tips = Vec::new();
        if let Ok(platform) = repo.references()
            && let Ok(iter) = platform.prefixed("refs/heads/")
        {
            for reference in iter.flatten() {
                let name = reference
                    .name()
                    .as_bstr()
                    .to_string()
                    .trim_start_matches("refs/heads/")
                    .to_string();
                let mut reference = reference;
                if let Ok(id) = reference.peel_to_id() {
                    tips.push((name, id.detach()));
                }
            }
        }
        Self { tips }
    }

    fn containing(&self, repo: &gix::Repository, sha: &str) -> Vec<String> {
        let Ok(anchor) = ObjectId::from_hex(sha.as_bytes()) else {
            return Vec::new();
        };
        self.tips
            .iter()
            .filter(|(_, tip)| {
                *tip == anchor
                    || repo
                        .merge_base(*tip, anchor)
                        .map(|b| b.detach() == anchor)
                        .unwrap_or(false)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Non-applying closes for one task: `to_status == Closed`, anchored, and the
/// anchor is definitively *not* in this checkout's ancestry (unknown anchors
/// are already surfaced as partial resolution, not here).
pub fn elsewhere_closes(
    repo: &gix::Repository,
    changes: &[&levi_core::StatusChange],
    anc: &mut GixAncestors,
    tips: &BranchTips,
) -> Vec<ElsewhereClose> {
    changes
        .iter()
        .filter(|c| matches!(c.to_status, levi_core::StatusKind::Closed))
        .filter_map(|c| {
            let sha = c.anchor_commit.as_ref()?;
            if anc.contains(sha) != Ancestry::No {
                return None;
            }
            Some(ElsewhereClose {
                anchor: sha.clone(),
                at: c.created.clone(),
                by: c.by_dev.clone(),
                branches: tips.containing(repo, sha),
            })
        })
        .collect()
}
