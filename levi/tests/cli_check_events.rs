mod common;

use common::TestRepo;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

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

/// `git mktree`, fed the tree listing on stdin.
fn mktree(repo: &TestRepo, listing: &str) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .arg("mktree")
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(listing.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_descendant_that_drops_events_fails() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("first", &[]);
    repo.add("second", &[]);
    let saved = head(&repo);
    let tasks = |repo: &TestRepo| {
        repo.levi_json(&["ls", "--json", "--all"])["tasks"]
            .as_array()
            .unwrap()
            .len()
    };
    let before = tasks(&repo);

    // A commit on top of the saved head whose tree leaves one shard out.
    let shards = repo.git(&["ls-tree", &saved]);
    let kept: String = shards.lines().skip(1).map(|l| format!("{l}\n")).collect();
    let tree = mktree(&repo, &kept);
    let forged = repo.git(&["commit-tree", tree.trim(), "-p", &saved, "-m", "drop"]);
    repo.git(&["update-ref", "refs/levi/events", forged.trim()]);
    assert!(tasks(&repo) < before, "the forged head should hide a task");

    let out = repo
        .levi(&["check-events", "--expect", &saved, "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(
        value["reason"]
            .as_str()
            .unwrap()
            .contains("are missing from"),
        "{value}"
    );
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

#[test]
fn an_expected_object_that_isnt_a_commit_fails_saying_so() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("first", &[]);
    let saved = head(&repo);
    let tree = repo.git(&["rev-parse", &format!("{saved}^{{tree}}")]);
    let tree = tree.trim();
    let out = repo
        .levi(&["check-events", "--expect", tree, "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], false);
    let reason = value["reason"].as_str().unwrap();
    assert!(reason.contains("is a tree, not a commit"), "{value}");
    assert!(!reason.contains("reset or rewritten"), "{value}");
}

/// Two clones of one bare remote, each with its own git identity.
fn two_clones() -> (TestRepo, PathBuf, PathBuf) {
    let base = TestRepo::new();
    let bare = base.path().join("origin.git");
    base.git(&["init", "-q", "-b", "main", "--bare", bare.to_str().unwrap()]);
    base.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
    base.git(&["push", "-q", "origin", "main"]);
    let a = base.path().join("clone-a");
    let b = base.path().join("clone-b");
    for clone in [&a, &b] {
        base.git(&[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ]);
        base.git_in(clone, &["config", "user.email", "agent@test"]);
        base.git_in(clone, &["config", "user.name", "Agent"]);
    }
    (base, a, b)
}

fn head_in(base: &TestRepo, dir: &Path) -> String {
    let out = base
        .levi_in(dir.to_path_buf(), &["check-events", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    value["head"].as_str().unwrap().to_owned()
}

#[test]
fn heads_saved_on_either_clone_survive_sync_and_divergent_appends() {
    let (base, a, b) = two_clones();
    let sync = |dir: &PathBuf| {
        base.levi_in(dir.clone(), &["sync", "--no-hub"])
            .assert()
            .success();
    };
    base.levi_in(a.clone(), &["init"]).assert().success();
    base.levi_in(a.clone(), &["add", "from a"])
        .assert()
        .success();
    sync(&a);
    let saved_a = head_in(&base, &a);
    sync(&b);
    assert_eq!(head_in(&base, &b), saved_a, "b adopts a's ref");

    // Both append while apart, then sync, so the ref gains a union merge.
    base.levi_in(a.clone(), &["add", "a again"])
        .assert()
        .success();
    base.levi_in(b.clone(), &["add", "from b"])
        .assert()
        .success();
    let saved_b = head_in(&base, &b);
    sync(&a);
    sync(&b);
    sync(&a);
    // Union merges give the clones the same events, not always the same commit:
    // the same tasks on both, and every saved head still held on both.
    let ls = |dir: &PathBuf| {
        let out = base
            .levi_in(dir.clone(), &["ls", "--all", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    assert_eq!(ls(&a), ls(&b), "clones list the same tasks");

    for dir in [&a, &b] {
        for saved in [&saved_a, &saved_b] {
            base.levi_in(dir.clone(), &["check-events", "--expect", saved])
                .assert()
                .success();
        }
    }
}
