//! Blockers that aren't tasks: `block` and `unblock`, with `next` skipping a
//! blocked task and `show`/`ls` saying what it waits on.

mod common;

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;

fn next_ids(repo: &TestRepo) -> Vec<String> {
    repo.levi_json(&["next", "--json", "-n", "10"])["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

fn blocker_id(out: &str) -> String {
    // "bk-xxxxxxxx blocks lv-xxxx"
    out.split_whitespace().next().unwrap().to_string()
}

#[test]
fn a_blocked_task_leaves_next_until_unblocked() {
    let repo = TestRepo::new();
    repo.init();
    let top = repo.add("needs a port", &["-p", "p0"]);
    let other = repo.add("something else", &[]);
    assert_eq!(next_ids(&repo), [top.clone(), other.clone()]);

    let out = repo.levi_ok(&[
        "block",
        &top,
        "-m",
        "waiting on the operator to open port 6300",
    ]);
    let bk = blocker_id(&out);
    assert!(bk.starts_with("bk-"), "{out}");
    assert_eq!(next_ids(&repo), vec![other.clone()]);

    let ls = repo.levi_json(&["ls", "--json"]);
    let t = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == top.as_str())
        .unwrap();
    assert_eq!(t["blockers"][0]["short"], bk.as_str());
    assert_eq!(
        t["blockers"][0]["reason"],
        "waiting on the operator to open port 6300"
    );
    assert_eq!(t["blockers"][0]["source"], "declared");
    let text = repo.levi_ok(&["ls"]);
    assert!(
        text.contains("⛔ waiting on: waiting on the operator"),
        "{text}"
    );

    repo.levi_ok(&["unblock", &bk, "-m", "the operator opened it"]);
    assert_eq!(next_ids(&repo), [top.clone(), other]);

    // show keeps the closed blocker, with who closed it and why.
    let show = repo.levi_json(&["show", &top, "--json"]);
    let b = &show["blockers"][0];
    assert_eq!(b["open"], false);
    assert_eq!(b["closed_by"], "agent@test");
    assert_eq!(b["closed_reason"], "the operator opened it");
    assert!(b["closed_at"].is_string());
    let text = repo.levi_ok(&["show", &top]);
    assert!(text.contains("waiting on (declared):"), "{text}");
    assert!(text.contains("closed by agent@test"), "{text}");
    let ls: Value = repo.levi_json(&["ls", "--json"]);
    assert!(
        ls["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["blockers"].as_array().unwrap().is_empty())
    );
}

#[test]
fn block_and_unblock_need_reasons_and_a_real_blocker() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    repo.levi(&["block", &id]).assert().failure();
    repo.levi(&["block", &id, "-m", " "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("waiting on"));

    let bk = blocker_id(&repo.levi_ok(&["block", &id, "-m", "a review"]));
    repo.levi(&["unblock", &bk, "-m", ""])
        .assert()
        .failure()
        .stderr(predicate::str::contains("why"));
    repo.levi(&["unblock", "bk-ab", "-m", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least 4"));
    repo.levi(&["unblock", "bk-zzzzzzzz", "-m", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no blocker matches"));

    // The bare hex works as well as the bk- form.
    repo.levi_ok(&["unblock", bk.trim_start_matches("bk-"), "-m", "done"]);
    repo.levi(&["unblock", &bk, "-m", "again"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already closed"));
}

#[test]
fn two_blockers_both_have_to_close() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    let a = blocker_id(&repo.levi_ok(&["block", &id, "-m", "first"]));
    let b = blocker_id(&repo.levi_ok(&["block", &id, "-m", "second"]));
    repo.levi_ok(&["unblock", &a, "-m", "done"]);
    assert!(next_ids(&repo).is_empty());
    repo.levi_ok(&["unblock", &b, "-m", "done"]);
    assert_eq!(next_ids(&repo), [id]);
}
