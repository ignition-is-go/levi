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

/// Orphaned-anchor detection (spec §Anchoring rules): a rebase/cherry-pick
/// moves shas, leaving an anchor unreachable from every ref — the task then
/// looks open on the rewritten history. Correct per the model, but
/// surprising, so the CLI warns and suggests a re-close. Checks only the
/// anchors passed in (the tasks being displayed) to stay cheap.
pub fn orphaned_anchors(
    repo: &gix::Repository,
    anchors: impl IntoIterator<Item = String>,
) -> Vec<String> {
    // Collect ref tips once (branches, tags, remotes — not refs/levi/*).
    let mut tips: Vec<ObjectId> = Vec::new();
    if let Ok(platform) = repo.references()
        && let Ok(iter) = platform.all() {
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
    orphaned
}
