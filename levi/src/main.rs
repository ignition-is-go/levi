use clap::Parser;
use levi::cli::{Cli, Cmd};
use levi::commands;
use levi::ctx::LeviCtx;
use levi_core::StatusKind;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let ctx = LeviCtx::load(cli.no_sync)?;
    match cli.cmd {
        Cmd::Init { name } => commands::init::run(&ctx, name),
        Cmd::Add { title, priority, body, labels, deps, json } => {
            commands::add::run(&ctx, title, priority, body, labels, deps, json)
        }
        Cmd::Ls { json, all, closed, label, branch, mine } => {
            commands::ls::run(&ctx, commands::ls::LsOpts { json, all, closed, label, branch, mine })
        }
        Cmd::Show { id, json } => commands::show::run(&ctx, &id, json),
        Cmd::Close { id, anchor, no_anchor, force } => commands::status::run(
            &ctx,
            &id,
            StatusKind::Closed,
            commands::status::StatusOpts { anchor, no_anchor, force },
        ),
        Cmd::Reopen { id, anchor, no_anchor, force } => commands::status::run(
            &ctx,
            &id,
            StatusKind::Reopened,
            commands::status::StatusOpts { anchor, no_anchor, force },
        ),
        Cmd::Next { .. }
        | Cmd::Start { .. }
        | Cmd::Steal { .. }
        | Cmd::Drop { .. }
        | Cmd::Dep { .. }
        | Cmd::Comment { .. }
        | Cmd::Edit { .. }
        | Cmd::Sync { .. }
        | Cmd::Watch { .. } => anyhow::bail!("not implemented yet"),
    }
}
