//! levi-hub: a myko CellServer + optional Postgres, no git anywhere
//! (spec §Hub). Receives task events + graph facts from CLIs and serves
//! aggregate reactive queries. The dashboard is a standalone Leptos CSR app
//! (levi-dash) that connects to the same `/myko` endpoint.

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
        /// Address for the myko WebSocket endpoint (ws://<bind>/myko).
        #[arg(long, default_value = "0.0.0.0:7377")]
        bind: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // myko logs through `tracing`; this installs the subscriber (honors
    // RUST_LOG) so server-side query/command errors are actually visible.
    let _telemetry = myko_server::telemetry::init_from_env();
    let Cli { cmd } = Cli::parse();
    match cmd {
        Cmd::Serve { bind } => serve(bind).await,
    }
}

async fn serve(bind: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    levi_core::link();
    let mut builder = myko_server::CellServer::builder().with_bind_addr(bind.parse()?);
    match myko_server::postgres::PostgresConfig::from_env() {
        Some(pg) => builder = builder.with_postgres(pg),
        None => tracing::warn!("MYKO_POSTGRES_URL not set: events are held in memory only"),
    }
    builder.build().run().await
}
