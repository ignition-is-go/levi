//! Per-invocation context: discover the repo, load + materialize the event
//! log, detect identity. There is no daemon — this is the "embedded node"
//! (spec §Architecture; materialization itself lives in levi-core).

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use gix::ObjectId;
use levi_core::materialize::{World, materialize};
use levi_core::rank::Identity;
use levi_core::resolve::{AncestorSet, Resolution, ResolvedStatus, effective_status};
use myko::prelude::Eventable;
use myko::wire::{MEvent, MEventType};

use crate::ancestors::GixAncestors;
use crate::config::LeviConfig;
use crate::store::EventStore;

pub struct LeviCtx {
    pub store: EventStore,
    pub world: World,
    pub identity: Identity,
    pub config: LeviConfig,
    pub no_sync: bool,
}

impl LeviCtx {
    pub fn load(no_sync: bool) -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let store = EventStore::discover(&cwd)?;
        let identity = detect_identity(store.repo())?;
        let config = LeviConfig::load(store.repo());
        let world = materialize(store.read()?);
        // Opportunistic sibling registration: any repo levi has run in is
        // findable as a sibling checkout from every other repo.
        if let (Some(project), Some(workdir)) = (&world.project, store.repo().workdir()) {
            crate::state::register_checkout(&project.id.0, workdir);
        }
        Ok(Self {
            store,
            world,
            identity,
            config,
            no_sync,
        })
    }

    /// Re-read and re-materialize (after an append, e.g. for `next --claim`).
    pub fn reload(&mut self) -> Result<()> {
        self.world = materialize(self.store.read()?);
        Ok(())
    }

    pub fn project_id(&self) -> Result<String> {
        match &self.world.project {
            Some(p) => Ok(p.id.to_string()),
            None => bail!(
                "no levi project here. Run `levi init`, or fetch existing events with \
                 `git fetch <remote> '+refs/levi/events:refs/levi/events'` (or `levi sync`)."
            ),
        }
    }

    /// True when the events ref is entirely absent (fresh clone, never fetched).
    pub fn uninitialized(&self) -> bool {
        self.world.project.is_none()
    }

    /// Append events and (unless `--no-sync`) kick the opportunistic
    /// best-effort hub sync. Returns the new event ids.
    pub fn append_and_sync(&self, events: Vec<MEvent>) -> Result<Vec<String>> {
        let ids = self.store.append(&events)?;
        if !self.no_sync {
            crate::sync::opportunistic(self);
        }
        Ok(ids)
    }

    /// Canonical event timestamp: RFC3339 UTC, fixed-width micros, so
    /// lexicographic order == chronological order (LWW sort key).
    pub fn now() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
    }

    /// Build a SET event stamped with this machine's identity and the
    /// canonical timestamp format.
    pub fn set_event<T: Eventable>(&self, item: &T) -> MEvent {
        let mut ev = MEvent::from_item(item, MEventType::SET, &self.identity.machine);
        ev.created_at = Self::now().into();
        ev
    }

    pub fn del_event<T: Eventable>(&self, item: &T) -> MEvent {
        let mut ev = MEvent::from_item(item, MEventType::DEL, &self.identity.machine);
        ev.created_at = Self::now().into();
        ev
    }

    /// Resolve every task's status against the given ancestor set.
    pub fn statuses(
        &self,
        anc: &mut impl AncestorSet,
        base: Resolution,
    ) -> BTreeMap<String, ResolvedStatus> {
        self.world
            .tasks
            .keys()
            .map(|id| {
                let changes = self.world.changes_for(id);
                (id.clone(), effective_status(&changes, anc, base))
            })
            .collect()
    }

    /// Ancestors for HEAD, or for `--branch X` when given.
    pub fn ancestors_for(&self, branch: Option<&str>) -> Result<GixAncestors<'_>> {
        let window = self.config.patch_id_window;
        match branch {
            None => Ok(GixAncestors::new(self.store.repo()).with_window(window)),
            Some(name) => {
                let head = self.rev(name)?;
                Ok(GixAncestors::at(self.store.repo(), Some(head)).with_window(window))
            }
        }
    }

    /// Resolve a revision string (branch name, sha, …) to a commit id.
    pub fn rev(&self, spec: &str) -> Result<ObjectId> {
        let id = self
            .store
            .repo()
            .rev_parse_single(spec)
            .with_context(|| format!("cannot resolve '{spec}'"))?;
        Ok(id.object()?.peel_to_kind(gix::object::Kind::Commit)?.id)
    }

    /// The full symbolic Git ref currently checked out.
    ///
    /// Claims are branch-owned, so detached HEAD is not a valid claiming
    /// context (including CI checkouts unless they create/check out a branch).
    pub fn current_git_ref(&self) -> Result<String> {
        let name = self
            .store
            .repo()
            .head_name()
            .context("cannot read HEAD")?
            .context("cannot claim from detached HEAD; check out a branch first")?;
        let name = name.as_bstr().to_string();
        if !name.starts_with("refs/heads/") {
            bail!("cannot claim from non-branch Git ref '{name}'; check out a branch first");
        }
        Ok(name)
    }

    /// This checkout's HEAD commit, if born.
    pub fn head_commit(&self) -> Option<ObjectId> {
        self.store.repo().head_id().ok().map(|id| id.detach())
    }
}

fn detect_identity(repo: &gix::Repository) -> Result<Identity> {
    let snap = repo.config_snapshot();
    let dev = snap
        .string("user.email")
        .map(|v| v.to_string())
        .filter(|s| !s.is_empty())
        .context("git config user.email is not set — levi uses it as your developer identity")?;
    let machine = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".into());
    let machine_id = crate::state::machine_id();
    let worktree = match repo.workdir() {
        Some(wd) => wd.canonicalize().unwrap_or_else(|_| wd.to_path_buf()),
        None => repo.git_dir().to_path_buf(),
    };
    Ok(Identity {
        dev,
        machine,
        machine_id,
        worktree: worktree.display().to_string(),
    })
}
