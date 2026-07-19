//! Machine-local state (never config, never synced):
//! `~/.local/state/levi/` holds the minted machine id and the checkout
//! registry; `.git/levi/foreign-status.toml` caches hub-derived foreign
//! blocker statuses per repo. `LEVI_STATE_DIR` overrides the state dir
//! (tests, and CI where no real siblings exist).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LEVI_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state/levi"))
}

/// The machine's identity for claims: a UUID minted once and persisted.
/// Hostnames collide (default laptop names, container fleets); a levi-owned
/// random id doesn't, and a fresh container correctly gets a fresh identity.
/// Empty string when no state dir is available (identity falls back to the
/// hostname display name, matching legacy events).
pub fn machine_id() -> String {
    let Some(dir) = state_dir() else {
        return String::new();
    };
    let path = dir.join("machine-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let minted = uuid::Uuid::new_v4().simple().to_string();
    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, &minted).is_ok() {
        minted
    } else {
        String::new()
    }
}

/// Opportunistically record "this project has a checkout at this path" —
/// every invocation upserts, so any repo levi has ever run in is findable
/// as a sibling from every other repo on the machine.
pub fn register_checkout(project_id: &str, worktree: &Path) {
    let Some(dir) = state_dir() else { return };
    let path = dir.join("checkouts.toml");
    let mut doc = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let entry = doc
        .entry(project_id.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(table) = entry.as_table_mut() {
        table.insert(
            worktree.display().to_string(),
            toml::Value::String(crate::ctx::LeviCtx::now()),
        );
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(s) = toml::to_string_pretty(&doc) {
        let _ = std::fs::write(&path, s);
    }
}

/// The most recently used live checkout of `project_id`, excluding
/// `not_this` (the consuming repo itself). Prunes vanished paths.
pub fn sibling_checkout(project_id: &str, not_this: &Path) -> Option<PathBuf> {
    let dir = state_dir()?;
    let path = dir.join("checkouts.toml");
    let mut doc = std::fs::read_to_string(&path)
        .ok()?
        .parse::<toml::Table>()
        .ok()?;
    let table = doc.get_mut(project_id)?.as_table_mut()?;
    let mut candidates: Vec<(String, String)> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    for (p, used) in table.iter() {
        if Path::new(p).exists() {
            candidates.push((p.clone(), used.as_str().unwrap_or("").to_string()));
        } else {
            stale.push(p.clone());
        }
    }
    if !stale.is_empty() {
        for p in &stale {
            table.remove(p);
        }
        if let Ok(s) = toml::to_string_pretty(&doc) {
            let _ = std::fs::write(&path, s);
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1)); // most recently used first
    candidates
        .into_iter()
        .map(|(p, _)| PathBuf::from(p))
        .find(|p| p != not_this)
}

/// Per-repo cache of foreign blocker statuses, written only by sync.
#[derive(Debug, Clone, Default)]
pub struct ForeignStatus {
    pub status: String,     // "open" | "closed"
    pub resolution: String, // "facts"
    pub observed: String,   // RFC3339
    pub title: String,
}

pub struct ForeignStatusCache {
    path: PathBuf,
    entries: BTreeMap<String, ForeignStatus>, // key: "{project_id}/{task_id}"
}

impl ForeignStatusCache {
    pub fn load(repo: &gix::Repository) -> Self {
        let path = repo.common_dir().join("levi").join("foreign-status.toml");
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
            .map(|doc| {
                doc.into_iter()
                    .filter_map(|(k, v)| {
                        let t = v.as_table()?;
                        let get = |key: &str| {
                            t.get(key)
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string()
                        };
                        Some((
                            k,
                            ForeignStatus {
                                status: get("status"),
                                resolution: get("resolution"),
                                observed: get("observed"),
                                title: get("title"),
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn get(&self, key: &str) -> Option<&ForeignStatus> {
        self.entries.get(key)
    }

    pub fn put(&mut self, key: String, status: ForeignStatus) {
        self.entries.insert(key, status);
    }

    pub fn save(&self) -> Result<()> {
        let mut doc = toml::Table::new();
        for (k, v) in &self.entries {
            let mut t = toml::Table::new();
            t.insert("status".into(), toml::Value::String(v.status.clone()));
            t.insert(
                "resolution".into(),
                toml::Value::String(v.resolution.clone()),
            );
            t.insert("observed".into(), toml::Value::String(v.observed.clone()));
            t.insert("title".into(), toml::Value::String(v.title.clone()));
            doc.insert(k.clone(), toml::Value::Table(t));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, toml::to_string_pretty(&doc)?)
            .with_context(|| format!("cannot write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env vars are process-global and tests run in parallel: serialize.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_state() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: serialized by ENV_LOCK for the guard's lifetime.
        unsafe { std::env::set_var("LEVI_STATE_DIR", dir.path()) };
        (dir, guard)
    }

    #[test]
    fn machine_id_mints_once() {
        let (_dir, _guard) = temp_state();
        let a = machine_id();
        let b = machine_id();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn registry_upserts_and_prunes() {
        let (_dir, _guard) = temp_state();
        let live = tempfile::TempDir::new().unwrap();
        let gone = live.path().join("vanished");
        std::fs::create_dir_all(&gone).unwrap();
        register_checkout("proj1", &gone);
        register_checkout("proj1", live.path());
        std::fs::remove_dir_all(&gone).unwrap();
        let found = sibling_checkout("proj1", Path::new("/elsewhere")).unwrap();
        assert_eq!(found, live.path());
        // The consuming repo itself is excluded.
        assert!(sibling_checkout("proj1", live.path()).is_none());
    }
}
