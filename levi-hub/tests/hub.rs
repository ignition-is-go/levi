//! Hub integration: token enforcement at the front door, and LogEntry
//! unwrapping into queryable entities via the saga.

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
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start_hub(token: Option<&str>) -> Hub {
    let port = free_port();
    let internal = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_levi-hub"));
    cmd.args([
        "serve",
        "--bind",
        &format!("127.0.0.1:{port}"),
        "--internal-port",
        &internal.to_string(),
    ])
    .env_remove("MYKO_POSTGRES_URL")
    .env_remove("LEVI_HUB_TOKEN");
    if std::env::var_os("DEBUG_HUB").is_some() {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    if let Some(t) = token {
        cmd.args(["--token", t]);
    }
    let child = cmd.spawn().expect("hub starts");
    let hub = Hub { child, port };
    // Wait for the front door to accept connections.
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
    tokio::time::timeout(Duration::from_secs(5), rx).await.map(|r| r.unwrap_or(false)).unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_rejected_correct_token_accepted() {
    let hub = start_hub(Some("sekrit"));

    // Raw WS handshake without the token: rejected with HTTP 401.
    let err = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/myko", hub.port)).await;
    assert!(err.is_err(), "connection without token must fail");

    // Wrong token: also rejected.
    let err =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/myko?token=nope", hub.port))
            .await;
    assert!(err.is_err(), "connection with wrong token must fail");

    // Correct token: handshake succeeds.
    let ok =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/myko?token=sekrit", hub.port))
            .await;
    assert!(ok.is_ok(), "connection with correct token must succeed: {:?}", ok.err());
}

#[tokio::test(flavor = "multi_thread")]
async fn log_entry_saga_unwraps_into_queryable_task() {
    let hub = start_hub(Some("sekrit"));
    let client = MykoClient::new();
    assert!(
        connect(&client, format!("ws://127.0.0.1:{}/myko?token=sekrit", hub.port)).await,
        "client connects through the front door"
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
        created_at: "2026-07-18T00:00:00.000000Z".into(),
    };
    let mut inner = MEvent::from_item(&task, MEventType::SET, "m");
    inner.created_at = task.created_at.clone();
    let mut cbor = Vec::new();
    ciborium::into_writer(&inner, &mut cbor).unwrap();
    let entry = LogEntry::wrap("aabbccdd00112233aabbccdd00112233aabbccdd", "proj1", &cbor, &task.created_at);

    client.send_event(MEvent::from_item(&entry, MEventType::SET, "test"));

    // The saga should apply the inner event; the task becomes queryable.
    let tasks = client.watch_query(levi_core::GetAllTasks {});
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = tasks.get();
        if got.iter().any(|t| t.title == "hub roundtrip") {
            break;
        }
        assert!(Instant::now() < deadline, "task never appeared via saga unwrap; got {got:?}");
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
