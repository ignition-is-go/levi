mod common;

use common::TestRepo;
use predicates::prelude::*;

#[test]
fn ranking_priority_then_unblocks_then_age() {
    let repo = TestRepo::new();
    repo.init();
    let older = repo.add("older p2", &[]);
    let unblocker = repo.add("unblocker p2", &[]);
    let _blocked = repo.add("blocked", &["--dep", &unblocker]);
    let urgent = repo.add("urgent", &["-p", "p0"]);

    let next = repo.levi_json(&["next", "--json", "-n", "10"]);
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    // urgent (P0) first; then unblocker (frees one open task) over older;
    // blocked is ineligible.
    assert_eq!(ids[0], urgent);
    assert_eq!(ids[1], unblocker);
    assert_eq!(ids[2], older);
    assert_eq!(ids.len(), 3);
    assert!(
        next["tasks"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("P0")
    );
}

#[test]
fn blocked_becomes_eligible_when_blocker_closes_on_this_branch() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("blocker", &[]);
    let blocked = repo.add("blocked", &["--dep", &blocker]);

    // Close the blocker on a side branch only.
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.commit("fix blocker");
    repo.levi_ok(&["close", &blocker]);

    // On side: blocked is eligible (blocker closed here).
    let next = repo.levi_json(&["next", "--json", "-n", "10"]);
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&blocked.as_str()));

    // On main: blocker still open -> blocked ineligible, blocker eligible.
    repo.checkout("main");
    let next = repo.levi_json(&["next", "--json", "-n", "10"]);
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&blocker));
    assert!(!ids.contains(&blocked));
}

#[test]
fn claim_flow_start_steal_drop() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("a", &["-p", "p0"]);
    let b = repo.add("b", &[]);

    // next --claim claims the top task; a second next returns the other.
    let first = repo.levi_json(&["next", "--claim", "--json"]);
    assert_eq!(first["tasks"][0]["id"].as_str().unwrap(), a);
    assert!(first["tasks"][0]["claim"]["dev"].as_str().is_some());
    let second = repo.levi_json(&["next", "--json"]);
    // Our own claim doesn't exclude a task for us — but next should prefer
    // unclaimed work? No: spec says eligible excludes only *foreign* claims,
    // so `a` is still eligible for us. Verify via ls --mine instead.
    let mine = repo.levi_json(&["ls", "--json", "--mine"]);
    let mine_ids: Vec<_> = mine["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(mine_ids, vec![a.as_str()]);
    assert!(!second["tasks"].as_array().unwrap().is_empty());

    // start on an already-claimed task fails (same identity claims are fine,
    // so exercise failure via a fake foreign claim: steal from another
    // "worktree" by running in a git worktree).
    repo.levi_ok(&["drop", &a]);
    let mine = repo.levi_json(&["ls", "--json", "--mine"]);
    assert_eq!(mine["tasks"].as_array().unwrap().len(), 0);

    // steal always wins.
    repo.levi_ok(&["start", &b]);
    repo.levi_ok(&["steal", &b]);
    repo.levi(&["drop", &a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not claimed"));
}

#[test]
fn foreign_claim_excludes_task_from_next() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("a", &["-p", "p0"]);
    let b = repo.add("b", &[]);

    // Claim `a` from a different worktree => different claim identity.
    let wt = repo.path().join("..").join("levi-claim-wt");
    let wt_str = wt.to_string_lossy().into_owned();
    repo.git(&["worktree", "add", "-q", &wt_str, "-b", "wtbranch"]);
    let wt = wt.canonicalize().unwrap();
    let out = repo.levi_in(wt.clone(), &["start", &a]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // From the main checkout, `a` is claimed by someone else: next -> b.
    let next = repo.levi_json(&["next", "--json"]);
    assert_eq!(next["tasks"][0]["id"].as_str().unwrap(), b);

    // start on it fails; steal takes it.
    repo.levi(&["start", &a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already claimed"));
    repo.levi_ok(&["steal", &a]);
    let next = repo.levi_json(&["next", "--json"]);
    assert_eq!(next["tasks"][0]["id"].as_str().unwrap(), a);

    repo.git(&["worktree", "remove", "--force", &wt.to_string_lossy()]);
}

#[test]
fn expired_ttl_frees_the_claim() {
    let repo = TestRepo::new();
    // ttl 0: claims are dead on arrival.
    std::fs::write(
        repo.path().join("levi-test-config.toml"),
        "[claim]\nttl_secs = 0\n",
    )
    .unwrap();
    repo.init();
    let a = repo.add("a", &[]);
    repo.levi_ok(&["start", &a]);
    // Claim exists but is expired: still eligible, not "mine".
    let next = repo.levi_json(&["next", "--json"]);
    assert_eq!(next["tasks"][0]["id"].as_str().unwrap(), a);
    let mine = repo.levi_json(&["ls", "--json", "--mine"]);
    assert_eq!(mine["tasks"].as_array().unwrap().len(), 0);
}

#[test]
fn parallel_next_claim_yields_distinct_tasks() {
    let repo = TestRepo::new();
    repo.init();
    for i in 0..4 {
        repo.add(&format!("task {i}"), &[]);
    }
    // 4 concurrent `next --claim` from 4 distinct identities (worktrees).
    let mut worktrees = Vec::new();
    for i in 0..4 {
        let wt = repo.path().join("..").join(format!("levi-par-wt-{i}"));
        let wt_str = wt.to_string_lossy().into_owned();
        repo.git(&["worktree", "add", "-q", &wt_str, "-b", &format!("par-{i}")]);
        worktrees.push(wt.canonicalize().unwrap());
    }
    let bin = assert_cmd::cargo::cargo_bin("levi");
    let config = repo.path().join("levi-test-config.toml");
    let children: Vec<_> = worktrees
        .iter()
        .map(|wt| {
            std::process::Command::new(&bin)
                .current_dir(wt)
                .env("LEVI_CONFIG", &config)
                .args(["--no-sync", "next", "--claim", "--json"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut claimed = Vec::new();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        claimed.push(v["tasks"][0]["id"].as_str().unwrap().to_string());
    }
    claimed.sort();
    claimed.dedup();
    assert_eq!(
        claimed.len(),
        4,
        "each parallel agent must claim a distinct task"
    );
    for wt in &worktrees {
        repo.git(&["worktree", "remove", "--force", &wt.to_string_lossy()]);
    }
}
