//! `levi check-events` — a tamper check on `refs/levi/events`.
//!
//! Levi only ever adds events, and merges by union, so a commit of the events
//! ref saved at one time stays in the ref's history from then on. `levi
//! check-events` prints the ref's current head, to save somewhere the ref's
//! writers can't reach; `levi check-events --expect SHA` later fails unless
//! that commit is still the head or one of its ancestors, and every event it
//! held is still in the head's tree. So a ref that was reset, rewritten, or
//! had events removed fails. It can't catch events added as someone else:
//! anyone with push access to the ref can write any event.
//!
//! It doesn't change the events ref, and reads it as it stands, never syncing
//! first, so what it checks is what this checkout holds. Like every levi
//! command it needs git `user.email` set.

use anyhow::{Result, bail};
use gix::ObjectId;
use serde_json::json;

use crate::ctx::LeviCtx;
use crate::output::SCHEMA_CHECK_EVENTS;
use crate::store::{EVENTS_REF, EventStore};

/// Why the events ref no longer holds an expected commit and all its events,
/// or `None` if it does.
fn fault(store: &EventStore, head: Option<ObjectId>, expected: ObjectId) -> Option<String> {
    let repo = store.repo();
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
        Ok(base) if base.detach() == expected => {}
        _ => {
            return Some(format!(
                "{expected} is no longer in {EVENTS_REF}'s history: the ref was reset or rewritten"
            ));
        }
    }
    // Ancestry alone isn't enough: a commit on top of the saved head can
    // leave events out of its tree, and only the head's tree is read.
    match store.missing_events(expected, head) {
        Ok(0) => None,
        Ok(n) => Some(format!(
            "{n} event(s) in {expected} are missing from {head}: the ref was rewritten"
        )),
        Err(e) => Some(format!("can't compare {expected} with {head}: {e:#}")),
    }
}

pub fn run(ctx: &LeviCtx, expect: Option<String>, json_output: bool) -> Result<()> {
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
    let fault = fault(&ctx.store, head, expected);
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
