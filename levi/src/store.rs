//! The event store: an append-only log of content-addressed CBOR event blobs
//! kept in `refs/levi/events`, never the working tree (spec §Storage).
//!
//! - Each event is one CBOR blob; its git blob OID is the event id.
//! - A commit on the ref holds a tree of all events, sharded by OID prefix
//!   (`ab/cdef12…`), like `.git/objects`.
//! - Append = write blobs + new tree + commit, then update the ref by
//!   compare-and-swap; on a race, re-read, union, retry (bounded).
//! - Merge of two histories = tree union + merge commit. Events are immutable
//!   and add-only, so union is always conflict-free.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, anyhow};
use gix::ObjectId;
use gix::objs::tree::{Entry, EntryKind};
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::Target;
use levi_core::materialize::EventRecord;
use myko::wire::MEvent;

pub const EVENTS_REF: &str = "refs/levi/events";
const MAX_CAS_RETRIES: usize = 5;

pub struct EventStore {
    repo: gix::Repository,
}

impl EventStore {
    /// Discover the repository containing `dir` (worktree-aware).
    pub fn discover(dir: &Path) -> anyhow::Result<Self> {
        let repo = gix::discover(dir).context("not inside a git repository")?;
        Ok(Self { repo })
    }

    pub fn repo(&self) -> &gix::Repository {
        &self.repo
    }

    /// Canonical CBOR encoding of one event.
    pub fn encode(event: &MEvent) -> Vec<u8> {
        let mut buf = Vec::new();
        ciborium::into_writer(event, &mut buf).expect("CBOR encoding of MEvent cannot fail");
        buf
    }

    /// Current head commit of the events ref, if any.
    pub fn head(&self) -> anyhow::Result<Option<ObjectId>> {
        match self.repo.try_find_reference(EVENTS_REF)? {
            Some(mut r) => Ok(Some(r.peel_to_id()?.detach())),
            None => Ok(None),
        }
    }

    /// All events on the ref. Absent ref ⇒ empty.
    pub fn read(&self) -> anyhow::Result<Vec<EventRecord>> {
        let Some(head) = self.head()? else { return Ok(Vec::new()) };
        let mut records = Vec::new();
        for (_path, oid) in self.blob_entries(head)? {
            let blob = self.repo.find_object(oid)?;
            let event: MEvent = ciborium::from_reader(blob.data.as_slice())
                .with_context(|| format!("event blob {oid} is not valid CBOR"))?;
            records.push(EventRecord { id: oid.to_string(), event });
        }
        Ok(records)
    }

    /// Append events; returns their ids (blob OIDs). Compare-and-swap with
    /// bounded retries; safe against concurrent invocations.
    pub fn append(&self, events: &[MEvent]) -> anyhow::Result<Vec<String>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut new_ids = Vec::with_capacity(events.len());
        let mut blobs = Vec::with_capacity(events.len());
        for event in events {
            let bytes = Self::encode(event);
            let oid = self.repo.write_blob(&bytes)?.detach();
            new_ids.push(oid.to_string());
            blobs.push(oid);
        }

        self.with_cas_retry(|old| {
            let mut shards = match old {
                Some(head) => self.load_shards(head)?,
                None => BTreeMap::new(),
            };
            let mut added = 0usize;
            for oid in &blobs {
                if insert_blob(&mut shards, *oid) {
                    added += 1;
                }
            }
            if added == 0 && old.is_some() {
                return Ok(None); // everything already present; nothing to commit
            }
            let tree = self.write_shard_tree(shards)?;
            let msg = format!("levi: {added} event(s)");
            Ok(Some(self.write_ref_commit(tree, old.into_iter().collect(), &msg)?))
        })?;
        Ok(new_ids)
    }

    /// Union-merge another events ref (e.g. `refs/levi/remotes/origin/events`)
    /// into ours. Returns the number of events that were new to us.
    pub fn merge_ref(&self, other_ref: &str) -> anyhow::Result<usize> {
        let Some(mut r) = self.repo.try_find_reference(other_ref)? else {
            return Ok(0);
        };
        let theirs = r.peel_to_id()?.detach();
        self.merge_commit(theirs)
    }

    /// Union-merge the events reachable from `theirs` (a commit on some
    /// events ref) into our ref.
    pub fn merge_commit(&self, theirs: ObjectId) -> anyhow::Result<usize> {
        let their_shards = self.load_shards(theirs)?;
        let mut new_count = 0usize;
        self.with_cas_retry(|old| {
            let Some(ours) = old else {
                // No local ref yet: adopt theirs wholesale.
                new_count = their_shards.values().map(BTreeMap::len).sum();
                return Ok(Some(theirs));
            };
            if ours == theirs {
                return Ok(None);
            }
            let mut shards = self.load_shards(ours)?;
            let mut added = 0usize;
            for (shard, entries) in &their_shards {
                for (name, oid) in entries {
                    if shards
                        .entry(shard.clone())
                        .or_default()
                        .insert(name.clone(), *oid)
                        .is_none()
                    {
                        added += 1;
                    }
                }
            }
            new_count = added;
            if added == 0 {
                // Theirs ⊆ ours. Only merge-commit if theirs isn't already an
                // ancestor (keeps sync idempotent).
                if self.repo.merge_base(ours, theirs).map(|b| b.detach() == theirs).unwrap_or(false)
                {
                    return Ok(None);
                }
            }
            let tree = self.write_shard_tree(shards)?;
            let msg = format!("levi: merge {added} event(s)");
            Ok(Some(self.write_ref_commit(tree, vec![ours, theirs], &msg)?))
        })?;
        Ok(new_count)
    }

    /// Serialize mutations of the events ref across processes/threads on this
    /// machine. gix's transaction lock does not revalidate the expected value
    /// after waiting for a contended ref lock, so concurrent CAS updates can
    /// silently clobber each other; an flock around read→build→edit closes
    /// that window (and auto-releases if the process dies).
    fn mutation_lock(&self) -> anyhow::Result<std::fs::File> {
        let path = self.repo.common_dir().join("levi-events.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open lock file {}", path.display()))?;
        fs4::fs_std::FileExt::lock_exclusive(&file)
            .with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(file)
    }

    /// Run `build` against the current ref target and CAS the result in;
    /// `build` returning `None` means nothing to do. Retries on races.
    fn with_cas_retry(
        &self,
        mut build: impl FnMut(Option<ObjectId>) -> anyhow::Result<Option<ObjectId>>,
    ) -> anyhow::Result<()> {
        let _lock = self.mutation_lock()?;
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..MAX_CAS_RETRIES {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(20 * attempt as u64));
            }
            let old = self.head()?;
            let Some(new) = build(old)? else { return Ok(()) };
            let expected = match old {
                Some(id) => PreviousValue::MustExistAndMatch(Target::Object(id)),
                None => PreviousValue::MustNotExist,
            };
            let edit = RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "levi append".into(),
                    },
                    expected,
                    new: Target::Object(new),
                },
                name: EVENTS_REF.try_into().expect("valid ref name"),
                deref: false,
            };
            match self.repo.edit_reference(edit) {
                Ok(_) => return Ok(()),
                Err(e) => last_err = Some(anyhow!(e)),
            }
        }
        Err(last_err
            .map(|e| e.context(format!("ref CAS failed after {MAX_CAS_RETRIES} attempts")))
            .unwrap_or_else(|| anyhow!("ref CAS failed")))
    }

    /// shard ("ab") -> filename ("cdef…") -> blob oid, from a ref commit.
    fn load_shards(&self, head: ObjectId) -> anyhow::Result<Shards> {
        let mut shards: Shards = BTreeMap::new();
        for (path, oid) in self.blob_entries(head)? {
            shards.entry(path.0).or_default().insert(path.1, oid);
        }
        Ok(shards)
    }

    fn blob_entries(&self, head: ObjectId) -> anyhow::Result<Vec<((String, String), ObjectId)>> {
        let commit = self.repo.find_object(head)?.try_into_commit()?;
        let tree = commit.tree()?;
        let mut out = Vec::new();
        for entry in tree.iter() {
            let entry = entry?;
            if !entry.mode().is_tree() {
                continue;
            }
            let shard = entry.filename().to_string();
            let sub = self.repo.find_object(entry.oid())?.try_into_tree()?;
            for blob in sub.iter() {
                let blob = blob?;
                if blob.mode().is_blob() {
                    out.push(((shard.clone(), blob.filename().to_string()), blob.oid().into()));
                }
            }
        }
        Ok(out)
    }

    fn write_shard_tree(&self, shards: Shards) -> anyhow::Result<ObjectId> {
        let mut root_entries = Vec::with_capacity(shards.len());
        for (shard, blobs) in shards {
            let mut entries: Vec<Entry> = blobs
                .into_iter()
                .map(|(name, oid)| Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.into(),
                    oid,
                })
                .collect();
            entries.sort();
            let sub = gix::objs::Tree { entries };
            let sub_id = self.repo.write_object(&sub)?.detach();
            root_entries.push(Entry {
                mode: EntryKind::Tree.into(),
                filename: shard.into(),
                oid: sub_id,
            });
        }
        root_entries.sort();
        Ok(self.repo.write_object(&gix::objs::Tree { entries: root_entries })?.detach())
    }

    fn write_ref_commit(
        &self,
        tree: ObjectId,
        parents: Vec<ObjectId>,
        message: &str,
    ) -> anyhow::Result<ObjectId> {
        let sig = gix::actor::Signature {
            name: "levi".into(),
            email: "levi@localhost".into(),
            time: gix::date::Time::now_utc(),
        };
        let commit = gix::objs::Commit {
            tree,
            parents: parents.into_iter().collect(),
            author: sig.clone(),
            committer: sig,
            encoding: None,
            message: message.into(),
            extra_headers: Vec::new(),
        };
        Ok(self.repo.write_object(&commit)?.detach())
    }
}

type Shards = BTreeMap<String, BTreeMap<String, ObjectId>>;

/// Insert a blob into the sharded layout; false if already present.
fn insert_blob(shards: &mut Shards, oid: ObjectId) -> bool {
    let hex = oid.to_string();
    let (shard, name) = hex.split_at(2);
    shards
        .entry(shard.to_string())
        .or_default()
        .insert(name.to_string(), oid)
        .is_none()
}

/// Guard: the events ref must never touch the working tree. (Nothing to do —
/// we only write objects and the ref — but keep the invariant test-visible.)
pub fn _invariant_no_worktree_writes() {}
