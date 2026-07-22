mod common;

use common::TestRepo;
use predicates::prelude::*;

#[test]
fn init_add_ls_show_roundtrip() {
    let repo = TestRepo::new();

    // init prints the project id; re-running is idempotent setup, not an error.
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("already initialized"), "got: {out}");

    let id = repo.add(
        "fix the flux capacitor",
        &["-p", "p1", "-l", "engine", "-b", "it broke"],
    );
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
    repo.levi(&["show", "lv-ffff"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no task matches"));
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
    repo.levi(&["ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains("levi init"));
    let ls = repo.levi_json(&["ls", "--json"]);
    assert_eq!(ls["tasks"].as_array().unwrap().len(), 0);
    // Mutating commands error.
    repo.levi(&["add", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("levi init"));
}

#[test]
fn init_sets_up_repo_and_agent_instructions() {
    let repo = TestRepo::new();

    // Fresh repo, no CLAUDE.md/AGENTS.md: initializes + creates AGENTS.md
    // and records the hub in .levi/config.toml.
    let out = repo.levi_ok(&["init", "--hub", "hub.example.com:7377"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
    let agents = std::fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- levi:begin -->"));
    assert!(agents.contains("levi next --claim"));
    assert!(agents.contains("levi close"));
    let config = std::fs::read_to_string(repo.path().join(".levi/config.toml")).unwrap();
    assert!(config.contains("hub.example.com:7377"), "got: {config}");

    // Re-run: idempotent — project kept, exactly one instruction block.
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("already initialized"), "got: {out}");
    let agents = std::fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert_eq!(agents.matches("<!-- levi:begin -->").count(), 1);

    // Existing CLAUDE.md content is preserved; the block is appended and
    // replaced in place on later runs.
    std::fs::write(
        repo.path().join("CLAUDE.md"),
        "# my project\n\nhand-written notes\n",
    )
    .unwrap();
    repo.levi_ok(&["init"]);
    let claude = std::fs::read_to_string(repo.path().join("CLAUDE.md")).unwrap();
    assert!(claude.starts_with("# my project"));
    assert!(claude.contains("hand-written notes"));
    assert_eq!(claude.matches("<!-- levi:begin -->").count(), 1);
    repo.levi_ok(&["init"]);
    let claude = std::fs::read_to_string(repo.path().join("CLAUDE.md")).unwrap();
    assert_eq!(claude.matches("<!-- levi:begin -->").count(), 1);

    // --hub preserves other keys on rewrite.
    std::fs::write(
        repo.path().join(".levi/config.toml"),
        "[claim]\nttl_secs = 60\n\n[hub]\naddress = \"old:1\"\n",
    )
    .unwrap();
    repo.levi_ok(&["init", "--hub", "new:2"]);
    let config = std::fs::read_to_string(repo.path().join(".levi/config.toml")).unwrap();
    assert!(config.contains("ttl_secs = 60"), "got: {config}");
    assert!(config.contains("new:2"), "got: {config}");
    assert!(!config.contains("old:1"), "got: {config}");
}

#[test]
fn onboard_alias_still_works() {
    let repo = TestRepo::new();
    let out = repo.levi_ok(&["onboard"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
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
    repo.levi(&["dep", "add", &a, "--on", &a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("itself"));

    repo.levi_ok(&["comment", &a, "first note"]);
    repo.levi_ok(&["comment", &a, "second note"]);
    let show = repo.levi_json(&["show", &a, "--json"]);
    let comments = show["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["body"], "first note");

    repo.levi_ok(&[
        "edit", &a, "-p", "p0", "--title", "renamed", "-l", "+urgent", "-l", "+web",
    ]);
    repo.levi_ok(&["edit", &a, "-l", "-web"]);
    let show = repo.levi_json(&["show", &a, "--json"]);
    assert_eq!(show["priority"], "P0");
    assert_eq!(show["title"], "renamed");
    assert_eq!(show["labels"].as_array().unwrap().len(), 1);
    assert_eq!(show["labels"][0], "urgent");
    repo.levi(&["edit", &a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nothing to edit"));
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

#[test]
fn init_works_in_bare_repo() {
    let repo = TestRepo::new();
    let bare = repo.path().join("bare.git");
    repo.git(&["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
    repo.git_in(&bare, &["config", "user.email", "agent@test"]);
    repo.git_in(&bare, &["config", "user.name", "Agent"]);
    let out = repo.levi_in(bare.clone(), &["init"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("initialized levi project"),
        "stdout: {stdout}"
    );
    // No AGENTS.md materialized inside the bare repo.
    assert!(!bare.join("AGENTS.md").exists());
}

/// Closing a task releases the caller's own claim; --no-drop keeps it.
#[test]
fn close_releases_the_claim() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("do a thing", &[]);
    repo.commit("work");

    // Claim it, then close — the claim should be gone.
    repo.levi_ok(&["start", &id]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_object(),
        "claimed before close");
    repo.levi_ok(&["close", &id]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_null(),
        "claim released on close");

    // Reopen + claim + close --no-drop keeps the claim.
    repo.levi_ok(&["reopen", &id]);
    repo.levi_ok(&["start", &id]);
    repo.commit("more");
    repo.levi_ok(&["close", &id, "--no-drop"]);
    assert!(repo.levi_json(&["show", &id, "--json"])["claim"].is_object(),
        "--no-drop keeps the claim");
}
