mod common;

use common::TestRepo;
use predicates::prelude::*;

fn failed_json(repo: &TestRepo, args: &[&str]) -> serde_json::Value {
    let out = repo.levi(args).output().unwrap();
    assert!(
        !out.status.success(),
        "command unexpectedly passed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).expect("failure still emits valid JSON")
}

#[test]
fn check_claims_fails_when_levi_events_are_absent() {
    let repo = TestRepo::new();
    repo.levi(&["check-claims", "--git-ref", "main", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no levi project"));
}

#[test]
fn claims_record_the_current_symbolic_branch() {
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["checkout", "-q", "-b", "feature/close-it"]);
    let id = repo.add("close me", &[]);

    repo.levi_ok(&["start", &id]);
    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["claim"]["git_ref"], "refs/heads/feature/close-it");

    repo.levi_ok(&["drop", &id]);
    repo.levi_ok(&["next", "--claim", "--json"]);
    let show = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["claim"]["git_ref"], "refs/heads/feature/close-it");
}

#[test]
fn claiming_from_detached_head_is_rejected_without_mutating() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("detached", &[]);
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);

    repo.levi(&["start", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("detached HEAD"));
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_null());

    repo.levi(&["next", "--claim"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("detached HEAD"));
}

#[test]
fn check_claims_uses_durable_claim_history_and_head_ancestry() {
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["checkout", "-q", "-b", "feature/ci"]);
    let closed = repo.add("done", &[]);
    let open = repo.add("not done", &[]);

    repo.levi_ok(&["start", &closed]);
    repo.commit("finish first task");
    repo.levi_ok(&["close", &closed]); // normal close deletes the live claim
    repo.levi_ok(&["start", &open]);
    repo.levi_ok(&["start", &open]); // repeated claims must remain one obligation
    repo.levi_ok(&["drop", &open]); // dropping must not erase branch responsibility

    let result = failed_json(
        &repo,
        &["check-claims", "--git-ref", "feature/ci", "--json"],
    );
    assert_eq!(result["schema"], "levi.check-claims/1");
    assert_eq!(result["git_ref"], "refs/heads/feature/ci");
    assert_eq!(result["tested_commit"].as_str().unwrap().len(), 40);
    assert_eq!(result["ok"], false);
    let tasks = result["tasks"].as_array().unwrap();
    assert_eq!(
        tasks.len(),
        2,
        "repeated/live claim state must be deduplicated"
    );
    assert_eq!(tasks.iter().filter(|t| t["status"] == "open").count(), 1);
    assert_eq!(
        tasks.iter().find(|t| t["id"] == open).unwrap()["status"],
        "open"
    );

    repo.commit("finish second task");
    repo.levi_ok(&["close", &open]);
    let result = repo.levi_json(&["check-claims", "--json"]);
    assert_eq!(result["ok"], true);
    assert_eq!(result["tasks"].as_array().unwrap().len(), 2);
}

#[test]
fn check_claims_selects_claim_branch_but_resolves_against_tested_head() {
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["checkout", "-q", "-b", "feature/owner"]);
    let id = repo.add("branch-owned", &[]);
    repo.levi_ok(&["start", &id]);
    repo.levi_ok(&["drop", &id]);

    // A close anchored only on another branch must not satisfy the owner
    // branch until that commit is merged into the tested HEAD.
    repo.git(&["checkout", "-q", "-b", "side-close", "main"]);
    repo.commit("unrelated branch fix");
    repo.levi_ok(&["close", &id]);
    repo.checkout("feature/owner");
    let result = failed_json(
        &repo,
        &["check-claims", "--git-ref", "feature/owner", "--json"],
    );
    assert_eq!(result["tasks"][0]["status"], "open");

    // A different branch with no claims is independent and passes empty.
    let empty = repo.levi_json(&["check-claims", "--git-ref", "feature/owner-2", "--json"]);
    assert_eq!(empty["ok"], true);
    assert_eq!(empty["tasks"].as_array().unwrap().len(), 0);

    repo.merge("side-close");
    let result = repo.levi_json(&["check-claims", "--git-ref", "feature/owner", "--json"]);
    assert_eq!(result["tasks"][0]["status"], "closed");
}

#[test]
fn check_claims_accepts_explicit_branch_while_head_is_detached() {
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["checkout", "-q", "-b", "feature/detached-ci"]);
    let id = repo.add("done before CI", &[]);
    repo.levi_ok(&["start", &id]);
    repo.commit("done");
    repo.levi_ok(&["close", &id]);

    let main_result = failed_json(
        &repo,
        &[
            "check-claims",
            "--git-ref",
            "feature/detached-ci",
            "--at",
            "main",
            "--json",
        ],
    );
    assert_eq!(main_result["tasks"][0]["status"], "open");

    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let result = repo.levi_json(&["check-claims", "--git-ref", "feature/detached-ci", "--json"]);
    assert_eq!(result["ok"], true);
}
