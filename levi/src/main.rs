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
    let mut ctx = LeviCtx::load(cli.no_sync)?;
    match cli.cmd {
        Cmd::Init { name, hub, files } => commands::init::run(&mut ctx, name, hub, files),
        Cmd::Add {
            title,
            project,
            priority,
            body,
            labels,
            deps,
            json,
        } => commands::add::run(&ctx, title, project, priority, body, labels, deps, json),
        Cmd::Ls {
            json,
            all,
            closed,
            label,
            branch,
            mine,
        } => commands::ls::run(
            &mut ctx,
            commands::ls::LsOpts {
                json,
                all,
                closed,
                label,
                branch,
                mine,
            },
        ),
        Cmd::Show { id, json } => commands::show::run(&ctx, &id, json),
        Cmd::Close {
            id,
            anchor,
            no_anchor,
            force,
        } => commands::status::run(
            &ctx,
            &id,
            StatusKind::Closed,
            commands::status::StatusOpts {
                anchor,
                no_anchor,
                force,
            },
        ),
        Cmd::Reopen {
            id,
            anchor,
            no_anchor,
            force,
        } => commands::status::run(
            &ctx,
            &id,
            StatusKind::Reopened,
            commands::status::StatusOpts {
                anchor,
                no_anchor,
                force,
            },
        ),
        Cmd::Next { claim, count, json } => commands::next::run(&mut ctx, claim, count, json),
        Cmd::Start { id } => commands::claim_ops::start(&ctx, &id),
        Cmd::Steal { id } => commands::claim_ops::steal(&ctx, &id),
        Cmd::Drop { id } => commands::claim_ops::drop(&ctx, &id),
        Cmd::Dep { cmd } => match cmd {
            levi::cli::DepCmd::Add { blocked, on, via } => {
                commands::dep::add(&ctx, &blocked, &on, via)
            }
            levi::cli::DepCmd::Rm { blocked, on } => commands::dep::rm(&ctx, &blocked, &on),
        },
        Cmd::Comment { id, text } => commands::comment::run(&ctx, &id, &text),
        Cmd::Edit {
            id,
            priority,
            title,
            labels,
            body,
        } => commands::edit::run(&ctx, &id, priority, title, labels, body),
        Cmd::Sync { no_git, no_hub } => commands::sync_cmd::run(&mut ctx, no_git, no_hub),
        Cmd::Watch { json } => commands::watch::run(&ctx, json),
    }
}
