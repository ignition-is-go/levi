//! HubSession lifetime (lv-10ea). Every `?` in the sync legs returns
//! without reaching `close()`, so teardown has to happen on drop: a leaked
//! session keeps its socket and reconnect loop alive, and dropping its
//! runtime from sync context blocks. In the 2026-07-21 hub flood that kept
//! a dead process transmitting for half an hour.

mod common;

use std::sync::mpsc;
use std::time::Duration;

use common::start_hub;
use levi::hub_client::HubSession;

/// Run `f` on a worker thread, failing if it doesn't finish in `limit`.
fn within<F: FnOnce() + Send + 'static>(limit: Duration, what: &str, f: F) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(limit).is_ok(),
        "{what} did not finish in {limit:?}"
    );
}

#[test]
fn dropped_session_tears_down_promptly() {
    let port = start_hub();
    let addr = format!("127.0.0.1:{port}");
    within(
        Duration::from_secs(15),
        "dropping a connected HubSession",
        move || {
            let session = HubSession::connect(&addr, Duration::from_secs(10)).expect("connects");
            // No close(): exactly the shape of an error path returning via `?`.
            drop(session);
        },
    );
}

#[test]
fn dropped_session_survives_a_vanished_hub() {
    // Worst case from the incident: the client is mid-reconnect when it's
    // dropped, so teardown must not wait on the socket coming back.
    let port = start_hub();
    let addr = format!("127.0.0.1:{port}");
    within(
        Duration::from_secs(15),
        "dropping a reconnecting HubSession",
        move || {
            let session = HubSession::connect(&addr, Duration::from_secs(10)).expect("connects");
            drop(session);
        },
    );
}

#[test]
fn explicit_close_still_works() {
    let port = start_hub();
    let addr = format!("127.0.0.1:{port}");
    within(Duration::from_secs(15), "close()", move || {
        let session = HubSession::connect(&addr, Duration::from_secs(10)).expect("connects");
        session.close();
    });
}
