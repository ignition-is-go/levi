//! Facts leg at scale (lv-b69e): a first publication from a large history
//! is chunked — each chunk barriers, verifies, and lands in the dedup cache
//! before the next, so no single hub round-trip waits on the whole history.

mod common;

use common::TestRepo;
use common::start_hub;

/// Extend the repo's history by `n` empty commits without touching the
/// index: a commit-tree chain and one ref update (fast enough for hundreds
/// of commits, unlike `git commit` per step).
fn grow_history(repo: &TestRepo, n: usize) {
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string();
    let mut parent = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    for i in 0..n {
        let out = std::process::Command::new("git")
            .args(["commit-tree", &tree, "-p", &parent, "-m", &format!("c{i}")])
            .current_dir(repo.path())
            .output()
            .expect("git commit-tree runs");
        assert!(
            out.status.success(),
            "commit-tree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        parent = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    repo.git(&["update-ref", "refs/heads/main", &parent]);
}

/// The `published N fact(s)` count from a `levi sync --no-git` run.
fn published_facts(repo: &TestRepo) -> usize {
    let out = repo.levi_ok(&["sync", "--no-git"]);
    let line = out
        .lines()
        .find(|l| l.contains("published"))
        .unwrap_or_else(|| panic!("no hub line in sync output: {out}"));
    line.split("published ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("cannot parse fact count from: {line}"))
}

/// lv-ef44: an unchanged branch head must not be republished on every sync.
/// Before the ref cache, a repo published one RefFact per branch per sync
/// forever — 95 events per `levi add` on a 95-branch repo.
#[test]
fn unchanged_branch_heads_are_not_republished() {
    let hub_port = start_hub();
    let repo = TestRepo::new();
    repo.set_hub(&format!("127.0.0.1:{hub_port}"));
    repo.init();
    for b in ["feature-a", "feature-b", "feature-c"] {
        repo.branch(b);
    }

    // First sync publishes the initial commit + 4 branch heads.
    assert!(published_facts(&repo) >= 4, "first sync publishes the refs");

    // Nothing changed: an idle repo must publish nothing at all.
    assert_eq!(
        published_facts(&repo),
        0,
        "an idle repo republished facts — the ref cache is not holding"
    );
    assert_eq!(published_facts(&repo), 0, "still nothing on a third run");

    // A moved head is published again (exactly one RefFact + one CommitFact).
    repo.commit("advance main");
    assert_eq!(
        published_facts(&repo),
        2,
        "a moved branch head must republish its RefFact and the new commit"
    );
    assert_eq!(published_facts(&repo), 0, "then settle again");

    // A brand-new branch publishes only its own RefFact (its commit is known).
    repo.branch("feature-d");
    assert_eq!(published_facts(&repo), 1, "new branch publishes one RefFact");
}

#[test]
fn facts_publish_chunks_large_history() {
    let hub_port = start_hub();
    let repo = TestRepo::new();
    repo.set_hub(&format!("127.0.0.1:{hub_port}"));
    repo.init();
    // 600 commits + the initial one: several SEND_CHUNK-sized chunks.
    grow_history(&repo, 600);

    let out = repo.levi(&["sync", "--no-git"]).output().unwrap();
    assert!(
        out.status.success(),
        "sync failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Every commit sha is verified-and-recorded in the dedup cache.
    let cache_path = repo.path().join(".git/levi/facts-published");
    let cache = std::fs::read_to_string(&cache_path).expect("cache written");
    assert_eq!(cache.lines().count(), 601, "601 commits published");

    // Re-sync: nothing re-published, cache stable (dedup works across runs).
    let out = repo.levi(&["sync", "--no-git"]).output().unwrap();
    assert!(out.status.success());
    let cache = std::fs::read_to_string(&cache_path).unwrap();
    assert_eq!(cache.lines().count(), 601);
}
