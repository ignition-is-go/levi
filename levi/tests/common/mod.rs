#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Start an in-process myko hub on a free port; returns the port.
pub fn start_hub() -> u16 {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return port;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("in-process hub did not start");
}

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

    /// Commit a real file change (patch-id matching needs non-empty diffs).
    pub fn commit_file(&self, name: &str, content: &str, msg: &str) -> String {
        std::fs::write(self.path().join(name), content).unwrap();
        self.git(&["add", name]);
        self.git(&["commit", "-q", "-m", msg]);
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
        // Hermetic config/state: no user file leakage (CI has no real
        // sibling checkouts; tests fabricate their own).
        cmd.env("LEVI_CONFIG", self.path().join("levi-test-config.toml"));
        cmd.env("LEVI_STATE_DIR", self.path().join("levi-test-state"));
        cmd
    }

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
