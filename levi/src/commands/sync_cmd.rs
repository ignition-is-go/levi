//! `levi sync` — three independent, best-effort legs (spec §Sync):
//! 1. git leg: fetch `refs/levi/events` from the remote, union-merge, push.
//! 2. hub leg: exchange event diffs with the hub.
//! 3. facts leg: publish CommitFacts/RefFacts to the hub.

use anyhow::{Result, bail};

use crate::ctx::LeviCtx;

pub fn run(ctx: &mut LeviCtx, no_git: bool, no_hub: bool) -> Result<()> {
    let mut failures = Vec::new();

    if !no_git {
        match crate::sync::git_leg(ctx) {
            Ok(summary) => println!("git: {summary}"),
            Err(e) => failures.push(format!("git leg: {e:#}")),
        }
    }
    if !no_hub {
        match crate::sync::hub_leg(ctx) {
            Ok(Some(summary)) => println!("hub: {summary}"),
            Ok(None) => println!("hub: not configured (run `levi init --hub <host:port>`)"),
            Err(e) => failures.push(format!("hub leg: {e:#}")),
        }
    }
    ctx.reload()?;
    if !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    Ok(())
}
