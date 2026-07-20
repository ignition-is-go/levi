use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "levi",
    version,
    about = "git-aware, agent-first, distributed issue tracker"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
    /// Skip the opportunistic hub/remote sync after mutating commands.
    #[arg(long, global = true)]
    pub no_sync: bool,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Set this repo up for levi: adopt the project from the remote when
    /// one exists (mint it otherwise), record the hub address in
    /// .levi/config.toml, and write task-tracking instructions for coding
    /// agents into CLAUDE.md / AGENTS.md. Idempotent.
    #[command(alias = "onboard")]
    Init {
        /// Project name (defaults to the repository directory name).
        #[arg(long)]
        name: Option<String>,
        /// Hub address to record in .levi/config.toml (e.g. hub.example.com:7377).
        #[arg(long)]
        hub: Option<String>,
        /// Instruction file(s) to write; defaults to every existing
        /// CLAUDE.md / AGENTS.md, or a new AGENTS.md if neither exists.
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
    },
    /// Add a task (use --project to file it in another project via the hub).
    Add {
        title: String,
        /// File into another project (name or id from the hub registry).
        #[arg(long)]
        project: Option<String>,
        /// Priority: p0..p3 (default p2).
        #[arg(short = 'p', long)]
        priority: Option<String>,
        #[arg(short = 'b', long)]
        body: Option<String>,
        /// May be repeated.
        #[arg(short = 'l', long = "label")]
        labels: Vec<String>,
        /// Existing task this one is blocked by; may be repeated.
        #[arg(long = "dep")]
        deps: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// List tasks open on this checkout (default), or --all / --closed.
    Ls {
        #[arg(long)]
        json: bool,
        #[arg(long, conflicts_with = "closed")]
        all: bool,
        #[arg(long)]
        closed: bool,
        #[arg(short = 'l', long = "label")]
        label: Option<String>,
        /// Resolve against this branch instead of HEAD.
        #[arg(long)]
        branch: Option<String>,
        /// Only tasks claimed by me (this dev/machine/worktree).
        #[arg(long)]
        mine: bool,
    },
    /// Task detail: body, comments, deps, claim, status history.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// The most important eligible work, deterministically ranked.
    Next {
        /// Atomically claim the top task before printing it.
        #[arg(long)]
        claim: bool,
        #[arg(short = 'n', long, default_value_t = 1)]
        count: usize,
        #[arg(long)]
        json: bool,
    },
    /// Claim a task for this dev/machine/worktree.
    Start { id: String },
    /// Take over a task someone else has claimed.
    Steal { id: String },
    /// Release your claim on a task.
    Drop { id: String },
    /// Close a task, anchored at HEAD by default.
    Close {
        id: String,
        /// Anchor at this commit instead of HEAD.
        #[arg(long, conflicts_with = "no_anchor")]
        anchor: Option<String>,
        /// Unanchored: resolves as closed on every checkout.
        #[arg(long)]
        no_anchor: bool,
        /// Allow a redundant transition (already closed here).
        #[arg(long)]
        force: bool,
    },
    /// Reopen a task, anchored at HEAD by default.
    Reopen {
        id: String,
        #[arg(long, conflicts_with = "no_anchor")]
        anchor: Option<String>,
        #[arg(long)]
        no_anchor: bool,
        #[arg(long)]
        force: bool,
    },
    /// Manage dependencies between tasks.
    Dep {
        #[command(subcommand)]
        cmd: DepCmd,
    },
    /// Comment on a task.
    Comment { id: String, text: String },
    /// Edit task metadata.
    Edit {
        id: String,
        #[arg(short = 'p', long)]
        priority: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// +label adds, -label removes; may be repeated.
        #[arg(short = 'l', long = "label", allow_hyphen_values = true)]
        labels: Vec<String>,
        #[arg(short = 'b', long)]
        body: Option<String>,
    },
    /// Sync the event log: git leg, hub leg, facts leg.
    Sync {
        #[arg(long)]
        no_git: bool,
        #[arg(long)]
        no_hub: bool,
    },
    /// Stream live events from the hub.
    Watch {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum DepCmd {
    /// BLOCKED is blocked by --on BLOCKER (same project, or
    /// `project/lv-xxxx[@ref]` for a cross-project blocker).
    Add {
        blocked: String,
        #[arg(long = "on")]
        on: String,
        /// How the dependency is consumed (cross-project only), e.g.
        /// "cargo: crates.io myko ^4.24". Agents verify against this.
        #[arg(long)]
        via: Option<String>,
    },
    /// Remove a dependency.
    Rm {
        blocked: String,
        #[arg(long = "on")]
        on: String,
    },
}
