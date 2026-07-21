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
