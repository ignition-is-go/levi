//! Sync convergence (spec §Testing): two repos with no shared git remote,
//! one in-process hub; mutate both offline, sync via the hub leg, assert
//! byte-identical materialized state. Plus `levi watch` streaming.

mod common;

use std::io::BufRead;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use common::TestRepo;
use common::start_hub_exclusive;
use serde_json::Value;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// In-process, in-memory hub (CellServer without a front door — token
/// enforcement is covered by levi-hub's own tests).

/// Second repo bootstrapped from the first via a one-time git fetch of the
/// events ref (fresh-clone flow); afterwards they share only the hub.
fn bootstrap_pair(hub_port: u16) -> (TestRepo, TestRepo) {
    let a = TestRepo::new();
    let b = TestRepo::new();
    a.init();
    let hub = format!("127.0.0.1:{hub_port}");
    a.set_hub(&hub);
    b.set_hub(&hub);
    b.git(&[
        "fetch",
        a.path().to_str().unwrap(),
        "+refs/levi/events:refs/levi/events",
    ]);
    (a, b)
}

#[test]
fn hub_leg_converges_two_disconnected_repos() {
    let __hub = start_hub_exclusive();
    let hub_port = __hub.port;
    let (a, b) = bootstrap_pair(hub_port);

    // Diverge offline.
    let closed_id = a.add("a: will close", &[]);
    a.add("a: stays open", &["-p", "p1"]);
    a.commit("fixing commit for a");
    a.levi_ok(&["close", &closed_id]);
    b.add("b: from the other side", &["-l", "remote"]);

    // Hub-only sync rounds: A push, B pull+push, A pull.
    a.levi_ok(&["sync", "--no-git"]);
    b.levi_ok(&["sync", "--no-git"]);
    a.levi_ok(&["sync", "--no-git"]);

    // Identical event sets in both refs…
    let tree_a = a.git(&["ls-tree", "-r", "refs/levi/events"]);
    let tree_b = b.git(&["ls-tree", "-r", "refs/levi/events"]);
    assert_eq!(tree_a, tree_b, "event blob sets must be identical");

    // …and byte-identical materialized JSON, resolved per-checkout: B lacks
    // A's fixing commit, so the closed task resolves open+partial there —
    // compare state on --branch-independent fields via --all listing from A
    // twice (A vs A) and structurally between A and B.
    let ls_a = a.levi_json(&["ls", "--json", "--all"]);
    let ls_b = b.levi_json(&["ls", "--json", "--all"]);
    assert_eq!(ls_a["tasks"].as_array().unwrap().len(), 3);
    assert_eq!(ls_b["tasks"].as_array().unwrap().len(), 3);
    let ids = |ls: &Value| {
        let mut v: Vec<String> = ls["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(ids(&ls_a), ids(&ls_b));

    // Ancestry semantics survive the hub: B doesn't have the fixing commit,
    // so the task A closed is open-but-partial on B.
    let closed_on_b = ls_b["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(closed_id.as_str()))
        .unwrap();
    assert_eq!(closed_on_b["status"], "open");
    assert_eq!(closed_on_b["resolution"], "partial");
    let closed_on_a = ls_a["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(closed_id.as_str()))
        .unwrap();
    assert_eq!(closed_on_a["status"], "closed");

    // The offline edit convergence check: same edit target from both sides,
    // later timestamp wins on both.
    a.levi_ok(&["edit", &closed_id, "--title", "renamed by a"]);
    std::thread::sleep(Duration::from_millis(50));
    b.levi_ok(&["edit", &closed_id, "--title", "renamed by b"]);
    a.levi_ok(&["sync", "--no-git"]);
    b.levi_ok(&["sync", "--no-git"]);
    a.levi_ok(&["sync", "--no-git"]);
    let title = |repo: &TestRepo| {
        repo.levi_json(&["show", &closed_id, "--json"])["title"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(title(&a), "renamed by b");
    assert_eq!(title(&b), "renamed by b");
}

#[test]
fn facts_reach_the_hub() {
    let __hub = start_hub_exclusive();
    let hub_port = __hub.port;
    let (a, _b) = bootstrap_pair(hub_port);
    let id = a.add("anchored", &[]);
    a.commit("anchor commit");
    a.levi_ok(&["close", &id]);
    a.levi_ok(&["sync", "--no-git"]);

    // Ask the hub directly for facts.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let client = myko::client::MykoClient::new();
        client.set_address(Some(format!("ws://127.0.0.1:{hub_port}/myko")));
        let deadline = Instant::now() + Duration::from_secs(10);
        let ref_facts = client.watch_query(levi_core::GetAllRefFacts {});
        let commit_facts = client.watch_query(levi_core::GetAllCommitFacts {});
        loop {
            let refs = myko::hyphae::Gettable::get(&ref_facts);
            let commits = myko::hyphae::Gettable::get(&commit_facts);
            let main_ok = refs.iter().any(|r| r.branch == "main");
            // initial + anchor + close-time commits >= 2 facts
            if main_ok && commits.len() >= 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "facts missing: refs={refs:?} commits={}",
                commits.len()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

#[test]
fn watch_streams_new_events() {
    let __hub = start_hub_exclusive();
    let hub_port = __hub.port;
    let (a, b) = bootstrap_pair(hub_port);
    a.levi_ok(&["sync", "--no-git"]);
    b.levi_ok(&["sync", "--no-git"]);

    // Start watch on B (talks to the hub).
    let bin = assert_cmd::cargo::cargo_bin("levi");
    let mut watch = std::process::Command::new(&bin)
        .current_dir(b.path())
        .env("LEVI_CONFIG", b.path().join("levi-test-config.toml"))
        .args(["watch", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let _ = tx.send(line);
        }
    });

    // Give the subscription a moment, then produce an event from A.
    std::thread::sleep(Duration::from_millis(1500));
    a.add("news flash", &[]);
    a.levi_ok(&["sync", "--no-git"]);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_task = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let v: Value = serde_json::from_str(&line).expect("watch emits JSON lines");
                assert_eq!(v["schema"], "levi.watch/1");
                if v["item_type"] == "Task" && v["item"]["title"].as_str() == Some("news flash") {
                    saw_task = true;
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    let _ = watch.kill();
    let _ = watch.wait();
    assert!(saw_task, "watch never streamed the new task event");
}
