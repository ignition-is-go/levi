mod common;

use common::TestRepo;
use predicates::prelude::*;

fn head(repo: &TestRepo) -> String {
    let value = repo.levi_json(&["check-events", "--json"]);
    assert_eq!(value["schema"], "levi.check-events/1");
    assert_eq!(value["ok"], true);
    value["head"].as_str().unwrap().to_owned()
}

#[test]
fn a_saved_head_still_in_the_events_history_passes_as_events_are_added() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("first", &[]);
    let saved = head(&repo);
    assert_eq!(saved, repo.git(&["rev-parse", "refs/levi/events"]).trim());

    // The same head passes, and so does every head that grows from it.
    repo.levi_ok(&["check-events", "--expect", &saved]);
    repo.add("second", &[]);
    repo.add("third", &[]);
    assert_ne!(head(&repo), saved);
    let value = repo.levi_json(&["check-events", "--expect", &saved, "--json"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["expected"], saved);
    assert!(value["reason"].is_null());
}

#[test]
fn a_reset_or_rewritten_events_ref_fails() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("first", &[]);
    let before = head(&repo);
    repo.add("second", &[]);
    let saved = head(&repo);

    // Wind the ref back past the saved head, as a reset would leave it.
    repo.git(&["update-ref", "refs/levi/events", &before]);
    let out = repo
        .levi(&["check-events", "--expect", &saved, "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["head"], before);
    assert!(
        value["reason"]
            .as_str()
            .unwrap()
            .contains("reset or rewritten"),
        "{value}"
    );

    // A rewritten history, where new events don't descend from the saved head, fails too.
    repo.add("instead", &[]);
    repo.levi(&["check-events", "--expect", &saved])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reset or rewritten"));
}

#[test]
fn an_unknown_commit_an_absent_ref_or_a_short_hash_fails() {
    let repo = TestRepo::new();
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    // No events yet.
    repo.levi(&["check-events", "--expect", unknown])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is absent here"));
    repo.levi(&["check-events"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nothing to save"));

    repo.init();
    repo.levi(&["check-events", "--expect", unknown])
        .assert()
        .failure()
        .stderr(predicate::str::contains("isn't in this repository"));
    repo.levi(&["check-events", "--expect", "0123abc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("full commit hash"));
}

#[test]
fn it_only_reads() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("first", &[]);
    let saved = head(&repo);
    repo.levi_ok(&["check-events", "--expect", &saved]);
    repo.levi_ok(&["check-events"]);
    assert_eq!(head(&repo), saved, "checking changed the events ref");
}
