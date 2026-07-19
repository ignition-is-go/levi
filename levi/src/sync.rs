//! Sync legs (spec §Sync). The git leg lands in Task 10; hub + facts legs in
//! Task 12. `opportunistic` is the best-effort background sync every mutating
//! command attempts on exit.

use crate::ctx::LeviCtx;

/// Best-effort, silent. No hub configured (or `--no-sync`) ⇒ no-op.
pub fn opportunistic(_ctx: &LeviCtx) {
    // Filled in by the hub leg (Task 12).
}
