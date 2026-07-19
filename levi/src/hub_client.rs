//! Hub connection for the sync hub leg, facts leg, and `levi watch`.
//! The CLI is synchronous; each session owns a small tokio runtime.
//!

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use myko::client::{ConnectionStatus, MykoClient};
use myko::hyphae::{Gettable, Signal, Watchable};
use myko::prelude::{Eventable, ReportIdStatic};
use myko::query::QueryParams;
use myko::report::ReportParams;
use myko::wire::MEvent;

pub struct HubSession {
    pub client: MykoClient,
    // Keep the runtime alive for the client's background tasks.
    pub runtime: tokio::runtime::Runtime,
}

pub fn ws_url(addr: &str) -> String {
    let base = if addr.starts_with("ws://") || addr.starts_with("wss://") {
        addr.trim_end_matches('/').to_string()
    } else {
        format!("ws://{}", addr.trim_end_matches('/'))
    };
    format!("{base}/myko")
}

impl HubSession {
    pub fn connect(addr: &str, timeout: Duration) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let client = runtime.block_on(async { MykoClient::new() });
        let (tx, rx) = mpsc::channel::<()>();
        let tx = std::sync::Mutex::new(Some(tx));
        let status = client.connection_status();
        let guard = status.subscribe(move |sig| {
            if let Signal::Value(s) = sig
                && matches!(&**s, ConnectionStatus::Connected(_))
                && let Some(tx) = tx.lock().unwrap().take()
            {
                let _ = tx.send(());
            }
        });
        client.set_address(Some(ws_url(addr)));
        let connected = rx.recv_timeout(timeout).is_ok();
        drop(guard);
        if !connected {
            client.set_address(None);
            bail!("could not connect to hub at {addr} within {timeout:?}");
        }
        Ok(Self { client, runtime })
    }

    /// One-shot report: reports answer `None` until the server responds, so
    /// (unlike query cells, which seed subscribers with an empty snapshot)
    /// `Some` is an unambiguous "the hub has answered".
    pub fn report_once<R, O>(&self, report: R, timeout: Duration) -> Result<O>
    where
        R: ReportParams + ReportIdStatic + Clone,
        O: myko::serde::de::DeserializeOwned
            + Clone
            + std::fmt::Debug
            + PartialEq
            + Send
            + Sync
            + 'static,
    {
        let cell = self.client.watch_report::<R, O>(report);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = cell.get() {
                return Ok(v);
            }
            if Instant::now() > deadline {
                bail!("hub did not answer the report in time");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// One-shot query snapshot, made unambiguous by first asking for the
    /// authoritative count via a report, then waiting for the query cell to
    /// catch up to it. (Entities here are append-only, so `>=` is safe.)
    pub fn query_at_least<Q>(
        &self,
        query: Q,
        expected: usize,
        timeout: Duration,
    ) -> Result<Vec<Arc<Q::Item>>>
    where
        Q: QueryParams + Clone,
        Q::Item: Eventable + Clone + std::fmt::Debug + 'static,
    {
        let cell = self.client.watch_query(query);
        let deadline = Instant::now() + timeout;
        loop {
            let items = cell.get();
            if items.len() >= expected {
                return Ok(items.to_vec());
            }
            if Instant::now() > deadline {
                bail!(
                    "hub query returned {} of {expected} expected items in time",
                    items.len()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// This project's LogEntries as the hub currently knows them: the count
    /// report is the "hub has answered" marker, then the ByQuery cell catches
    /// up to it. (levi entity fields deliberately avoid the wire keys
    /// `QueryRequest` flattens beside the query — `createdAt`/`tx` — which is
    /// why entities use `created`, not `created_at`.)
    pub fn log_entries(
        &self,
        project_id: &str,
        timeout: Duration,
    ) -> Result<Vec<Arc<levi_core::LogEntry>>> {
        let filter = levi_core::PartialLogEntry {
            project_id: Some(project_id.to_string()),
            ..Default::default()
        };
        let count: levi_core::LogEntryCount =
            self.report_once(levi_core::CountLogEntrys(filter.clone()), timeout)?;
        self.query_at_least(levi_core::GetLogEntrysByQuery(filter), count.count, timeout)
    }

    /// Send events and barrier on a round-trip so they're processed before we
    /// return (messages on one connection are handled in order).
    pub fn send_events(&self, events: Vec<MEvent>) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let _ = self.client.send_event_batch(events);
        // Round-trip barrier: a report response means the batch was consumed.
        let _: levi_core::ProjectCount =
            self.report_once(levi_core::CountAllProjects {}, Duration::from_secs(10))?;
        Ok(())
    }

    pub fn close(self) {
        self.client.set_address(None);
        // Give the socket a beat to flush the close frame.
        self.runtime.shutdown_timeout(Duration::from_millis(500));
    }
}
