//! `levi onboard` — set a repo up to use levi: initialize the project (if
//! needed) and write task-tracking instructions for coding agents into
//! CLAUDE.md / AGENTS.md. Idempotent: the instruction block is delimited by
//! markers and replaced in place on re-runs.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::ctx::LeviCtx;

const BEGIN: &str = "<!-- levi:begin -->";
const END: &str = "<!-- levi:end -->";

fn instructions() -> String {
    format!(
        "{BEGIN}\n\
## Task tracking (levi)\n\
\n\
This repo tracks tasks with levi, a git-aware issue tracker. State lives in\n\
the repo itself (`refs/levi/events`); status is resolved against git\n\
ancestry, so a task closed at commit X counts as closed only on checkouts\n\
that contain X. Every read command takes `--json` (stable schemas) — prefer\n\
it when parsing.\n\
\n\
- **Pick work**: `levi next --claim --json` returns the most important\n\
  eligible task, claims it for this dev/machine/worktree (so parallel agents\n\
  never grab the same task), and tells you why it ranked first. If you stop\n\
  working on a task, release it: `levi drop <id>`.\n\
- **Inspect**: `levi ls --json` (open on this checkout), `levi show <id>\n\
  --json` (body, deps, claim, comments, status history).\n\
- **Create**: `levi add \"title\" [-p p0..p3] [-b body] [-l label]\n\
  [--dep <blocker-id>]` — file follow-ups you discover instead of fixing\n\
  drive-by; link blockers with `--dep`/`levi dep add`.\n\
- **Complete**: commit the work first, then `levi close <id>` — the close\n\
  anchors at HEAD, so it only applies where the fixing commit exists\n\
  (feature-branch closes stay open on main until merged; that is correct).\n\
  `--no-anchor` is only for tasks unrelated to code state.\n\
- **Reopen** regressions with `levi reopen <id>`; leave context with\n\
  `levi comment <id> \"text\"`.\n\
- Sync is opportunistic after every mutation; `levi sync` forces a full\n\
  git-remote + hub exchange.\n\
- **Cross-project**: file upstream bugs with `levi add --project <name>\n\
  \"title\"`; link with `levi dep add <id> --on <project>/lv-xxxx --via\n\
  \"<how this repo consumes that project>\"`. When a foreign blocker\n\
  closes, verify the fix is actually reachable through the `via`\n\
  mechanism (published release, updated pin, ...) before starting work.\n\
{END}"
    )
}

pub fn run(
    ctx: &LeviCtx,
    name: Option<String>,
    hub: Option<String>,
    files: Vec<PathBuf>,
) -> Result<()> {
    // 1. Project: init if this repo has none yet.
    match &ctx.world.project {
        Some(p) => println!("levi project already initialized: {} ({})", p.name, p.id.0),
        None => {
            let project = super::init::create_project(ctx, name)?;
            println!(
                "initialized levi project '{}' ({})",
                project.name, project.id.0
            );
        }
    }

    let root = ctx
        .store
        .repo()
        .workdir()
        .context("levi onboard needs a worktree (not a bare repo)")?
        .to_path_buf();

    // 2. Hub address: recorded in .levi/config.toml so it travels with the
    //    repo (committed, one clone away from working).
    if let Some(hub) = &hub {
        let path = crate::config::write_repo_hub(&root, hub)?;
        println!("hub {} recorded in {}", hub, path.display());
    }

    // 3. Agent instructions: explicit --file targets win; otherwise update
    //    every existing CLAUDE.md/AGENTS.md, creating AGENTS.md if neither
    //    exists.
    let targets: Vec<PathBuf> = if files.is_empty() {
        let existing: Vec<PathBuf> = ["CLAUDE.md", "AGENTS.md"]
            .iter()
            .map(|f| root.join(f))
            .filter(|p| p.exists())
            .collect();
        if existing.is_empty() {
            vec![root.join("AGENTS.md")]
        } else {
            existing
        }
    } else {
        files
            .into_iter()
            .map(|f| if f.is_absolute() { f } else { root.join(f) })
            .collect()
    };

    let block = instructions();
    for path in &targets {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        let updated = match (current.find(BEGIN), current.find(END)) {
            // Replace the existing block in place.
            (Some(start), Some(end)) if end > start => {
                let mut s = String::with_capacity(current.len());
                s.push_str(&current[..start]);
                s.push_str(&block);
                s.push_str(&current[end + END.len()..]);
                s
            }
            // Corrupted markers: never guess at what to keep — appending a
            // fresh block would leave the stale partial one in place.
            (Some(_), _) | (None, Some(_)) => anyhow::bail!(
                "{} has a broken levi instruction block ({BEGIN} / {END} \
                 mismatched); fix or remove the markers and re-run",
                path.display()
            ),
            _ if current.is_empty() => format!("{block}\n"),
            _ => {
                let mut s = current.clone();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
                s.push_str(&block);
                s.push('\n');
                s
            }
        };
        std::fs::write(path, updated)
            .with_context(|| format!("cannot write {}", path.display()))?;
        println!("wrote task-tracking instructions to {}", path.display());
    }

    if hub.is_none() && ctx.config.hub.is_none() {
        println!(
            "tip: point this repo at a hub for cross-machine sync and dashboards: \
             `levi onboard --hub <host:port>` (writes .levi/config.toml)"
        );
    }
    Ok(())
}
