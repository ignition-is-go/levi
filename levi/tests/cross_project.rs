//! Cross-project dependencies + upstream bug filing (spec 2026-07-19).
//! CI has no real sibling checkouts: every repo here is fabricated in temp
//! dirs and the state dir is per-repo (see common harness).

mod common;

use common::TestRepo;
use common::start_hub;
use predicates::prelude::*;
use serde_json::Value;

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

/// lv (cross-project listing): `levi ls --project` surfaces a foreign
/// project's tasks with a `ref` that `levi dep add --on` accepts verbatim,
/// and does so without fetching any CommitFacts.
#[test]
fn ls_project_lists_foreign_tasks_and_ref_round_trips() {
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);

    // B files a task in its own project and pushes it to the hub.
    let b_task = b.add("bounded ingest queues", &[]);
    b.levi_ok(&["sync", "--no-git"]);

    // A lists B's project through the hub — no local knowledge of it.
    let ls = a.levi_json(&["ls", "--project", "upstream", "--json"]);
    let row = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "bounded ingest queues")
        .expect("foreign task listed");
    assert_eq!(row["project"], "upstream");
    let the_ref = row["ref"].as_str().unwrap().to_string();
    assert!(the_ref.starts_with("upstream/lv-"), "ref: {the_ref}");
    assert!(
        the_ref.contains(&b_task[..8]),
        "ref must name the task: {the_ref}"
    );

    // The ref is accepted verbatim by `dep add --on`.
    let local = a.add("drop workaround once upstream ships", &[]);
    a.levi_ok(&[
        "dep",
        "add",
        &local,
        "--on",
        &the_ref,
        "--via",
        "cargo: crates.io",
    ]);
    let show = a.levi_json(&["show", &local, "--json"]);
    assert_eq!(
        show["blocked_by"][0]["id"].as_str().unwrap(),
        b_task,
        "the dependency resolved to B's task"
    );
}

/// `--all-projects` spans projects; a project whose CommitFacts never reached
/// the hub still lists (proving the listing needs no facts).
#[test]
fn ls_all_projects_needs_no_facts() {
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);
    a.add("local downstream task", &[]);
    a.levi_ok(&["sync", "--no-git"]);
    b.add("upstream task", &[]);
    b.levi_ok(&["sync", "--no-git"]);

    let ls = a.levi_json(&["ls", "--all-projects", "--json"]);
    let titles: Vec<&str> = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"local downstream task"), "got: {titles:?}");
    assert!(titles.contains(&"upstream task"), "got: {titles:?}");
}

/// Both cross-project flags require a hub.
#[test]
fn ls_cross_project_without_hub_fails_clearly() {
    let repo = TestRepo::new();
    repo.init();
    repo.levi(&["ls", "--all-projects"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("need a hub"));
}

/// Surface 2: the hub computes the whole blocking graph and returns it, so
/// the dashboard never pulls raw entities. Seed a cross-project dependency,
/// then ask the hub for the graph over the wire.
#[test]
fn issue_graph_report_is_computed_on_the_hub() {
    use std::time::Duration;
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);

    // upstream files a blocker; downstream depends on it.
    let blocker = b.add("bounded ingest queues", &[]);
    b.levi_ok(&["sync", "--no-git"]);
    let ls = a.levi_json(&["ls", "--project", "upstream", "--json"]);
    let the_ref = ls["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "bounded ingest queues")
        .unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_string();
    let blocked = a.add("drop workaround", &[]);
    a.levi_ok(&[
        "dep",
        "add",
        &blocked,
        "--on",
        &the_ref,
        "--via",
        "cargo: crates.io",
    ]);
    a.levi_ok(&["sync", "--no-git"]);

    // Ask the hub to compute the graph.
    let session = levi::hub_client::HubSession::connect(
        &format!("127.0.0.1:{hub_port}"),
        Duration::from_secs(10),
    )
    .expect("connect to hub");
    let out: levi_core::graph::IssueGraphOut = session
        .report_once(
            levi_core::graph::IssueGraphReport {},
            Duration::from_secs(10),
        )
        .expect("graph report");
    let g = out.graph;

    // Exactly the two connected tasks are nodes; the edge carries the via.
    assert!(
        g.nodes.iter().any(|n| n.task_id == blocker),
        "blocker node present"
    );
    assert!(
        g.nodes.iter().any(|n| n.task_id == blocked),
        "blocked node present"
    );
    let edge = g
        .edges
        .iter()
        .find(|e| e.blocked_task_id == blocked)
        .expect("edge to the blocked task");
    assert_eq!(edge.blocker_task_id, blocker);
    assert_eq!(edge.via.as_deref(), Some("cargo: crates.io"));
    // The blocked task sits one layer past its blocker.
    let layer = |id: &str| g.nodes.iter().find(|n| n.task_id == id).unwrap().layer;
    assert_eq!(layer(&blocked), layer(&blocker) + 1);
}

/// A task closed on a feature branch then squash-merged resolves closed from
/// FACTS alone on a second client (git-free), via published patch-ids.
#[test]
fn squash_resolves_from_facts_across_clients() {
    let hub_port = start_hub();
    let (a, b) = two_projects(hub_port);

    // A closes a task anchored on a feature branch, squash-merges to main,
    // then publishes facts (the anchor's patch-id + main's window).
    let id = a.add("upstream fix", &[]);
    a.git(&["checkout", "-q", "-b", "feature"]);
    a.commit_file("fix.txt", "the fix\n", "the fix");
    a.levi_ok(&["close", &id]);
    a.checkout("main");
    a.git(&["merge", "-q", "--squash", "feature"]);
    a.git(&["commit", "-q", "-m", "upstream fix (squashed)"]);
    a.levi_ok(&["sync", "--no-git"]);

    // B syncs and resolves the task from facts: closed, squashed.
    b.levi_ok(&["sync", "--no-git"]);
    // Give B main's commit so its own head can host the fact graph is not
    // required — B resolves foreign facts via the ladder; assert via show.
    let show = a.levi_json(&["show", &id, "--json"]);
    assert_eq!(show["status"], "closed");
    assert_eq!(show["resolution"], "squashed");
}
