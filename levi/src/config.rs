//! Configuration: per-repo `git config levi.*` overrides
//! `~/.config/levi/config.toml` (path overridable via `LEVI_CONFIG` for
//! tests). Spec §Sync.

use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct LeviConfig {
    /// Hub address, e.g. "localhost:7377" (`git config levi.hub`).
    pub hub: Option<String>,
    /// Git remote for the sync git leg (`git config levi.remote`, default "origin").
    pub remote: String,
    /// Claim ttl (`git config levi.claimTtlSecs`, default 24h).
    pub claim_ttl_secs: u64,
}

impl LeviConfig {
    pub fn load(repo: &gix::Repository) -> Self {
        let file = FileConfig::load();
        let snap = repo.config_snapshot();
        let get = |key: &str| snap.string(key).map(|v| v.to_string());
        LeviConfig {
            hub: get("levi.hub").or(file.hub),
            remote: get("levi.remote")
                .or(file.remote)
                .unwrap_or_else(|| "origin".into()),
            claim_ttl_secs: get("levi.claimTtlSecs")
                .and_then(|v| v.parse().ok())
                .or(file.claim_ttl_secs)
                .unwrap_or(24 * 60 * 60),
        }
    }
}

#[derive(Default)]
struct FileConfig {
    hub: Option<String>,
    remote: Option<String>,
    claim_ttl_secs: Option<u64>,
}

impl FileConfig {
    fn load() -> Self {
        let path = std::env::var_os("LEVI_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/levi/config.toml"))
            });
        let Some(path) = path else {
            return Self::default();
        };
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
        }
    }
}
