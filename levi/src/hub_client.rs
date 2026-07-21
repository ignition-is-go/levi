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

/// Events per send batch. Bounds every barrier wait by chunk size instead
/// of total batch size; sized so a chunk clears a slow hub's 10s barrier
/// with generous headroom (~5ms/event observed on the hosted hub ⇒ ~1.3s).
pub const SEND_CHUNK: usize = 256;

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

    /// This project's LogEntries as the hub currently knows them (used by
    /// `levi watch` for its history snapshot; the sync hub leg uses the
    /// Merkle bucket walk instead).
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
    /// return (messages on one connection are handled in order). Sent in
    /// SEND_CHUNK-sized chunks, each with its own barrier: the barrier can't
    /// answer until the hub consumes everything ahead of it on the
    /// connection, so a single batch-wide barrier would scale its wait with
    /// batch size and time out on large publications (lv-b69e).
    pub fn send_events(&self, mut events: Vec<MEvent>) -> Result<()> {
        while !events.is_empty() {
            let take = events.len().min(SEND_CHUNK);
            let chunk: Vec<MEvent> = events.drain(..take).collect();
            self.client
                .send_event_batch(chunk)
                .map_err(|e| anyhow::anyhow!("event batch send failed: {e}"))?;
            // Round-trip barrier: a report response means the chunk was
            // consumed.
            let _: levi_core::ProjectCount =
                self.report_once(levi_core::CountAllProjects {}, Duration::from_secs(10))?;
        }
        Ok(())
    }

    /// Push events into a (possibly foreign) project with verification:
    /// wrap as LogEntries (id = content address), send, and confirm the hub
    /// holds every id before returning. Hub-ack-then-forget — the caller
    /// stores nothing locally (spec: cross-project write path).
    pub fn push_events_verified(&self, project_id: &str, events: &[MEvent]) -> Result<Vec<String>> {
        let mut ids = Vec::with_capacity(events.len());
        let mut wrapped = Vec::with_capacity(events.len());
        for event in events {
            let bytes = crate::store::EventStore::encode(event);
            let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &bytes)
                .map_err(|e| anyhow::anyhow!("hashing event: {e}"))?
                .to_string();
            let entry = levi_core::LogEntry::wrap(&oid, project_id, &bytes, &event.created_at);
            wrapped.push(MEvent::from_item(
                &entry,
                myko::wire::MEventType::SET,
                "levi",
            ));
            ids.push(oid);
        }
        self.send_events(wrapped)?;
        self.query_at_least(
            levi_core::GetLogEntrysByIds {
                ids: ids.iter().map(|i| i.as_str().into()).collect(),
            },
            ids.len(),
            Duration::from_secs(10),
        )
        .map_err(|e| anyhow::anyhow!("hub did not acknowledge the write: {e}"))?;
        Ok(ids)
    }

    pub fn close(self) {
        self.client.set_address(None);
        // Give the socket a beat to flush the close frame.
        self.runtime.shutdown_timeout(Duration::from_millis(500));
    }
}
