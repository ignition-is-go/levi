#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct TestRepo {
    pub dir: TempDir,
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let repo = Self { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "agent@test"]);
        repo.git(&["config", "user.name", "Agent"]);
        repo.commit("initial");
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(self.path(), args)
    }

    pub fn git_in(&self, cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Create an empty commit and return its sha.
    pub fn commit(&self, msg: &str) -> String {
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    pub fn merge(&self, name: &str) {
        self.git(&["merge", "-q", "--no-edit", name]);
    }

    /// A levi command in this repo (or a subdir/worktree via `levi_in`).
    pub fn levi(&self, args: &[&str]) -> assert_cmd::Command {
        self.levi_in(self.path().to_path_buf(), args)
    }

    pub fn levi_in(&self, cwd: PathBuf, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("levi").unwrap();
        cmd.current_dir(cwd).arg("--no-sync").args(args);
        // Hermetic config: no user config file leakage.
        cmd.env("LEVI_CONFIG", self.path().join("levi-test-config.toml"));
        cmd
    }

    /// Run levi expecting success; returns stdout.
    pub fn levi_ok(&self, args: &[&str]) -> String {
        let out = self.levi(args).output().unwrap();
        assert!(
            out.status.success(),
            "levi {args:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// levi ok + parse stdout as JSON.
    pub fn levi_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.levi_ok(args)).expect("valid JSON output")
    }

    /// `levi init` + return the project setup; also returns the task-adding helper.
    pub fn init(&self) {
        self.levi_ok(&["init"]);
    }

    /// Point this repo at a hub via `.levi/config.toml` (the repo-level
    /// config file).
    pub fn set_hub(&self, addr: &str) {
        let dir = self.path().join(".levi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!("[hub]\naddress = \"{addr}\"\n"),
        )
        .unwrap();
    }

    /// Add a task, returning its full id.
    pub fn add(&self, title: &str, extra: &[&str]) -> String {
        let out = self.levi_ok(&[&["add", title], extra].concat());
        out.split_whitespace()
            .nth(1)
            .expect("add prints 'short id'")
            .to_string()
    }
}
