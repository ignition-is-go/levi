//! Requested reads: `request-read` and `withdraw-read`, answered by a later
//! review, with `closed_before_read` and the `--require-review` gate.

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;

fn other_worktree(repo: &TestRepo, branch: &str) -> PathBuf {
    let name = format!(
        "{}-{branch}-wt",
        repo.path().file_name().unwrap().to_string_lossy()
    );
    let wt = repo.path().join("..").join(name);
    let wt_str = wt.to_string_lossy().into_owned();
    repo.git(&["worktree", "add", "-q", &wt_str, "-b", branch]);
    wt.canonicalize().unwrap()
}

fn ok_in(repo: &TestRepo, cwd: &Path, args: &[&str]) -> String {
    let out = repo.levi_in(cwd.to_path_buf(), args).output().unwrap();
    assert!(
        out.status.success(),
        "levi {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn requests(repo: &TestRepo, id: &str) -> Vec<Value> {
    repo.levi_json(&["show", id, "--json"])["read_requests"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_request_stays_open_until_a_review_answers_it() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("please read", &[]);
    let reviewer = other_worktree(&repo, "reviewer");
    let head = repo.commit("work");

    let out = repo.levi_ok(&["request-read", &id]);
    assert!(out.contains("read requested"), "{out}");
    let reqs = requests(&repo, &id);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["commit"], head.as_str());
    assert_eq!(reqs[0]["open"], true);
    assert_eq!(reqs[0]["source"], "declared");
    assert!(reqs[0]["answered_by"].is_null());

    // Asking again replaces the request rather than adding one.
    let newer = repo.commit("more work");
    repo.levi_ok(&["request-read", &id, "--commit", &newer]);
    let reqs = requests(&repo, &id);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["commit"], newer.as_str());

    ok_in(
        &repo,
        &reviewer,
        &["send-back", &id, "--commit", &newer, "-m", "tests?"],
    );
    let reqs = requests(&repo, &id);
    assert_eq!(reqs[0]["open"], false);
    assert_eq!(reqs[0]["answered_by"]["verdict"], "sent_back");

    let text = repo.levi_ok(&["show", &id]);
    assert!(text.contains("read requests (declared):"), "{text}");
    assert!(text.contains("answered: sent_back"), "{text}");
}

#[test]
fn withdraw_removes_only_your_own_request() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("changed my mind", &[]);
    let other = other_worktree(&repo, "other");
    repo.commit("work");

    repo.levi_ok(&["request-read", &id]);
    // Someone in another worktree has no request to withdraw.
    repo.levi_in(other.clone(), &["withdraw-read", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no read request"));
    assert_eq!(requests(&repo, &id).len(), 1);

    let out = repo.levi_ok(&["withdraw-read", &id]);
    assert!(out.contains("withdrawn"), "{out}");
    assert!(requests(&repo, &id).is_empty());
    repo.levi(&["withdraw-read", &id]).assert().failure();
}

#[test]
fn a_close_before_the_read_is_marked() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("merged too soon", &[]);
    let reviewer = other_worktree(&repo, "reviewer");
    repo.commit("work");
    repo.levi_ok(&["request-read", &id]);
    repo.levi_ok(&["close", &id]);

    let reqs = requests(&repo, &id);
    assert_eq!(reqs[0]["open"], true);
    assert_eq!(reqs[0]["closed_before_read"], true);
    let text = repo.levi_ok(&["show", &id]);
    assert!(text.contains("closed here before it was read"), "{text}");

    // The late read answers the request, and the mark stays.
    ok_in(&repo, &reviewer, &["verify", &id, "--commit", "main"]);
    let reqs = requests(&repo, &id);
    assert_eq!(reqs[0]["open"], false);
    assert_eq!(reqs[0]["closed_before_read"], true);
}

#[test]
fn an_open_request_fails_the_review_gate() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("gated on a read", &[]);
    let reviewer = other_worktree(&repo, "reviewer");

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.levi_ok(&["start", &id]);
    let fix = repo.commit("the fix");
    repo.levi_ok(&["close", &id]);
    ok_in(&repo, &reviewer, &["verify", &id, "--commit", &fix]);
    repo.levi_ok(&["check-claims", "--git-ref", "feature", "--require-review"]);

    // A second read is asked for after the verification: the gate holds
    // the merge until it's answered.
    repo.levi_ok(&["request-read", &id]);
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
    assert_eq!(result["tasks"][0]["review"], "verified");
    assert_eq!(result["tasks"][0]["open_reads"], 1);

    ok_in(&repo, &reviewer, &["verify", &id, "--commit", &fix]);
    let result = repo.levi_json(&[
        "check-claims",
        "--git-ref",
        "feature",
        "--require-review",
        "--json",
    ]);
    assert_eq!(result["ok"], true);
    assert_eq!(result["tasks"][0]["open_reads"], 0);
}
