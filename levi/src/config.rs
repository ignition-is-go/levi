//! Configuration. Repo-level settings live in `<repo>/.levi/config.toml`
//! (committed with the repo, so onboarding is one clone away); user-global
//! fallback is `~/.config/levi/config.toml` (path overridable via
//! `LEVI_CONFIG`, used by tests).
//!
//! ```toml
//! [hub]
//! address = "hub.example.com:7377"
//!
//! [sync]
//! remote = "origin"
//!
//! [claim]
//! ttl_secs = 86400
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const REPO_CONFIG_PATH: &str = ".levi/config.toml";

#[derive(Debug, Clone, Default)]
pub struct LeviConfig {
    /// Hub address, e.g. "localhost:7377".
    pub hub: Option<String>,
    /// Git remote for the sync git leg (default "origin").
    pub remote: String,
    /// Claim ttl (default 24h).
    pub claim_ttl_secs: u64,
    /// Commits per head to compute patch-ids for when publishing facts.
    pub patch_id_window: usize,
}

impl LeviConfig {
    pub fn load(repo: &gix::Repository) -> Self {
        let repo_cfg = repo
            .workdir()
            .map(|w| FileConfig::load(w.join(REPO_CONFIG_PATH)))
            .unwrap_or_default();
        let global_cfg = global_config_path()
            .map(FileConfig::load)
            .unwrap_or_default();
        LeviConfig {
            hub: repo_cfg.hub.or(global_cfg.hub),
            remote: repo_cfg
                .remote
                .or(global_cfg.remote)
                .unwrap_or_else(|| "origin".into()),
            claim_ttl_secs: repo_cfg
                .claim_ttl_secs
                .or(global_cfg.claim_ttl_secs)
                .unwrap_or(24 * 60 * 60),
            patch_id_window: repo_cfg
                .patch_id_window
                .or(global_cfg.patch_id_window)
                .unwrap_or(300),
        }
    }
}

fn global_config_path() -> Option<PathBuf> {
    std::env::var_os("LEVI_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/levi/config.toml"))
        })
}

/// Set `[hub] address` in the repo's `.levi/config.toml`, preserving any
/// other keys already there.
pub fn write_repo_hub(workdir: &Path, hub: &str) -> Result<PathBuf> {
    let path = workdir.join(REPO_CONFIG_PATH);
    let mut doc = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let hub_table = doc
        .entry("hub")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(table) = hub_table.as_table_mut() {
        table.insert("address".into(), toml::Value::String(hub.to_string()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, toml::to_string_pretty(&doc)?)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

#[derive(Default)]
struct FileConfig {
    hub: Option<String>,
    remote: Option<String>,
    claim_ttl_secs: Option<u64>,
    patch_id_window: Option<usize>,
}

impl FileConfig {
    fn load(path: impl AsRef<Path>) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            return Self::default();
        };
        let str_at =
            |table: &str, key: &str| doc.get(table)?.get(key)?.as_str().map(str::to_string);
        FileConfig {
            hub: str_at("hub", "address"),
            remote: str_at("sync", "remote"),
            claim_ttl_secs: doc
                .get("claim")
                .and_then(|c| c.get("ttl_secs"))
                .and_then(|v| v.as_integer())
                .and_then(|v| u64::try_from(v).ok()),
            patch_id_window: doc
                .get("facts")
                .and_then(|c| c.get("patch_id_window"))
                .and_then(|v| v.as_integer())
                .and_then(|v| usize::try_from(v).ok()),
        }
    }
}
