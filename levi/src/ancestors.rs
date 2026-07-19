//! Exact ancestry answers from the real repository via gix merge-base
//! (spec §Status resolution: "Locally: ancestry via gix merge-base against
//! the real repo (exact)").

use std::collections::HashMap;

use gix::ObjectId;
use levi_core::resolve::{Ancestry, AncestorSet};

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
        Self { repo, head, cache: HashMap::new() }
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
