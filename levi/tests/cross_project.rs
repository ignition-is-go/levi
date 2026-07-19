//! Cross-project dependencies + upstream bug filing (spec 2026-07-19).
//! CI has no real sibling checkouts: every repo here is fabricated in temp
//! dirs and the state dir is per-repo (see common harness).

mod common;

use std::net::TcpListener;
use std::time::{Duration, Instant};

use common::TestRepo;
use predicates::prelude::*;
use serde_json::Value;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_hub() -> u16 {
    let port = free_port();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            levi_core::link();
            let server = myko_server::CellServer::builder()
                .with_bind_addr(([127, 0, 0, 1], port).into())
                .build();
            if let Err(e) = server.run().await {
                eprintln!("in-process hub died: {e}");
            }
        });
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return port;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("in-process hub did not start");
}

/// Two independent projects (their own repos), one hub. Returns (a, b).
fn two_projects(hub_port: u16) -> (TestRepo, TestRepo) {
    let a = TestRepo::new();
    let b = TestRepo::new();
    let hub = format!("127.0.0.1:{hub_port}");
    a.set_hub(&hub);
    b.set_hub(&hub);
    a.levi_ok(&["init", "--name", "downstream"]);
    b.levi_ok(&["init", "--name", "upstream"]);
    a.levi_ok(&["sync", "--no-git"]);
    b.levi_ok(&["sync", "--no-git"]);
    (a, b)
}

#[test]
fn file_bug_upstream_and_block_on_it() {
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);

    // A files a bug in B's project, by name, through the hub.
    let out = a.levi_ok(&[
        "add",
        "--project",
        "upstream",
        "upstream bug found from downstream",
        "-p",
        "p1",
    ]);
    assert!(out.contains("upstream/lv-"), "got: {out}");
    let foreign_full = out.split_whitespace().last().unwrap().to_string(); // "<project_id>/<task_id>"
    let (b_project_id, foreign_task_id) = foreign_full.split_once('/').unwrap();

    // A comments on it too.
    a.levi_ok(&["comment", &foreign_full, "repro from downstream's side"]);

    // B syncs: the filed bug and comment land in B's real ref.
    b.levi_ok(&["sync", "--no-git"]);
    let ls = b.levi_json(&["ls", "--json"]);
    let task = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(foreign_task_id))
        .expect("filed bug reached upstream's repo");
    assert_eq!(task["title"], "upstream bug found from downstream");
    let show = b.levi_json(&["show", foreign_task_id, "--json"]);
    assert_eq!(show["comments"][0]["body"], "repro from downstream's side");

    // A blocks a local task on the foreign bug, with a via annotation.
    let local = a.add("drop workaround once upstream fixes", &[]);
    a.levi_ok(&[
        "dep",
        "add",
        &local,
        "--on",
        &format!("upstream/lv-{}", &foreign_task_id[..4]),
        "--via",
        "cargo: crates.io upstream ^1.0",
    ]);

    // Unknown foreign status (no cache yet): blocked, and show says so.
    let next = a.levi_json(&["next", "--json", "-n", "10"]);
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&local.as_str()),
        "must be blocked while unknown"
    );
    let show = a.levi_json(&["show", &local, "--json"]);
    assert_eq!(
        show["blocked_by"][0]["id"].as_str().unwrap(),
        foreign_task_id
    );
    assert_eq!(
        show["blocked_by"][0]["project_id"].as_str().unwrap(),
        b_project_id
    );
    assert_eq!(
        show["blocked_by"][0]["via"],
        "cargo: crates.io upstream ^1.0"
    );

    // A syncs: cache refreshes from hub facts -> still open (B hasn't fixed).
    a.levi_ok(&["sync", "--no-git"]);
    let show = a.levi_json(&["show", &local, "--json"]);
    assert_eq!(show["blocked_by"][0]["status"], "open");
    assert_eq!(show["blocked_by"][0]["resolution"], "facts");

    // B fixes on main and syncs (events + facts + reffacts reach the hub).
    b.commit("the upstream fix");
    b.levi_ok(&["close", foreign_task_id]);
    b.levi_ok(&["sync", "--no-git"]);

    // A syncs: the cache flips closed; the task becomes eligible and the
    // reason carries the verify-via note.
    a.levi_ok(&["sync", "--no-git"]);
    let next = a.levi_json(&["next", "--json", "-n", "10"]);
    let entry = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(local.as_str()))
        .expect("unblocked after upstream fix landed on their main");
    let reason = entry["reason"].as_str().unwrap();
    assert!(reason.contains("closed"), "reason: {reason}");
    assert!(
        reason.contains("verify availability via: cargo: crates.io upstream ^1.0"),
        "reason: {reason}"
    );
}

#[test]
fn sibling_checkout_wins_and_tracks_its_head() {
    // No hub at all: the sibling checkout rung resolves exactly, offline.
    let a = TestRepo::new();
    let b = TestRepo::new();
    // Shared state dir so A can find B's checkout registration.
    let state = a.path().join("levi-test-state");
    a.levi_ok(&["init", "--name", "downstream"]);

    let mut init_b = assert_cmd::Command::cargo_bin("levi").unwrap();
    init_b
        .current_dir(b.path())
        .env("LEVI_CONFIG", b.path().join("levi-test-config.toml"))
        .env("LEVI_STATE_DIR", &state)
        .args(["--no-sync", "init", "--name", "upstream"]);
    init_b.assert().success();
    let levi_b = |args: &[&str]| {
        let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
        cmd.current_dir(b.path())
            .env("LEVI_CONFIG", b.path().join("levi-test-config.toml"))
            .env("LEVI_STATE_DIR", &state)
            .arg("--no-sync")
            .args(args);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // A task in B.
    let b_task = levi_b(&["add", "fix the shared library"])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    // B's project id, via the checkouts registry it just wrote.
    let b_project = {
        let reg = std::fs::read_to_string(state.join("checkouts.toml")).unwrap();
        let doc: toml::Table = reg.parse().unwrap();
        doc.into_iter()
            .find(|(_, v)| {
                v.as_table()
                    .map(|t| {
                        t.keys().any(|k| {
                            k.contains(&b.path().file_name().unwrap().to_string_lossy().to_string())
                        })
                    })
                    .unwrap_or(false)
            })
            .map(|(k, _)| k)
            .expect("B registered itself")
    };

    let local = a.add("consume the fixed library", &[]);
    a.levi_ok(&[
        "dep",
        "add",
        &local,
        "--on",
        &format!("{b_project}/{b_task}"),
        "--via",
        "path dep ../upstream",
    ]);

    // A's harness state dir differs; re-point A at the shared registry.
    let levi_a_shared = |args: &[&str]| {
        let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
        cmd.current_dir(a.path())
            .env("LEVI_CONFIG", a.path().join("levi-test-config.toml"))
            .env("LEVI_STATE_DIR", &state)
            .arg("--no-sync")
            .args(args);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Open in B's checkout -> A is blocked (exact, offline).
    let next: Value =
        serde_json::from_str(&levi_a_shared(&["next", "--json", "-n", "10"])).unwrap();
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&local.as_str()));
    let show: Value = serde_json::from_str(&levi_a_shared(&["show", &local, "--json"])).unwrap();
    assert_eq!(show["blocked_by"][0]["status"], "open");
    assert_eq!(show["blocked_by"][0]["resolution"], "exact");
    assert_eq!(show["blocked_by"][0]["title"], "fix the shared library");

    // B commits the fix and closes at its HEAD: A unblocks immediately.
    b.commit("the fix in the sibling");
    levi_b(&["close", &b_task]);
    let next: Value =
        serde_json::from_str(&levi_a_shared(&["next", "--json", "-n", "10"])).unwrap();
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&local.as_str()),
        "sibling close unblocks exactly"
    );

    // B rewinds to before the fix: A re-blocks (status tracks B's HEAD).
    b.git(&["checkout", "-q", "-b", "before-fix", "HEAD~1"]);
    let next: Value =
        serde_json::from_str(&levi_a_shared(&["next", "--json", "-n", "10"])).unwrap();
    let ids: Vec<_> = next["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&local.as_str()),
        "sibling checked out pre-fix history: blocked again"
    );
}

#[test]
fn cross_project_errors_are_clear() {
    let repo = TestRepo::new();
    repo.init();
    // No hub configured.
    repo.levi(&["add", "--project", "upstream", "t"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("need a hub"));
    // --dep with --project rejected.
    let hub_port = start_hub();
    repo.set_hub(&format!("127.0.0.1:{hub_port}"));
    repo.levi(&["add", "--project", "upstream", "t", "--dep", "abcd"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not supported"));
    // Unknown project name.
    repo.levi_ok(&["sync", "--no-git"]);
    repo.levi(&["add", "--project", "nope", "t"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no project named"));
}

#[test]
fn machine_id_distinguishes_identical_hostname_and_worktree() {
    // Same repo, same worktree, same hostname — but two different state
    // dirs = two machines. The second "machine" must not own the first's
    // claim.
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("contended", &[]);
    let state_a = repo.path().join("state-machine-a");
    let state_b = repo.path().join("state-machine-b");

    let levi_as = |state: &std::path::Path, args: &[&str]| {
        let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
        cmd.current_dir(repo.path())
            .env("LEVI_CONFIG", repo.path().join("levi-test-config.toml"))
            .env("LEVI_STATE_DIR", state)
            .arg("--no-sync")
            .args(args);
        cmd
    };

    levi_as(&state_a, &["start", &task]).assert().success();
    // Machine B: same dev/hostname/worktree, different machine id — the
    // claim is foreign to it.
    levi_as(&state_b, &["start", &task])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already claimed"));
    levi_as(&state_b, &["drop", &task])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not you"));
    // Machine A still owns it.
    levi_as(&state_a, &["drop", &task]).assert().success();
}
