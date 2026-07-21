//! Sync git leg: two clones sharing a bare remote, plus orphaned-anchor
//! warnings after a rebase-like history rewrite.

mod common;

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;
use std::path::PathBuf;

fn status_of(ls: &Value, id: &str) -> Option<String> {
    ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(id))
        .map(|t| t["status"].as_str().unwrap().to_string())
}

/// Two clones (a, b) of one bare remote. Returns (bare_holder, a, b).
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

#[test]
fn sync_round_trips_events_between_clones() {
    let (base, a, b) = two_clones();

    // A initializes, adds, closes anchored at a commit it pushes.
    base.levi_in(a.clone(), &["init"]).assert().success();
    let out = base
        .levi_in(a.clone(), &["add", "shared task"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    base.git_in(&a, &["commit", "-q", "--allow-empty", "-m", "fix"]);
    base.levi_in(a.clone(), &["close", &id]).assert().success();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();
    // Push the fixing commit itself separately: levi sync only moves refs/levi/*.
    base.git_in(&a, &["push", "-q", "origin", "main"]);

    // B syncs: events arrive, but B doesn't have the fixing commit yet, so
    // the anchor is unknown -> open + partial resolution.
    base.levi_in(b.clone(), &["sync", "--no-hub"])
        .assert()
        .success();
    let out = base
        .levi_in(b.clone(), &["ls", "--json", "--all"])
        .output()
        .unwrap();
    let ls: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status_of(&ls, &id).unwrap(), "open");
    let task = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id.as_str())
        .unwrap();
    assert_eq!(task["resolution"], "partial");

    // B pulls the git history too: now the anchor is in ancestry -> closed.
    base.git_in(&b, &["pull", "-q"]);
    let out = base
        .levi_in(b.clone(), &["ls", "--json", "--all"])
        .output()
        .unwrap();
    let ls: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status_of(&ls, &id).unwrap(), "closed");
    let task = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id.as_str())
        .unwrap();
    assert_eq!(task["resolution"], "exact");
}

#[test]
fn divergent_appends_converge_to_identical_state() {
    let (base, a, b) = two_clones();
    base.levi_in(a.clone(), &["init"]).assert().success();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();
    base.levi_in(b.clone(), &["sync", "--no-hub"])
        .assert()
        .success();

    // Both add tasks offline.
    for i in 0..3 {
        base.levi_in(a.clone(), &["add", &format!("from a {i}")])
            .assert()
            .success();
        base.levi_in(b.clone(), &["add", &format!("from b {i}")])
            .assert()
            .success();
    }
    // Two sync rounds each (push/pull propagation).
    for _ in 0..2 {
        base.levi_in(a.clone(), &["sync", "--no-hub"])
            .assert()
            .success();
        base.levi_in(b.clone(), &["sync", "--no-hub"])
            .assert()
            .success();
    }

    let out_a = base
        .levi_in(a.clone(), &["ls", "--json", "--all"])
        .output()
        .unwrap();
    let out_b = base
        .levi_in(b.clone(), &["ls", "--json", "--all"])
        .output()
        .unwrap();
    assert_eq!(
        out_a.stdout, out_b.stdout,
        "materialized state must be byte-identical"
    );
    let ls: Value = serde_json::from_slice(&out_a.stdout).unwrap();
    assert_eq!(ls["tasks"].as_array().unwrap().len(), 6);
}

#[test]
fn sync_without_remote_is_graceful() {
    let repo = TestRepo::new();
    repo.init();
    repo.levi(&["sync", "--no-hub"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no remote"));
}

#[test]
fn rebased_away_anchor_warns() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    repo.commit("will be rebased away");
    repo.levi_ok(&["close", &id]);
    // Rewrite history: reset to the initial commit, add a different commit.
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    repo.commit("replacement history");
    repo.levi(&["ls", "--all"])
        .assert()
        .success()
        .stderr(predicate::str::contains("unreachable from any ref"));
}

#[test]
fn mutations_sync_to_git_remote_in_background() {
    // No hub, just a git remote: `levi add` (without --no-sync) must land the
    // events ref on the remote by itself, via the detached background sync.
    let repo = TestRepo::new();
    let bare = repo.path().join("origin.git");
    repo.git(&["init", "-q", "-b", "main", "--bare", bare.to_str().unwrap()]);
    repo.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
    repo.init();

    let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
    cmd.current_dir(repo.path())
        .env("LEVI_CONFIG", repo.path().join("levi-test-config.toml"))
        .args(["add", "fire and forget"]);
    cmd.assert().success();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let out = std::process::Command::new("git")
            .args(["ls-remote", bare.to_str().unwrap(), "refs/levi/events"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        if !out.stdout.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background sync never pushed the events ref"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
fn rewritten_anchor_suggests_the_new_sha() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("t", &[]);
    repo.commit_file("fix.txt", "the fix\n", "original message");
    repo.levi_ok(&["close", &id]);
    // Reword the commit: same diff, new sha — the anchor is now orphaned
    // but patch-id matches the rewritten commit.
    repo.git(&["commit", "--amend", "-q", "-m", "reworded message"]);
    let new_sha = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let out = repo.levi(&["ls", "--all"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("rewritten as {}", &new_sha[..8])),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("--anchor {}", &new_sha[..8])),
        "stderr: {stderr}"
    );
}

#[test]
fn next_recovers_events_from_remote_on_fresh_clone() {
    let (base, a, b) = two_clones();
    base.levi_in(a.clone(), &["init"]).assert().success();
    base.levi_in(a.clone(), &["add", "remote task"])
        .assert()
        .success();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();

    // b has no refs/levi/events; `next` without --no-sync must fetch it.
    let out = base
        .levi_syncing(b.clone(), &["next", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let next: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = next["tasks"].as_array().unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(tasks[0]["title"], "remote task");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("fetched events via sync"),
        "stderr: {stderr}"
    );
}

#[test]
fn next_stays_quiet_when_remote_has_no_events() {
    let (base, _a, b) = two_clones();

    // Neither clone has ever run `levi init`: no events locally or on the
    // remote. `next` without --no-sync should skip the doomed push quietly
    // instead of printing a misleading push-failure note.
    let out = base
        .levi_syncing(b.clone(), &["next", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"tasks\":[]"), "stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no levi events here"), "stderr: {stderr}");
    assert!(
        predicate::str::contains("sync attempt failed")
            .not()
            .eval(&stderr),
        "stderr: {stderr}"
    );
}

#[test]
fn next_with_no_sync_does_not_recover() {
    let (base, a, b) = two_clones();
    base.levi_in(a.clone(), &["init"]).assert().success();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();

    // levi_in injects --no-sync: today's dead-end message, no fetch.
    base.levi_in(b.clone(), &["next", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tasks\":[]"))
        .stderr(predicate::str::contains("no levi events here"));
}

#[test]
fn next_without_remote_degrades_gracefully() {
    // No remote configured at all: recovery is a quiet miss, exit 0.
    let repo = TestRepo::new();
    repo.levi_syncing(repo.path().to_path_buf(), &["next", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tasks\":[]"))
        .stderr(predicate::str::contains("no levi events here"));
}

#[test]
fn ls_recovers_events_from_remote_on_fresh_clone() {
    let (base, a, b) = two_clones();
    base.levi_in(a.clone(), &["init"]).assert().success();
    base.levi_in(a.clone(), &["add", "remote task"])
        .assert()
        .success();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();

    let out = base
        .levi_syncing(b.clone(), &["ls", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ls: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(ls["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn init_adopts_existing_project_from_remote() {
    let (base, a, b) = two_clones();
    let a_out = base
        .levi_in(a.clone(), &["init", "--name", "shared"])
        .output()
        .unwrap();
    assert!(a_out.status.success());
    // The "initialized levi project 'shared' (<id>)" line — `init` now also
    // writes agent instructions and (absent --hub) prints a tip, both of
    // which can contain their own '(...)' groups, so scope the id
    // extraction to just that line rather than the whole stdout.
    let a_stdout = String::from_utf8_lossy(&a_out.stdout).to_string();
    let a_line = a_stdout
        .lines()
        .find(|l| l.starts_with("initialized levi project"))
        .unwrap_or_else(|| panic!("no 'initialized levi project' line — stdout: {a_stdout}"));
    let a_id = a_line
        .split('(')
        .nth(1)
        .unwrap()
        .trim_end_matches(')')
        .to_string();
    base.levi_in(a.clone(), &["sync", "--no-hub"])
        .assert()
        .success();

    let out = base.levi_syncing(b.clone(), &["init"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let b_line = stdout
        .lines()
        .find(|l| l.starts_with("joined existing levi project 'shared'"))
        .unwrap_or_else(|| {
            panic!("no \"joined existing levi project 'shared'\" line — stdout: {stdout}")
        });
    assert!(b_line.contains(&a_id), "ids must match — line: {b_line}");
}

#[test]
fn init_mints_when_remote_has_no_events_ref() {
    let (base, _a, b) = two_clones();
    let out = base.levi_syncing(b.clone(), &["init"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("initialized levi project"),);
}

#[test]
fn init_bails_when_remote_unreachable_unless_no_sync() {
    let repo = TestRepo::new();
    repo.git(&["remote", "add", "origin", "/nonexistent/nowhere.git"]);
    repo.levi_syncing(repo.path().to_path_buf(), &["init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot reach remote 'origin'"));
    // Escape hatch: --no-sync skips the probe and mints (levi() injects it).
    repo.levi(&["init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized levi project"));
}

/// lv-1e92: a squash merge replaces the branch commit with a new one, so an
/// anchor on the branch is never in main's ancestry and its task reads open
/// forever. The branch usually still exists, so the orphan path (which needs
/// the anchor unreachable from *every* ref) stays silent.
#[test]
fn squash_merged_anchor_suggests_the_squashed_sha() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("fix the thing", &[]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("fix.txt", "the fix\n", "the fix");
    repo.levi_ok(&["close", &id]);

    // Squash-merge onto main, leaving `feature` (and its anchor) alive.
    repo.checkout("main");
    repo.git(&["merge", "-q", "--squash", "feature"]);
    repo.git(&["commit", "-q", "-m", "fix the thing (#9)"]);
    let squashed = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

    let out = repo.levi(&["ls", "--all"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&squashed[..8]),
        "must name the squashed commit — stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("--anchor {}", &squashed[..8])),
        "must suggest the re-anchor command — stderr: {stderr}"
    );
}

/// The same detection must not fire for ordinary unmerged feature work:
/// that anchor is legitimately absent from this history and the task is
/// correctly open here.
#[test]
fn unmerged_feature_anchor_is_not_reported_as_squashed() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("fix the thing", &[]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("fix.txt", "the fix\n", "the fix");
    repo.levi_ok(&["close", &id]);
    repo.checkout("main");

    let out = repo.levi(&["ls", "--all"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("squashed"),
        "unmerged work must not be reported as squashed — stderr: {stderr}"
    );
}
