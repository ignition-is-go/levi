//! Hub integration: LogEntry unwrapping into queryable entities via the saga.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use levi_core::{LogEntry, Task};
use myko::client::{ConnectionStatus, MykoClient};
use myko::hyphae::{Gettable, Signal, Watchable};
use myko::wire::{MEvent, MEventType};

struct Hub {
    child: Child,
    port: u16,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_hub() -> Hub {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_levi-hub"));
    cmd.args(["serve", "--bind", &format!("127.0.0.1:{port}")])
        .env_remove("MYKO_POSTGRES_URL");
    if std::env::var_os("DEBUG_HUB").is_some() {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let child = cmd.spawn().expect("hub starts");
    let hub = Hub { child, port };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", hub.port)).is_ok() {
            return hub;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("hub did not start listening");
}

async fn connect(client: &MykoClient, addr: String) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = std::sync::Mutex::new(Some(tx));
    let status = client.connection_status();
    let _guard = status.subscribe(move |sig| {
        if let Signal::Value(s) = sig
            && matches!(&**s, ConnectionStatus::Connected(_))
            && let Some(tx) = tx.lock().unwrap().take()
        {
            let _ = tx.send(true);
        }
    });
    client.set_address(Some(addr));
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map(|r| r.unwrap_or(false))
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn log_entry_saga_unwraps_into_queryable_task() {
    let hub = start_hub();
    let client = MykoClient::new();
    assert!(
        connect(&client, format!("ws://127.0.0.1:{}/myko", hub.port)).await,
        "client connects to the hub"
    );

    // Wrap a Task SET the way the CLI's hub leg does.
    let task = Task {
        id: "cafe0000cafe0000cafe0000cafe0000".into(),
        project_id: "proj1".into(),
        title: "hub roundtrip".into(),
        body: String::new(),
        priority: Default::default(),
        labels: vec![],
        created_by_dev: "d@e".into(),
        created_by_machine: "m".into(),
        created: "2026-07-18T00:00:00.000000Z".into(),
    };
    let mut inner = MEvent::from_item(&task, MEventType::SET, "m");
    inner.created_at = task.created.clone();
    let mut cbor = Vec::new();
    ciborium::into_writer(&inner, &mut cbor).unwrap();
    let entry = LogEntry::wrap(
        "aabbccdd00112233aabbccdd00112233aabbccdd",
        "proj1",
        &cbor,
        &task.created,
    );

    let _ = client.send_event(MEvent::from_item(&entry, MEventType::SET, "test"));

    // The saga should apply the inner event; the task becomes queryable.
    let tasks = client.watch_query(levi_core::GetAllTasks {});
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = tasks.get();
        if got.iter().any(|t| t.title == "hub roundtrip") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "task never appeared via saga unwrap; got {got:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // And the LogEntry itself is queryable (the sync pull path).
    let entries = client.watch_query(levi_core::GetAllLogEntrys {});
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !entries.get().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "log entry not queryable");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_order_events_do_not_clobber_newer_state() {
    let hub = start_hub();
    let client = MykoClient::new();
    assert!(connect(&client, format!("ws://127.0.0.1:{}/myko", hub.port)).await);

    let mk_entry = |title: &str, created: &str, oid: &str| {
        let task = Task {
            id: "feed0000feed0000feed0000feed0000".into(),
            project_id: "projO".into(),
            title: title.into(),
            body: String::new(),
            priority: Default::default(),
            labels: vec![],
            created_by_dev: "d@e".into(),
            created_by_machine: "m".into(),
            created: "2026-07-01T00:00:00.000000Z".into(),
        };
        let mut inner = MEvent::from_item(&task, MEventType::SET, "m");
        inner.created_at = created.to_string();
        let mut cbor = Vec::new();
        ciborium::into_writer(&inner, &mut cbor).unwrap();
        LogEntry::wrap(oid, "projO", &cbor, created)
    };

    // Newer edit arrives first; the older original arrives later (e.g. from
    // a machine that synced late). LWW: the newer title must survive.
    let newer = mk_entry(
        "newer title",
        "2026-07-02T00:00:00.000000Z",
        "bbbb000011112222bbbb000011112222bbbb0000",
    );
    let older = mk_entry(
        "older title",
        "2026-07-01T12:00:00.000000Z",
        "aaaa000011112222aaaa000011112222aaaa0000",
    );

    let _ = client.send_event(MEvent::from_item(&newer, MEventType::SET, "test"));
    // Wait until the newer edit is applied before sending the stale one.
    let tasks = client.watch_query(levi_core::GetAllTasks {});
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if tasks.get().iter().any(|t| t.title == "newer title") {
            break;
        }
        assert!(Instant::now() < deadline, "newer event never applied");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = client.send_event(MEvent::from_item(&older, MEventType::SET, "test"));
    // Give the saga time to (wrongly) apply the stale event, then check.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let titles: Vec<String> = tasks
        .get()
        .iter()
        .filter(|t| &*t.id.0 == "feed0000feed0000feed0000feed0000")
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(
        titles,
        ["newer title"],
        "stale event must not clobber newer state"
    );
}
