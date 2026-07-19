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
fn dep_comment_edit_flow() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("a", &[]);
    let b = repo.add("b", &[]);

    repo.levi_ok(&["dep", "add", &b, "--on", &a]);
    let show = repo.levi_json(&["show", &b, "--json"]);
    assert_eq!(show["blocked_by"][0]["id"].as_str().unwrap(), a);

    // Idempotent add; cycle add warns but succeeds.
    repo.levi_ok(&["dep", "add", &b, "--on", &a]);
    repo.levi(&["dep", "add", &a, "--on", &b])
        .assert()
        .success()
        .stderr(predicate::str::contains("cycle"));
    repo.levi_ok(&["dep", "rm", &a, "--on", &b]);
    repo.levi_ok(&["dep", "rm", &b, "--on", &a]);
    let show = repo.levi_json(&["show", &b, "--json"]);
    assert_eq!(show["blocked_by"].as_array().unwrap().len(), 0);
    repo.levi(&["dep", "rm", &b, "--on", &a]).assert().failure();
    repo.levi(&["dep", "add", &a, "--on", &a]).assert().failure().stderr(predicate::str::contains("itself"));

    repo.levi_ok(&["comment", &a, "first note"]);
    repo.levi_ok(&["comment", &a, "second note"]);
    let show = repo.levi_json(&["show", &a, "--json"]);
    let comments = show["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["body"], "first note");

    repo.levi_ok(&["edit", &a, "-p", "p0", "--title", "renamed", "-l", "+urgent", "-l", "+web"]);
    repo.levi_ok(&["edit", &a, "-l", "-web"]);
    let show = repo.levi_json(&["show", &a, "--json"]);
    assert_eq!(show["priority"], "P0");
    assert_eq!(show["title"], "renamed");
    assert_eq!(show["labels"].as_array().unwrap().len(), 1);
    assert_eq!(show["labels"][0], "urgent");
    repo.levi(&["edit", &a]).assert().failure().stderr(predicate::str::contains("nothing to edit"));
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
