//! The public face of the hub: serves dashboard static files, validates the
//! shared bearer token, and proxies `/myko` WebSocket traffic to the internal
//! CellServer on loopback. Token sources, in order: `?token=` query param
//! (the CLI's transport appends it), `Authorization: Bearer`, and the
//! `levi_token` cookie (set by the dashboard so the browser's WS upgrade
//! carries it on the same origin).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::Router;
use axum::extract::ws::{Message as AMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as TMsg;
use tower_http::services::ServeDir;

#[derive(Clone)]
struct DoorState {
    token: Option<String>,
    internal_port: u16,
}

pub async fn serve(
    bind: &str,
    internal_port: u16,
    token: Option<String>,
    dash_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = DoorState {
        token,
        internal_port,
    };
    let mut app = Router::new()
        .route("/myko", get(ws_handler))
        .with_state(state);
    app = match dash_dir {
        Some(dir) => {
            log::info!("serving dashboard from {}", dir.display());
            app.fallback_service(ServeDir::new(dir))
        }
        None => app.fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                "no dashboard built (start levi-hub with --dash-dir)",
            )
        }),
    };
    let listener = tokio::net::TcpListener::bind(bind).await?;
    log::info!("levi-hub: ws://{bind}/myko (internal myko on 127.0.0.1:{internal_port})");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(
    State(state): State<DoorState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&state, &query, &headers) {
        return (StatusCode::UNAUTHORIZED, "invalid or missing token").into_response();
    }
    let port = state.internal_port;
    ws.on_upgrade(move |client| async move {
        if let Err(e) = pipe(client, port).await {
            log::debug!("ws proxy session ended: {e:#}");
        }
    })
}

fn authorized(state: &DoorState, query: &HashMap<String, String>, headers: &HeaderMap) -> bool {
    let Some(expected) = &state.token else {
        return true;
    };
    if query.get("token") == Some(expected) {
        return true;
    }
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok())
        && auth.strip_prefix("Bearer ") == Some(expected.as_str())
    {
        return true;
    }
    if let Some(cookies) = headers.get("cookie").and_then(|v| v.to_str().ok())
        && cookies
            .split(';')
            .any(|part| part.trim().strip_prefix("levi_token=") == Some(expected.as_str()))
    {
        return true;
    }
    false
}

/// Bidirectional byte-pipe between the authenticated client socket and the
/// internal CellServer.
async fn pipe(client: WebSocket, internal_port: u16) -> anyhow::Result<()> {
    let url = format!("ws://127.0.0.1:{internal_port}/myko");
    // The CellServer may still be starting; retry briefly.
    let mut upstream = None;
    for attempt in 0..10 {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => {
                upstream = Some(ws);
                break;
            }
            Err(_) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let upstream = upstream.expect("loop either sets upstream or returns");

    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut client_tx, mut client_rx) = client.split();

    let to_upstream = async {
        while let Some(msg) = client_rx.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(_) => break,
            };
            let out = match msg {
                AMsg::Text(t) => TMsg::Text(t),
                AMsg::Binary(b) => TMsg::Binary(b),
                AMsg::Ping(p) => TMsg::Ping(p),
                AMsg::Pong(p) => TMsg::Pong(p),
                AMsg::Close(_) => break,
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };
    let to_client = async {
        while let Some(msg) = up_rx.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(_) => break,
            };
            let out = match msg {
                TMsg::Text(t) => AMsg::Text(t),
                TMsg::Binary(b) => AMsg::Binary(b),
                TMsg::Ping(p) => AMsg::Ping(p),
                TMsg::Pong(p) => AMsg::Pong(p),
                TMsg::Close(_) | TMsg::Frame(_) => break,
            };
            if client_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };

    tokio::join!(to_upstream, to_client);
    Ok(())
}
