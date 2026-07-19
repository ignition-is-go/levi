use std::path::Path;
use std::process::Command;

use levi::store::{EVENTS_REF, EventStore};
use levi_core::Task;
use myko::wire::{MEvent, MEventType};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "agent@test"]);
    git(dir.path(), &["config", "user.name", "Agent"]);
    dir
}

fn task_event(n: u32) -> MEvent {
    let t = Task {
        id: format!("{n:032x}").into(),
        project_id: "p".into(),
        title: format!("task {n}"),
        body: String::new(),
        priority: Default::default(),
        labels: vec![],
        created_by_dev: "d".into(),
        created_by_machine: "m".into(),
        created_at: format!("2026-07-{:02}T00:00:00Z", 1 + n % 28),
    };
    MEvent::from_item(&t, MEventType::SET, "m")
}

#[test]
fn append_and_read_roundtrip() {
    let dir = test_repo();
    let store = EventStore::discover(dir.path()).unwrap();

    assert!(store.read().unwrap().is_empty());

    let ids = store.append(&[task_event(1), task_event(2), task_event(3)]).unwrap();
    assert_eq!(ids.len(), 3);
    let records = store.read().unwrap();
    assert_eq!(records.len(), 3);
    let mut got: Vec<_> = records.iter().map(|r| r.id.clone()).collect();
    got.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(got, want);

    // Blob OID is the content address: git agrees on the bytes.
    let cat = Command::new("git")
        .args(["cat-file", "blob", &ids[0]])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(cat.status.success());
    let event: MEvent = ciborium::from_reader(cat.stdout.as_slice()).unwrap();
    assert_eq!(event.item_type, "Task");

    // Second append parents the first commit.
    store.append(&[task_event(4), task_event(5)]).unwrap();
    assert_eq!(store.read().unwrap().len(), 5);

    // The working tree is untouched.
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");

    // Appending byte-identical events is a no-op (content addressed).
    let ev = task_event(6);
    let first = store.append(std::slice::from_ref(&ev)).unwrap();
    let second = store.append(&[ev]).unwrap();
    assert_eq!(first, second);
    assert_eq!(store.read().unwrap().len(), 6);
}

#[test]
fn concurrent_appends_all_survive() {
    let dir = test_repo();
    let path = dir.path().to_path_buf();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let path = path.clone();
            std::thread::spawn(move || {
                let store = EventStore::discover(&path).unwrap();
                for i in 0..5 {
                    store.append(&[task_event(100 + t * 10 + i)]).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let store = EventStore::discover(&path).unwrap();
    assert_eq!(store.read().unwrap().len(), 40);
}

#[test]
fn merge_ref_unions_histories() {
    let a = test_repo();
    let store_a = EventStore::discover(a.path()).unwrap();
    store_a.append(&[task_event(1), task_event(2)]).unwrap();

    // Clone (bare fetch of the levi ref) into b, then diverge both.
    let b = test_repo();
    let store_b = EventStore::discover(b.path()).unwrap();
    git(
        b.path(),
        &["fetch", a.path().to_str().unwrap(), &format!("{EVENTS_REF}:{EVENTS_REF}")],
    );
    assert_eq!(store_b.read().unwrap().len(), 2);

    store_a.append(&[task_event(3)]).unwrap();
    store_b.append(&[task_event(4)]).unwrap();

    // b fetches a's ref to a tracking ref and union-merges.
    git(
        b.path(),
        &[
            "fetch",
            a.path().to_str().unwrap(),
            &format!("+{EVENTS_REF}:refs/levi/remotes/origin/events"),
        ],
    );
    let new = store_b.merge_ref("refs/levi/remotes/origin/events").unwrap();
    assert_eq!(new, 1);
    assert_eq!(store_b.read().unwrap().len(), 4);

    // Idempotent: merging again adds nothing and creates no new commit.
    let head_before = git(b.path(), &["rev-parse", EVENTS_REF]);
    assert_eq!(store_b.merge_ref("refs/levi/remotes/origin/events").unwrap(), 0);
    assert_eq!(git(b.path(), &["rev-parse", EVENTS_REF]), head_before);
}
