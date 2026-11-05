//! `levi check-events` — a tamper check on `refs/levi/events`.
//!
//! Levi only ever adds events, and merges by union, so a commit of the events
//! ref saved at one time stays in the ref's history from then on. `levi
//! check-events` prints the ref's current head, to save somewhere the ref's
//! writers can't reach; `levi check-events --expect SHA` later fails unless
//! that commit is still the head or one of its ancestors. A ref that was reset
//! or rewritten fails. It can't catch events added as someone else: anyone
//! with push access to the ref can write any event.
//!
//! It reads the local ref as it stands, never syncing first, so what it checks
//! is what this checkout holds.

use anyhow::{Result, bail};
use gix::ObjectId;
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::SCHEMA_CHECK_EVENTS;
use crate::store::EVENTS_REF;

/// Why an expected commit isn't in the events ref's history, or `None` if it is.
fn fault(repo: &gix::Repository, head: Option<ObjectId>, expected: ObjectId) -> Option<String> {
    let Some(head) = head else {
        return Some(format!("{EVENTS_REF} is absent here"));
    };
    if head == expected {
        return None;
    }
    match repo.try_find_header(expected).ok().flatten() {
        None => {
            return Some(format!(
                "{expected} isn't in this repository: fetch {EVENTS_REF} first, or it was lost"
            ));
        }
        Some(header) if header.kind() != gix::object::Kind::Commit => {
            return Some(format!(
                "{expected} is a {}, not a commit: pass a head printed by levi check-events",
                header.kind()
            ));
        }
        Some(_) => {}
    }
    match repo.merge_base(head, expected) {
        Ok(base) if base.detach() == expected => None,
        _ => Some(format!(
            "{expected} is no longer in {EVENTS_REF}'s history: the ref was reset or rewritten"
        )),
    }
}

pub fn run(ctx: &LeviCtx, expect: Option<String>, json_output: bool) -> Result<()> {
    let repo = ctx.store.repo();
    let head = ctx.store.head()?;
    let Some(expect) = expect else {
        // No expectation: say where the ref is, so it can be saved.
        let Some(head) = head else {
            bail!("{EVENTS_REF} is absent here, so there's nothing to save");
        };
        if json_output {
            println!(
                "{}",
                json!({"schema": SCHEMA_CHECK_EVENTS, "head": head.to_string(), "ok": true})
            );
        } else {
            println!("{head}");
        }
        return Ok(());
    };
    let expected = match ObjectId::from_hex(expect.trim().as_bytes()) {
        Ok(id) => id,
        Err(_) => bail!("--expect takes a full commit hash, not {expect:?}"),
    };
    let fault = fault(repo, head, expected);
    if json_output {
        println!(
            "{}",
            json!({
                "schema": SCHEMA_CHECK_EVENTS,
                "head": head.map(|head| head.to_string()),
                "expected": expected.to_string(),
                "ok": fault.is_none(),
                "reason": fault,
            })
        );
    } else if fault.is_none() {
        println!("{EVENTS_REF} still contains {expected}");
    }
    if let Some(fault) = fault {
        bail!("{fault}");
    }
    Ok(())
}
