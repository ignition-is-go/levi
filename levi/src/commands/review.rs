//! `levi verify` / `levi send-back` — record a review of a task at a commit.
//!
//! A review resolves by ancestry, like a close: it applies on a checkout when
//! the commit it read is an ancestor of HEAD. Reviewing a task you claimed or
//! closed is refused, matched on (dev, machine, worktree). levi's identity is
//! self-declared, so this catches mistakes, not impostors.

use anyhow::{Context, Result, bail};
use levi_core::ids::{resolve_prefix, short_id};
use levi_core::rank::{SelfReview, is_self_review};
use levi_core::{Review, ReviewVerdict};

use crate::ctx::LeviCtx;

pub const SELF_REVIEW_WARNING: &str =
    "reviewer matches the closer's dev and machine; if you're a different session, this is fine";

pub fn run(
    ctx: &LeviCtx,
    id_input: &str,
    verdict: ReviewVerdict,
    commit: Option<&str>,
    note: &str,
) -> Result<()> {
    let project_id = ctx.project_id()?;
    let task_id = resolve_prefix(&ctx.world, id_input)?.to_string();
    let short = short_id(&ctx.world, &task_id);

    if verdict == ReviewVerdict::SentBack && note.trim().is_empty() {
        bail!("send-back needs a note saying what's missing (-m)");
    }

    let self_review = is_self_review(&ctx.world, &task_id, &ctx.identity);
    if let Some(who) = self_review
        && who.refuses()
    {
        let role = match who {
            SelfReview::Claimant => "hold, or last held, the claim on",
            _ => "closed",
        };
        bail!(
            "you {role} {short}, so you can't review it; a review needs another worktree or machine \
             (levi's identity is self-declared: this catches mistakes, not impostors)"
        );
    }
    let self_review_warning = matches!(self_review, Some(SelfReview::CloserHostOnly));

    let commit = match commit {
        Some(spec) => ctx.rev(spec)?.to_string(),
        None => ctx
            .head_commit()
            .context("HEAD has no commits; name the commit you read with --commit")?
            .to_string(),
    };

    let review = Review {
        id: uuid::Uuid::new_v4().simple().to_string().into(),
        project_id,
        task_id: task_id.clone(),
        verdict,
        commit: commit.clone(),
        note: note.to_string(),
        by_dev: ctx.identity.dev.clone(),
        by_machine: ctx.identity.machine.clone(),
        by_machine_id: ctx.identity.machine_id.clone(),
        by_worktree: ctx.identity.worktree.clone(),
        self_review_warning,
        created: LeviCtx::now(),
    };
    ctx.append_and_sync(vec![ctx.set_event(&review)])?;

    if self_review_warning {
        eprintln!("warning: {SELF_REVIEW_WARNING}");
    }
    let verb = match verdict {
        ReviewVerdict::Verified => "verified",
        ReviewVerdict::SentBack => "sent back",
    };
    println!("{short} {verb} @ {}", &commit[..8]);
    Ok(())
}
