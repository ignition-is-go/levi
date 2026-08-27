//! `levi init` — set a repo up to use levi: adopt the project from the
//! remote when one exists (mint it otherwise), record the hub address, and
//! write task-tracking instructions for coding agents into CLAUDE.md /
//! AGENTS.md. Idempotent: re-running keeps the existing project and
//! replaces the instruction block in place (it's delimited by markers).
//! The old setup-command name survives as a hidden clap alias for muscle
//! memory (see `cli.rs`).

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use levi_core::Project;

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
- **CI**: `levi check-claims --git-ref <branch>` fails unless every task\n\
  claimed by that branch is closed in the tested HEAD ancestry.\n\
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
    ctx: &mut LeviCtx,
    name: Option<String>,
    hub: Option<String>,
    files: Vec<PathBuf>,
) -> Result<()> {
    // 0. --hub/--file need somewhere on disk to write to; fail before any
    // mutation rather than minting a project and then discovering there's
    // no worktree to record it in.
    if ctx.store.repo().workdir().is_none() && (hub.is_some() || !files.is_empty()) {
        bail!("levi init --hub/--file needs a worktree (not a bare repo)");
    }

    // 1. Project: reuse what's here, adopt from the remote, or mint.
    if let Some(p) = &ctx.world.project {
        println!("levi project already initialized: {} ({})", p.name, p.id.0);
    } else if !adopt_from_remote(ctx)? {
        let project = create_project(ctx, name)?;
        println!(
            "initialized levi project '{}' ({})",
            project.name, project.id.0
        );
    }

    let Some(root) = ctx.store.repo().workdir().map(|w| w.to_path_buf()) else {
        eprintln!("levi: bare repo — skipped agent instructions and hub recording");
        return Ok(());
    };

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
             `levi init --hub <host:port>` (writes .levi/config.toml)"
        );
    }
    Ok(())
}

/// Probe the configured git remote for an existing events ref and adopt it
/// rather than minting a fork of the project (spec: safe init).
/// Ok(true): events fetched, world reloaded, "joined" printed.
/// Ok(false): nothing to adopt — no remote, ref absent, or `--no-sync`.
/// Err: remote unreachable, or a fetch that failed after a successful
/// probe. Never falls through to minting on Err: minting when we know (or
/// cannot know) whether events exist is the fork footgun this prevents.
fn adopt_from_remote(ctx: &mut LeviCtx) -> Result<bool> {
    if ctx.no_sync {
        return Ok(false);
    }
    let repo_dir = ctx
        .store
        .repo()
        .workdir()
        .unwrap_or_else(|| ctx.store.repo().git_dir())
        .to_path_buf();
    let remote = ctx.config.remote.clone();
    let has_remote = Command::new("git")
        .args(["remote", "get-url", &remote])
        .current_dir(&repo_dir)
        .output()
        .context("running git")?;
    if !has_remote.status.success() {
        return Ok(false);
    }
    let probe = Command::new("git")
        .args([
            "ls-remote",
            "--exit-code",
            &remote,
            crate::store::EVENTS_REF,
        ])
        .current_dir(&repo_dir)
        .output()
        .context("running git ls-remote")?;
    match probe.status.code() {
        // --exit-code: 2 = remote reachable, no matching ref.
        Some(2) => Ok(false),
        Some(0) => {
            crate::sync::git_leg(ctx).context("remote has levi events but fetching them failed")?;
            ctx.reload()?;
            match &ctx.world.project {
                Some(p) => {
                    println!("joined existing levi project '{}' ({})", p.name, p.id.0);
                    Ok(true)
                }
                None => bail!(
                    "fetched events from '{remote}' but found no project event; \
                     refusing to mint a new project over them"
                ),
            }
        }
        _ => bail!(
            "cannot reach remote '{remote}' to check for an existing levi project: {}\n\
             retry when online, or pass --no-sync to initialize a standalone project",
            String::from_utf8_lossy(&probe.stderr).trim()
        ),
    }
}

/// Mint the project id and append the first event (spec: stored in the first
/// event on the ref). Callers must have checked no project exists yet.
pub fn create_project(ctx: &LeviCtx, name: Option<String>) -> Result<Project> {
    let name = name.unwrap_or_else(|| {
        ctx.store
            .repo()
            .workdir()
            .and_then(|w| w.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    });
    let project = Project {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        name,
        created: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&project)])?;
    Ok(project)
}
