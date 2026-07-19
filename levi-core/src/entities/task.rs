use myko::prelude::*;

#[myko_subtype(derive(Default, Eq, Hash, PartialOrd, Ord, Copy))]
pub enum Priority {
    P0,
    P1,
    #[default]
    P2,
    P3,
}

impl Priority {
    pub fn rank(self) -> u8 {
        match self {
            Priority::P0 => 0,
            Priority::P1 => 1,
            Priority::P2 => 2,
            Priority::P3 => 3,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "p0" | "0" => Some(Priority::P0),
            "p1" | "1" => Some(Priority::P1),
            "p2" | "2" => Some(Priority::P2),
            "p3" | "3" => Some(Priority::P3),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }
}

/// A tracked task. NOTE (spec): status is *not* a field — it is derived
/// per-checkout from [`crate::StatusChange`] records. Metadata edits are
/// ordinary myko SET events; last-writer-wins by event `created_at`, ties
/// broken by event id (levi sorts before replay — see `materialize`).
#[myko_item]
pub struct Task {
    pub project_id: String,
    #[searchable]
    pub title: String,
    #[serde(default)]
    #[searchable]
    pub body: String,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub labels: Vec<String>,
    pub created_by_dev: String,
    pub created_by_machine: String,
    /// RFC3339
    pub created_at: String,
}
