# Safe Init + Uninitialized-Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a fresh clone, `levi next`/`levi ls` auto-fetch the events ref, and `levi init` adopts an existing remote project instead of forking one; `init` absorbs `onboard` into one idempotent setup command.

**Architecture:** A blocking best-effort `recover_uninitialized` helper in `levi/src/sync.rs` reuses the existing `git_leg`/`hub_leg`; `next`/`ls` call it from their `uninitialized()` branches. `init` gains a `git ls-remote --exit-code` probe with four outcomes (no remote → mint, ref absent → mint, ref present → fetch+adopt, unreachable → bail), then absorbs onboard's hub-recording and instruction-writing steps. `onboard` survives as a hidden clap alias.

**Tech Stack:** Rust (edition 2024), clap, gix + `git` CLI transport, assert_cmd/predicates integration tests.

**Spec:** `docs/superpowers/specs/2026-07-20-next-sync-recovery-design.md`

## Global Constraints

- Conventional commits required (release workflow derives semver). No AI attribution anywhere.
- `--no-sync` (existing global flag) must always mean "never touch the network": it skips recovery and the init probe.
- No network I/O on any initialized read path: recovery/probe code runs only when `ctx.uninitialized()` (reads) or `world.project.is_none()` (init).
- stdout is reserved for command output (`--json` must stay parseable); all recovery/sync notices go to stderr.
- Every failure of read-path recovery degrades to today's message with exit 0; never a new hard error.
- The test harness's `levi_in` injects `--no-sync` — sync-dependent tests must use the `levi_syncing` helper added in Task 1.
- Run tests per-task with `cargo test -p levi --test <file>`; full `cargo test --workspace` before finishing a task's commit is not required until Task 5.

---

### Task 1: `recover_uninitialized` + wire into `levi next`

**Files:**
- Modify: `levi/src/sync.rs` (append new function at end)
- Modify: `levi/src/commands/next.rs:18-25`
- Modify: `levi/tests/common/mod.rs` (new helper after `levi_in`, ~line 88)
- Test: `levi/tests/cli_git_sync.rs`

**Interfaces:**
- Consumes: `sync::git_leg(&LeviCtx) -> Result<String>`, `sync::hub_leg(&LeviCtx) -> Result<Option<String>>`, `LeviCtx::{reload, uninitialized, no_sync}` — all existing.
- Produces: `pub fn recover_uninitialized(ctx: &mut LeviCtx) -> bool` in `levi/src/sync.rs` (Tasks 2 uses it); `TestRepo::levi_syncing(&self, cwd: PathBuf, args: &[&str]) -> assert_cmd::Command` (Tasks 2–4 use it).

- [ ] **Step 1: Add the `levi_syncing` test helper**

In `levi/tests/common/mod.rs`, directly below `levi_in`:

```rust
    /// Like `levi_in`, but WITHOUT the default `--no-sync` — for tests that
    /// exercise sync-dependent behavior (recovery, init adoption). Same
    /// hermetic config/state env.
    pub fn levi_syncing(&self, cwd: PathBuf, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
        cmd.current_dir(cwd).args(args);
        cmd.env("LEVI_CONFIG", self.path().join("levi-test-config.toml"));
        cmd.env("LEVI_STATE_DIR", self.path().join("levi-test-state"));
        cmd
    }
```

- [ ] **Step 2: Write the failing tests**

Append to `levi/tests/cli_git_sync.rs`:

```rust
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
    assert_eq!(tasks.len(), 1, "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(tasks[0]["title"], "remote task");
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p levi --test cli_git_sync next_ -- --nocapture`
Expected: `next_recovers_events_from_remote_on_fresh_clone` FAILS (0 tasks — no recovery exists yet). The other two PASS already (they assert current behavior; they are regression guards).

- [ ] **Step 4: Implement `recover_uninitialized`**

Append to `levi/src/sync.rs`:

```rust
/// Blocking recovery for read commands on a fresh clone: when the events
/// ref is absent locally, run the git leg once and reload. Hub-based
/// recovery is impossible while uninitialized (the hub is queried by
/// project id, which we don't know yet) — but once the git leg lands
/// events, the hub leg tops up best-effort. Failures degrade to a stderr
/// note and `false`; stdout stays reserved for command output.
pub fn recover_uninitialized(ctx: &mut LeviCtx) -> bool {
    if ctx.no_sync || !ctx.uninitialized() {
        return false;
    }
    if let Err(e) = git_leg(ctx) {
        eprintln!("levi: sync attempt failed: {e:#}");
    }
    if ctx.reload().is_err() || ctx.uninitialized() {
        return false;
    }
    if let Err(e) = hub_leg(ctx) {
        eprintln!("levi: hub sync failed: {e:#}");
    }
    let _ = ctx.reload();
    eprintln!("levi: fetched events via sync (repo had none locally)");
    true
}
```

- [ ] **Step 5: Wire into `next` and converge its message on the `ls` wording**

In `levi/src/commands/next.rs`, replace lines 19–25:

```rust
    if ctx.uninitialized() && !crate::sync::recover_uninitialized(ctx) {
        if json {
            println!("{}", json!({"schema": SCHEMA_NEXT, "tasks": []}));
        }
        eprintln!(
            "no levi events here. Run `levi init`, or fetch existing events with \
             `git fetch <remote> '+refs/levi/events:refs/levi/events'` (or `levi sync`)."
        );
        return Ok(());
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p levi --test cli_git_sync -- --nocapture`
Expected: all tests in the file PASS (including the four pre-existing ones).

- [ ] **Step 7: Commit**

```bash
git add levi/src/sync.rs levi/src/commands/next.rs levi/tests/common/mod.rs levi/tests/cli_git_sync.rs
git commit -m "feat: levi next auto-fetches the events ref on a fresh clone"
```

---

### Task 2: Wire recovery into `levi ls`

**Files:**
- Modify: `levi/src/commands/ls.rs:45-56` (and the `run` signature)
- Modify: `levi/src/main.rs:36` (pass `&mut ctx`)
- Test: `levi/tests/cli_git_sync.rs`

**Interfaces:**
- Consumes: `crate::sync::recover_uninitialized(&mut LeviCtx) -> bool` (Task 1).
- Produces: `ls::run(ctx: &mut LeviCtx, opts: LsOpts)` — signature change consumed by `main.rs` only.

- [ ] **Step 1: Write the failing test**

Append to `levi/tests/cli_git_sync.rs`:

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p levi --test cli_git_sync ls_recovers -- --nocapture`
Expected: FAIL — 0 tasks (ls does not recover yet).

- [ ] **Step 3: Change `ls::run` to `&mut` and call recovery**

In `levi/src/commands/ls.rs`, change the signature and the uninitialized branch:

```rust
pub fn run(ctx: &mut LeviCtx, opts: LsOpts) -> Result<()> {
    if ctx.uninitialized() && !crate::sync::recover_uninitialized(ctx) {
        if opts.json {
            println!("{}", json!({"schema": SCHEMA_LS, "tasks": []}));
        }
        eprintln!(
            "no levi events here. Run `levi init`, or fetch existing events with \
             `git fetch <remote> '+refs/levi/events:refs/levi/events'` (or `levi sync`)."
        );
        return Ok(());
    }
```

(The message text is unchanged; only the guard's shape changes.)

In `levi/src/main.rs:36`, change `commands::ls::run(&ctx, ...)` to `commands::ls::run(&mut ctx, ...)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p levi --test cli_git_sync -- --nocapture`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add levi/src/commands/ls.rs levi/src/main.rs levi/tests/cli_git_sync.rs
git commit -m "feat: levi ls auto-fetches the events ref on a fresh clone"
```

---

### Task 3: Safe `init` — adopt an existing remote project before minting

**Files:**
- Modify: `levi/src/commands/init.rs`
- Modify: `levi/src/main.rs:18` (pass `&mut ctx`)
- Test: `levi/tests/cli_git_sync.rs`

**Interfaces:**
- Consumes: `crate::sync::git_leg`, `crate::store::EVENTS_REF`, `LeviCtx::{reload, no_sync}`, `ctx.config.remote` (`String`, default `"origin"`).
- Produces: `init::run(ctx: &mut LeviCtx, name: Option<String>) -> Result<()>`; private `fn adopt_from_remote(ctx: &mut LeviCtx) -> Result<bool>` (Task 4 keeps it verbatim).

- [ ] **Step 1: Write the failing tests**

Append to `levi/tests/cli_git_sync.rs`:

```rust
#[test]
fn init_adopts_existing_project_from_remote() {
    let (base, a, b) = two_clones();
    let a_out = base
        .levi_in(a.clone(), &["init", "--name", "shared"])
        .output()
        .unwrap();
    assert!(a_out.status.success());
    // "initialized levi project 'shared' (<id>)"
    let a_stdout = String::from_utf8_lossy(&a_out.stdout).to_string();
    let a_id = a_stdout
        .split('(')
        .nth(1)
        .unwrap()
        .trim_end()
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
    assert!(
        stdout.contains("joined existing levi project 'shared'"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains(&a_id), "ids must match — stdout: {stdout}");
}

#[test]
fn init_mints_when_remote_has_no_events_ref() {
    let (base, _a, b) = two_clones();
    let out = base.levi_syncing(b.clone(), &["init"]).output().unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("initialized levi project"),
    );
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p levi --test cli_git_sync init_ -- --nocapture`
Expected: `init_adopts_existing_project_from_remote` FAILS (init mints a second project — output says "initialized", not "joined"). `init_bails_when_remote_unreachable_unless_no_sync` FAILS (init succeeds today). `init_mints_when_remote_has_no_events_ref` PASSES (regression guard).

- [ ] **Step 3: Implement the probe + adopt path**

Replace `levi/src/commands/init.rs` lines 1–20 (imports and `run`; `create_project` stays):

```rust
use std::process::Command;

use anyhow::{Context, Result, bail};
use levi_core::Project;

use crate::ctx::LeviCtx;

pub fn run(ctx: &mut LeviCtx, name: Option<String>) -> Result<()> {
    if let Some(p) = &ctx.world.project {
        bail!(
            "levi project already initialized here: {} ({})",
            p.name,
            p.id.0
        );
    }
    if adopt_from_remote(ctx)? {
        return Ok(());
    }
    let project = create_project(ctx, name)?;
    println!(
        "initialized levi project '{}' ({})",
        project.name, project.id.0
    );
    Ok(())
}

/// Probe the configured git remote for an existing events ref and adopt it
/// rather than minting a fork of the project (spec: safe init).
/// Ok(true): events fetched, world reloaded, "joined" printed.
/// Ok(false): nothing to adopt — no remote, ref absent, or `--no-sync`.
/// Err: remote unreachable, or a fetch that failed after a successful
/// probe. Never falls through to minting on Err: minting when we know (or
/// cannot know) whether events exist is the fork footgun this prevents.
fn adopt_from_remote(ctx: &mut LeviCtx) -> Result<bool> {
    if ctx.no_sync {
        return Ok(false);
    }
    let repo_dir = ctx
        .store
        .repo()
        .workdir()
        .unwrap_or_else(|| ctx.store.repo().git_dir())
        .to_path_buf();
    let remote = ctx.config.remote.clone();
    let has_remote = Command::new("git")
        .args(["remote", "get-url", &remote])
        .current_dir(&repo_dir)
        .output()
        .context("running git")?;
    if !has_remote.status.success() {
        return Ok(false);
    }
    let probe = Command::new("git")
        .args(["ls-remote", "--exit-code", &remote, crate::store::EVENTS_REF])
        .current_dir(&repo_dir)
        .output()
        .context("running git ls-remote")?;
    match probe.status.code() {
        // --exit-code: 2 = remote reachable, no matching ref.
        Some(2) => Ok(false),
        Some(0) => {
            crate::sync::git_leg(ctx)
                .context("remote has levi events but fetching them failed")?;
            ctx.reload()?;
            match &ctx.world.project {
                Some(p) => {
                    println!("joined existing levi project '{}' ({})", p.name, p.id.0);
                    Ok(true)
                }
                None => bail!(
                    "fetched events from '{remote}' but found no project event; \
                     refusing to mint a new project over them"
                ),
            }
        }
        _ => bail!(
            "cannot reach remote '{remote}' to check for an existing levi project: {}\n\
             retry when online, or pass --no-sync to initialize a standalone project",
            String::from_utf8_lossy(&probe.stderr).trim()
        ),
    }
}
```

In `levi/src/main.rs:18`, change to:

```rust
        Cmd::Init { name } => commands::init::run(&mut ctx, name),
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p levi --test cli_git_sync -- --nocapture` then `cargo test -p levi`
Expected: all PASS. (`cli_basic`/`cross_project` init calls run under `--no-sync` via the harness, so the probe is skipped and existing behavior is unchanged.)

- [ ] **Step 5: Commit**

```bash
git add levi/src/commands/init.rs levi/src/main.rs levi/tests/cli_git_sync.rs
git commit -m "feat: levi init adopts an existing remote project instead of forking one"
```

---

### Task 4: Merge `onboard` into `init` (idempotent single setup command)

**Files:**
- Modify: `levi/src/cli.rs:19-40` (Init gains onboard's args + alias; Onboard variant removed)
- Modify: `levi/src/commands/init.rs` (absorb onboard's run body)
- Delete: `levi/src/commands/onboard.rs`
- Modify: `levi/src/commands/mod.rs:9` (drop `pub mod onboard;`)
- Modify: `levi/src/main.rs:18-19` (single Init arm)
- Modify: `levi/src/commands/sync_cmd.rs:22`, `levi/src/commands/watch.rs:20`, `levi/src/foreign.rs:26` (`levi onboard --hub` → `levi init --hub`)
- Test: `levi/tests/cli_basic.rs`

**Interfaces:**
- Consumes: `adopt_from_remote` and `create_project` from Task 3 (unchanged); onboard's `instructions()`/BEGIN/END block logic and `crate::config::write_repo_hub(&root, hub) -> Result<PathBuf>` (moved, not rewritten).
- Produces: `init::run(ctx: &mut LeviCtx, name: Option<String>, hub: Option<String>, files: Vec<PathBuf>) -> Result<()>` — final signature.

- [ ] **Step 1: Update the tests to the merged surface**

In `levi/tests/cli_basic.rs`:

1. Rename `onboard_sets_up_repo_and_agent_instructions` → `init_sets_up_repo_and_agent_instructions` and replace every `"onboard"` arg with `"init"` inside it (five call sites).
2. Replace the init-twice test at lines 11–16 (idempotency instead of bailing):

```rust
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
    // Re-running is idempotent setup, not an error.
    let out = repo.levi_ok(&["init"]);
    assert!(out.contains("already initialized"), "got: {out}");
```

3. Append an alias regression test:

```rust
#[test]
fn onboard_alias_still_works() {
    let repo = TestRepo::new();
    let out = repo.levi_ok(&["onboard"]);
    assert!(out.contains("initialized levi project"), "got: {out}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p levi --test cli_basic -- --nocapture`
Expected: FAIL — `init` doesn't accept the setup surface / bails on re-run (`--hub` unknown arg, "already initialized" is an error).

- [ ] **Step 3: Merge the CLI surface**

In `levi/src/cli.rs`, replace the `Init` and `Onboard` variants with one:

```rust
    /// Set this repo up for levi: adopt the project from the remote when
    /// one exists (mint it otherwise), record the hub address in
    /// .levi/config.toml, and write task-tracking instructions for coding
    /// agents into CLAUDE.md / AGENTS.md. Idempotent.
    #[command(alias = "onboard")]
    Init {
        /// Project name (defaults to the repository directory name).
        #[arg(long)]
        name: Option<String>,
        /// Hub address to record in .levi/config.toml (e.g. hub.example.com:7377).
        #[arg(long)]
        hub: Option<String>,
        /// Instruction file(s) to write; defaults to every existing
        /// CLAUDE.md / AGENTS.md, or a new AGENTS.md if neither exists.
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
    },
```

In `levi/src/main.rs`, replace the two arms with:

```rust
        Cmd::Init { name, hub, files } => commands::init::run(&mut ctx, name, hub, files),
```

- [ ] **Step 4: Absorb onboard's body into `init::run`**

Move `BEGIN`, `END`, and `instructions()` from `onboard.rs` into `init.rs` unchanged, then replace `init::run` with (keep `adopt_from_remote` and `create_project` as-is):

```rust
use std::path::PathBuf;

pub fn run(
    ctx: &mut LeviCtx,
    name: Option<String>,
    hub: Option<String>,
    files: Vec<PathBuf>,
) -> Result<()> {
    // 1. Project: reuse what's here, adopt from the remote, or mint.
    if let Some(p) = &ctx.world.project {
        println!("levi project already initialized: {} ({})", p.name, p.id.0);
    } else if !adopt_from_remote(ctx)? {
        let project = create_project(ctx, name)?;
        println!(
            "initialized levi project '{}' ({})",
            project.name, project.id.0
        );
    }

    let root = ctx
        .store
        .repo()
        .workdir()
        .context("levi init needs a worktree (not a bare repo)")?
        .to_path_buf();

    // 2. Hub address: recorded in .levi/config.toml so it travels with the
    //    repo (committed, one clone away from working).
    if let Some(hub) = &hub {
        let path = crate::config::write_repo_hub(&root, hub)?;
        println!("hub {} recorded in {}", hub, path.display());
    }

    // 3. Agent instructions: explicit --file targets win; otherwise update
    //    every existing CLAUDE.md/AGENTS.md, creating AGENTS.md if neither
    //    exists.
    let targets: Vec<PathBuf> = if files.is_empty() {
        let existing: Vec<PathBuf> = ["CLAUDE.md", "AGENTS.md"]
            .iter()
            .map(|f| root.join(f))
            .filter(|p| p.exists())
            .collect();
        if existing.is_empty() {
            vec![root.join("AGENTS.md")]
        } else {
            existing
        }
    } else {
        files
            .into_iter()
            .map(|f| if f.is_absolute() { f } else { root.join(f) })
            .collect()
    };

    let block = instructions();
    for path in &targets {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        let updated = match (current.find(BEGIN), current.find(END)) {
            // Replace the existing block in place.
            (Some(start), Some(end)) if end > start => {
                let mut s = String::with_capacity(current.len());
                s.push_str(&current[..start]);
                s.push_str(&block);
                s.push_str(&current[end + END.len()..]);
                s
            }
            // Corrupted markers: never guess at what to keep — appending a
            // fresh block would leave the stale partial one in place.
            (Some(_), _) | (None, Some(_)) => anyhow::bail!(
                "{} has a broken levi instruction block ({BEGIN} / {END} \
                 mismatched); fix or remove the markers and re-run",
                path.display()
            ),
            _ if current.is_empty() => format!("{block}\n"),
            _ => {
                let mut s = current.clone();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
                s.push_str(&block);
                s.push('\n');
                s
            }
        };
        std::fs::write(path, updated)
            .with_context(|| format!("cannot write {}", path.display()))?;
        println!("wrote task-tracking instructions to {}", path.display());
    }

    if hub.is_none() && ctx.config.hub.is_none() {
        println!(
            "tip: point this repo at a hub for cross-machine sync and dashboards: \
             `levi init --hub <host:port>` (writes .levi/config.toml)"
        );
    }
    Ok(())
}
```

Then: delete `levi/src/commands/onboard.rs`, remove `pub mod onboard;` from `levi/src/commands/mod.rs`, and update the three stale messages (`sync_cmd.rs:22`, `watch.rs:20`, `foreign.rs:26`) from `levi onboard --hub` to `levi init --hub`. Update `init.rs`'s module doc comment to describe the merged command.

- [ ] **Step 5: Run the full package tests**

Run: `cargo test -p levi`
Expected: all PASS. Watch specifically for `cli_basic` (migrated tests + alias) and `cli_git_sync` (Task 1–3 tests still green — `init_adopts_existing_project_from_remote` now also writes an AGENTS.md in clone-b; the assertions don't check for its absence, so no change needed).

- [ ] **Step 6: Commit**

```bash
git add -A levi/src levi/tests/cli_basic.rs
git commit -m "feat: merge levi onboard into an idempotent levi init (onboard stays as a hidden alias)"
```

---

### Task 5: Documentation sweep + full verification

**Files:**
- Modify: `README.md:23-25` and `README.md:54-56`
- Modify: `docs/superpowers/plans/2026-07-18-levi-implementation.md` (top-of-file deviations list — add one line)

**Interfaces:** none (docs only).

- [ ] **Step 1: Update README**

At `README.md:23-25`, replace the onboard lines:

```sh
levi init [--hub host:port]          # adopt-or-create the project + agent
                                     # instructions (AGENTS.md / CLAUDE.md)
                                     # + hub in .levi/config.toml
```

At `README.md:54-56` (hub leg bullet), change `levi onboard --hub <host:port>` to `levi init --hub <host:port>`.

Check for stragglers: `grep -rn onboard README.md docs/ levi/src/` — the only remaining hits should be the spec/plan documents for this feature and the historical 2026-07-18 spec/plan (leave historical docs as-is).

- [ ] **Step 2: Record the spec deviation note**

The 2026-07-18 plan's header lists deviations from the original spec. Add:

```markdown
- `levi onboard` merged into `levi init` (hidden alias retained); init
  adopts an existing remote project instead of minting a fork, and
  `next`/`ls` auto-fetch the events ref on a fresh clone
  (spec: 2026-07-20-next-sync-recovery-design.md).
```

- [ ] **Step 3: Full workspace verification**

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets`
Expected: all tests PASS, no new clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/superpowers/plans/2026-07-18-levi-implementation.md
git commit -m "docs: point setup instructions at the merged levi init"
```
