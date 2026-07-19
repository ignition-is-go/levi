//! levi-hub: myko CellServer + optional Postgres, no git anywhere. Receives
//! task events + graph facts from CLIs, serves aggregate reactive queries,
//! and serves the dashboard's static files on the same origin as the WS
//! endpoint (spec §Hub).
//!
//! myko has no auth hooks, so the CellServer binds loopback-only and a thin
//! axum front door on the public address validates the shared bearer token
//! (spec deviation 5) before proxying `/myko` WebSocket traffic to it.

mod front_door;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "levi-hub", version, about = "levi aggregation hub")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the hub.
    Serve {
        /// Public address (front door: dashboard + authenticated WS).
        #[arg(long, default_value = "0.0.0.0:7377")]
        bind: String,
        /// Loopback port for the internal myko CellServer.
        #[arg(long, default_value_t = 7378)]
        internal_port: u16,
        /// Directory of built dashboard static files (levi-dash/dist).
        #[arg(long)]
        dash_dir: Option<std::path::PathBuf>,
        /// Shared bearer token; falls back to $LEVI_HUB_TOKEN. Unset = open hub.
        #[arg(long)]
        token: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // myko logs through `tracing`; this installs the subscriber (honors
    // RUST_LOG) so server-side query/command errors are actually visible.
    let _telemetry = myko_server::telemetry::init_from_env();
    let Cli { cmd } = Cli::parse();
    match cmd {
        Cmd::Serve {
            bind,
            internal_port,
            dash_dir,
            token,
        } => serve(bind, internal_port, dash_dir, token).await,
    }
}

async fn serve(
    bind: String,
    internal_port: u16,
    dash_dir: Option<std::path::PathBuf>,
    token: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    levi_core::link();
    let token = token
        .or_else(|| std::env::var("LEVI_HUB_TOKEN").ok())
        .filter(|t| !t.is_empty());
    if token.is_none() {
        log::warn!("no token configured (--token / LEVI_HUB_TOKEN): hub is open");
    }

    let mut builder =
        myko_server::CellServer::builder().with_bind_addr(([127, 0, 0, 1], internal_port).into());
    match myko_server::postgres::PostgresConfig::from_env() {
        Some(pg) => builder = builder.with_postgres(pg),
        None => log::warn!("MYKO_POSTGRES_URL not set: events are held in memory only"),
    }
    let server = builder.build();
    tokio::spawn(async move {
        if let Err(e) = server.run().await {
            log::error!("cell server exited: {e}");
            std::process::exit(1);
        }
    });

    front_door::serve(&bind, internal_port, token, dash_dir).await
}
