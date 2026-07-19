//! The heart of the spec: status is a function of git ancestry.

mod common;

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;

fn status_of(ls: &Value, id: &str) -> Option<String> {
    ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(id))
        .map(|t| t["status"].as_str().unwrap().to_string())
}

#[test]
fn close_on_feature_branch_stays_open_on_main_until_merge() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("branch-local fix", &[]);

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit("the fix");
    repo.levi_ok(&["close", &id]);

    // On feature: closed (gone from default ls; visible with --all).
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
    assert_eq!(ls["tasks"][0]["resolution"], "exact");

    // On main: the fixing commit isn't in ancestry — genuinely open here.
    repo.checkout("main");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "open");

    // --branch feature sees it closed from main's checkout.
    let ls = repo.levi_json(&["ls", "--json", "--all", "--branch", "feature"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");

    // Merge the fix: now closed on main too.
    repo.merge("feature");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
}

#[test]
fn branch_created_before_close_sees_task_open() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("late fix", &[]);
    repo.branch("old-branch");
    repo.commit("fixing commit");
    repo.levi_ok(&["close", &id]);

    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");

    repo.checkout("old-branch");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "open");
}

#[test]
fn no_anchor_close_applies_everywhere() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("process task, not code", &[]);
    repo.branch("elsewhere");
    repo.levi_ok(&["close", &id, "--no-anchor"]);

    repo.checkout("elsewhere");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
}

#[test]
fn reopen_is_ancestry_scoped_too() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("regression", &[]);

    // Close on main (anchored), then branch, then reopen on main only.
    repo.commit("fix");
    repo.levi_ok(&["close", &id]);
    repo.branch("feature");
    repo.commit("regressed again");
    repo.levi_ok(&["reopen", &id]);

    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "open");

    // The feature branch has the close but not the reopen: closed there.
    repo.checkout("feature");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
}

#[test]
fn redundant_transitions_refused_without_force() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    repo.levi(&["reopen", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already open"));
    repo.levi_ok(&["close", &id]);
    repo.levi(&["close", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already closed"));
    repo.levi_ok(&["close", &id, "--force"]);
}

#[test]
fn explicit_anchor_overrides_head() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    let old = repo.commit("old point");
    repo.branch("at-old");
    repo.commit("newer");
    // Anchor the close at `old`, not HEAD: the branch at old sees it closed.
    repo.levi_ok(&["close", &id, "--anchor", &old]);
    repo.checkout("at-old");
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
}

#[test]
fn worktrees_share_events_but_resolve_own_head() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("wt task", &[]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit("fix on feature");
    repo.levi_ok(&["close", &id]);
    repo.checkout("main");

    // Add a worktree on feature: same event log, feature's ancestry.
    let wt = repo.path().join("..").join("levi-wt-test");
    let wt_str = wt.to_string_lossy().into_owned();
    repo.git(&["worktree", "add", "-q", &wt_str, "feature"]);
    let wt = wt.canonicalize().unwrap();

    // From the main checkout: open. From the worktree: closed.
    let ls = repo.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(status_of(&ls, &id).unwrap(), "open");
    let out = repo
        .levi_in(wt.clone(), &["ls", "--json", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ls: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");

    repo.git(&["worktree", "remove", "--force", &wt.to_string_lossy()]);
}
