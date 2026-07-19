//! Sync legs (spec §Sync). The git leg lives in `commands::sync_cmd`; the hub
//! and facts legs land with the hub client (Task 12). `opportunistic` is the
//! best-effort background sync every mutating command attempts on exit.

use anyhow::Result;

use crate::ctx::LeviCtx;

/// Hub leg: exchange event diffs with the configured hub. `Ok(None)` when no
/// hub is configured. Implemented in Task 12.
pub fn hub_leg(_ctx: &LeviCtx) -> Result<Option<String>> {
    Ok(None)
}

/// Best-effort, silent. No hub configured (or `--no-sync`) ⇒ no-op.
pub fn opportunistic(ctx: &LeviCtx) {
    let _ = hub_leg(ctx);
}
