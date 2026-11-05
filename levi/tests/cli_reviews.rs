//! Reviews: `verify` and `send-back` at a commit, resolved by ancestry, with
//! the self-review rule and the `check-claims --require-review` gate.

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;

/// A second worktree of the same repo: same dev and machine, different
/// worktree, so a different reviewer by levi's identity.
fn reviewer_worktree(repo: &TestRepo, branch: &str) -> PathBuf {
    let name = format!(
        "{}-review-wt",
        repo.path().file_name().unwrap().to_string_lossy()
    );
    let wt = repo.path().join("..").join(name);
    let wt_str = wt.to_string_lossy().into_owned();
    repo.git(&["worktree", "add", "-q", &wt_str, "-b", branch]);
    wt.canonicalize().unwrap()
}

fn ok_in(repo: &TestRepo, cwd: &Path, args: &[&str]) -> (String, String) {
    let out = repo.levi_in(cwd.to_path_buf(), args).output().unwrap();
    assert!(
        out.status.success(),
        "levi {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn task<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(id))
        .expect("task listed")
}

#[test]
fn verification_on_a_branch_applies_on_main_once_merged() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("reviewed work", &[]);
    let wt = reviewer_worktree(&repo, "reviewer");

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.levi_ok(&["start", &id]);
    let fix = repo.commit("the fix");
    repo.levi_ok(&["close", &id]);

    let (out, _) = ok_in(
        &repo,
        &wt,
        &["verify", &id, "--commit", &fix, "-m", "ran it"],
    );
    assert!(out.contains("verified @"), "{out}");

    // On feature: verified, declared, and read at the commit that closed it.
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    let t = task(&ls, &id);
    assert_eq!(t["review"], "verified");
    assert_eq!(t["review_source"], "declared");
    assert_eq!(t["review_resolution"], "exact");
    assert!(t["review_differs"].is_null());

    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["review"], "verified");
    assert_eq!(show["reviews"][0]["verdict"], "verified");
    assert_eq!(show["reviews"][0]["commit"], fix.as_str());
    assert_eq!(show["reviews"][0]["note"], "ran it");
    assert_eq!(show["reviews"][0]["applies_here"], "yes");
    assert_eq!(show["reviews"][0]["source"], "declared");
    assert_eq!(
        show["reviews"][0]["worktree"],
        wt.to_string_lossy().as_ref()
    );

    // On main the read commit isn't in ancestry yet.
    repo.checkout("main");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(task(&ls, &id)["review"], "none");
    assert!(task(&ls, &id)["review_source"].is_null());

    repo.merge("feature");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(task(&ls, &id)["review"], "verified");
}

#[test]
fn the_claimant_cannot_review_their_own_task() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("mine", &[]);
    repo.levi_ok(&["start", &id]);
    repo.commit("work");

    repo.levi(&["verify", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("can't review it"))
        .stderr(predicate::str::contains("not impostors"));
    // Dropping the claim doesn't make you a second reader.
    repo.levi_ok(&["drop", &id]);
    repo.levi(&["send-back", &id, "-m", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("can't review it"));

    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["reviews"].as_array().unwrap().len(), 0);
}

#[test]
fn a_closer_who_never_claimed_gets_a_warning_not_a_refusal() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("closed without a claim", &[]);
    repo.commit("work");
    repo.levi_ok(&["close", &id]);
    let wt = reviewer_worktree(&repo, "reviewer");

    let (_, err) = ok_in(&repo, &wt, &["verify", &id, "--commit", "main"]);
    assert!(
        err.contains("reviewer matches the closer's dev and machine"),
        "{err}"
    );
    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["reviews"][0]["self_review_warning"], true);

    // The closer in their own worktree has no claim to tell them apart,
    // so they're warned too, not refused: levi can't know more.
    let (_, err) = ok_in(&repo, repo.path(), &["verify", &id]);
    assert!(err.contains("warning:"), "{err}");
}

#[test]
fn send_back_needs_a_note_and_puts_the_task_back_in_next() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("needs tests", &["-p", "p1"]);
    let wt = reviewer_worktree(&repo, "reviewer");
    repo.commit("first try");

    repo.levi_in(wt.clone(), &["send-back", &id, "--commit", "main"])
        .assert()
        .failure();
    repo.levi_in(
        wt.clone(),
        &["send-back", &id, "--commit", "main", "-m", "  "],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("saying what's missing"));

    ok_in(
        &repo,
        &wt,
        &["send-back", &id, "--commit", "main", "-m", "no tests"],
    );
    let next = repo.levi_json(&["next", "--json"]);
    let t = task(&next, &id);
    assert_eq!(t["review"], "sent_back");
    assert!(
        t["reason"]
            .as_str()
            .unwrap()
            .contains("sent back: no tests"),
        "{}",
        t["reason"]
    );

    // A later verification supersedes it.
    ok_in(&repo, &wt, &["verify", &id, "--commit", "main"]);
    let next = repo.levi_json(&["next", "--json"]);
    let t = task(&next, &id);
    assert_eq!(t["review"], "verified");
    assert!(!t["reason"].as_str().unwrap().contains("sent back"));
}

#[test]
fn review_differs_when_the_task_closed_at_another_commit() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("moving head", &[]);
    let wt = reviewer_worktree(&repo, "reviewer");
    let read = repo.commit("what was read");
    ok_in(&repo, &wt, &["verify", &id, "--commit", &read]);
    let closed = repo.commit("one more change after the read");
    repo.levi_ok(&["close", &id]);

    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["review"], "verified");
    assert_eq!(show["review_differs"]["verified"], read.as_str());
    assert_eq!(show["review_differs"]["closed"], closed.as_str());
    assert_eq!(show["review_differs"]["source"], "observed");

    let text = repo.levi_ok(&["show", &id]);
    assert!(text.contains("reviews (declared):"), "{text}");
    assert!(text.contains("but closed here at"), "{text}");
}

#[test]
fn check_claims_require_review_gates_on_a_verified_read() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("gated", &[]);
    let wt = reviewer_worktree(&repo, "reviewer");

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.levi_ok(&["start", &id]);
    let fix = repo.commit("the fix");
    repo.levi_ok(&["close", &id]);

    // Closed is enough without the flag.
    repo.levi_ok(&["check-claims", "--git-ref", "feature"]);

    // With it, the gate fails until someone else verifies.
    let out = repo
        .levi(&[
            "check-claims",
            "--git-ref",
            "feature",
            "--require-review",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["tasks"][0]["status"], "closed");
    assert_eq!(result["tasks"][0]["review"], "none");
    repo.levi(&["check-claims", "--git-ref", "feature", "--require-review"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not closed and verified"));

    // A send-back doesn't pass it.
    ok_in(
        &repo,
        &wt,
        &["send-back", &id, "--commit", &fix, "-m", "missing docs"],
    );
    repo.levi(&["check-claims", "--git-ref", "feature", "--require-review"])
        .assert()
        .failure();

    ok_in(&repo, &wt, &["verify", &id, "--commit", &fix]);
    let result = repo.levi_json(&[
        "check-claims",
        "--git-ref",
        "feature",
        "--require-review",
        "--json",
    ]);
    assert_eq!(result["ok"], true);
    assert_eq!(result["tasks"][0]["review"], "verified");
    assert_eq!(result["tasks"][0]["review_source"], "declared");
}

#[test]
fn require_review_fails_where_the_close_holds_but_the_read_does_not() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("read after the close", &[]);
    let wt = reviewer_worktree(&repo, "reviewer");

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.levi_ok(&["start", &id]);
    let fix = repo.commit("the fix");
    repo.levi_ok(&["close", &id]);
    let later = repo.commit("a follow-up, read by the reviewer");
    ok_in(&repo, &wt, &["verify", &id, "--commit", &later]);

    // At the fix the task is closed, but the read commit isn't in ancestry,
    // so only the review condition fails.
    let out = repo
        .levi(&[
            "check-claims",
            "--git-ref",
            "feature",
            "--require-review",
            "--at",
            &fix,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["tasks"][0]["status"], "closed");
    assert_eq!(result["tasks"][0]["review"], "none");

    // At the read commit both hold.
    let result = repo.levi_json(&[
        "check-claims",
        "--git-ref",
        "feature",
        "--require-review",
        "--at",
        &later,
        "--json",
    ]);
    assert_eq!(result["ok"], true);
    assert_eq!(
        result["tasks"][0]["review_differs"]["verified"],
        later.as_str()
    );
    assert_eq!(result["tasks"][0]["review_differs"]["closed"], fix.as_str());
}
