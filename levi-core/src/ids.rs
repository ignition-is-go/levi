//! Short task ids: display as `lv-` + a unique hex prefix (git-style),
//! prefix matching on input.

use crate::materialize::World;

const MIN_SHORT: usize = 4;

/// Shortest unique-prefix display id for `id` within `world` (min 4 hex
/// chars). Falls back to lengthening on collision, git-style.
pub fn short_id(world: &World, id: &str) -> String {
    let mut len = MIN_SHORT;
    while len < id.len() {
        let prefix = &id[..len];
        let collisions = world
            .tasks
            .keys()
            .filter(|other| other.as_str() != id && other.starts_with(prefix))
            .count();
        if collisions == 0 {
            return format!("lv-{prefix}");
        }
        len += 2;
    }
    format!("lv-{id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixError {
    NotFound(String),
    Ambiguous(String, Vec<String>),
}

impl std::fmt::Display for PrefixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrefixError::NotFound(p) => write!(f, "no task matches '{p}'"),
            PrefixError::Ambiguous(p, ids) => {
                write!(f, "'{p}' is ambiguous: matches {}", ids.join(", "))
            }
        }
    }
}

impl std::error::Error for PrefixError {}

/// Resolve user input (`lv-3f2a`, `3f2a`, or a full id) to a task id.
pub fn resolve_prefix<'w>(world: &'w World, input: &str) -> Result<&'w str, PrefixError> {
    let prefix = input.strip_prefix("lv-").unwrap_or(input);
    let mut matches: Vec<&str> = world
        .tasks
        .keys()
        .filter(|id| id.starts_with(prefix))
        .map(String::as_str)
        .collect();
    match matches.len() {
        0 => Err(PrefixError::NotFound(input.to_string())),
        1 => Ok(matches.remove(0)),
        _ => Err(PrefixError::Ambiguous(
            input.to_string(),
            matches.iter().map(|id| short_id_of(id)).collect(),
        )),
    }
}

fn short_id_of(id: &str) -> String {
    format!("lv-{}", &id[..id.len().min(8)])
}
