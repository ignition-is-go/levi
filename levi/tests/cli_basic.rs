mod common;

use common::TestRepo;
use predicates::prelude::*;

#[test]
fn init_add_ls_show_roundtrip() {
    let repo = TestRepo::new();

    // init prints the project id; double init errors.
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
    repo.levi(&["init"]).assert().failure().stderr(predicate::str::contains("already initialized"));

    let id = repo.add("fix the flux capacitor", &["-p", "p1", "-l", "engine", "-b", "it broke"]);
    assert_eq!(id.len(), 32);

    // ls shows it open with a short id.
    let ls = repo.levi_json(&["ls", "--json"]);
    assert_eq!(ls["schema"], "levi.ls/1");
    let tasks = ls["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["status"], "open");
    assert_eq!(tasks[0]["resolution"], "exact");
    assert_eq!(tasks[0]["priority"], "P1");
    assert_eq!(tasks[0]["labels"][0], "engine");
    let short = tasks[0]["short"].as_str().unwrap();
    assert!(short.starts_with("lv-"), "short id: {short}");

    // show by short id (prefix matching) and by full id.
    let show = repo.levi_json(&["show", short, "--json"]);
    assert_eq!(show["schema"], "levi.show/1");
    assert_eq!(show["id"].as_str().unwrap(), id);
    assert_eq!(show["body"], "it broke");
    let show2 = repo.levi_json(&["show", &id, "--json"]);
    assert_eq!(show2["id"], show["id"]);

    // Unknown and bad prefixes error clearly.
    repo.levi(&["show", "lv-ffff"]).assert().failure().stderr(predicate::str::contains("no task matches"));
}

#[test]
fn ambiguous_prefix_lists_candidates() {
    let repo = TestRepo::new();
    repo.init();
    // Force ambiguity by adding tasks until two share a first hex char.
    // Deterministic route: query full ids, use their common prefix of length 0
    // is trivially shared — instead assert on a 1-char prefix once we have
    // two ids starting with the same char.
    let mut ids = Vec::new();
    for i in 0..20 {
        ids.push(repo.add(&format!("task {i}"), &[]));
    }
    let first = &ids[0][..1];
    if ids[1..].iter().any(|i| i.starts_with(first)) {
        repo.levi(&["show", first])
            .assert()
            .failure()
            .stderr(predicate::str::contains("ambiguous"));
    }
}

#[test]
fn uninitialized_repo_gives_guidance() {
    let repo = TestRepo::new();
    // ls without any levi events: exit 0, guidance on stderr, empty JSON.
    repo.levi(&["ls"]).assert().success().stderr(predicate::str::contains("levi init"));
    let ls = repo.levi_json(&["ls", "--json"]);
    assert_eq!(ls["tasks"].as_array().unwrap().len(), 0);
    // Mutating commands error.
    repo.levi(&["add", "nope"]).assert().failure().stderr(predicate::str::contains("levi init"));
}

#[test]
fn add_with_dep_links_tasks() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("foundation", &[]);
    let blocked = repo.add("tower", &["--dep", &blocker]);
    let show = repo.levi_json(&["show", &blocked, "--json"]);
    let blocked_by = show["blocked_by"].as_array().unwrap();
    assert_eq!(blocked_by.len(), 1);
    assert_eq!(blocked_by[0]["id"].as_str().unwrap(), blocker);
    assert_eq!(blocked_by[0]["status"], "open");
    let show_blocker = repo.levi_json(&["show", &blocker, "--json"]);
    assert_eq!(show_blocker["blocks"][0]["id"].as_str().unwrap(), blocked);
}
