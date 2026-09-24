//! `dup` - AI clone version control CLI.
//!
//! Git-style file-level sync between a local clone workspace and the
//! remote (duphub or an opencarrier runtime). Workflow:
//!   clone -> pull -> (edit) -> status -> commit -> push
//! Local history (commit + restore) is content-backed by `.dup/objects/`;
//! the remote is stateless, so rollback is always a local operation.
//!
//! aginx-carrier 移植说明：上游 opencarrier-clones/crates/dup 的同步核心
//! 原样搬运；`create`（LLM 生成分身）、`health`/`eval`（体检评分）留在
//! 宿主侧生态未搬。remote 端点默认 duphub templates 形状，可配。

mod commands;
mod config;
mod diff;
mod manifest;
mod merge;
mod objects;
mod remote;
mod state;
mod workspace;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dup", about = "AI clone version control - like git, for agents")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Clone a workspace from the remote (init + first pull)
    #[command(visible_alias = "get")]
    Clone {
        /// Clone name on the remote
        name: String,
    },

    /// Initialize / link this workspace to the remote
    Init {
        /// Clone name on the remote (defaults to directory name)
        name: Option<String>,
    },

    /// Pull remote changes and 3-way merge into the working tree
    Pull,

    /// Push local changes to the remote (fast-forward only)
    Push,

    /// Show working tree status (changes since last commit)
    #[command(visible_alias = "st")]
    Status {
        /// Compare against the remote base instead of the last commit
        #[arg(long)]
        remote: bool,
    },

    /// Show file-level diff of working tree changes
    #[command(visible_alias = "d")]
    Diff {
        /// Compare against the remote base instead of the last commit
        #[arg(long)]
        remote: bool,
    },

    /// Save a local version (snapshot working tree)
    #[command(visible_alias = "ci")]
    Commit {
        /// Commit message
        #[arg(short = 'm', long)]
        message: String,
    },

    /// Show commit history
    Log,

    /// Restore files from a local commit (rollback; remote is stateless)
    Restore {
        /// Commit hash (unique prefix is enough)
        commit: String,
        /// Only restore these paths (default: full snapshot rollback)
        paths: Vec<String>,
    },

    /// Show or set configuration
    #[command(visible_alias = "cfg")]
    Config {
        /// Config key and optional value
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Clone { name } => {
            commands::clone_cmd::run(&name).await?;
        }
        Commands::Init { name } => {
            commands::init::run(name)?;
        }
        Commands::Pull => {
            commands::pull::run().await?;
        }
        Commands::Push => {
            commands::push::run().await?;
        }
        Commands::Status { remote } => {
            commands::status::run(remote)?;
        }
        Commands::Diff { remote } => {
            commands::diff_cmd::run(remote)?;
        }
        Commands::Commit { message } => {
            commands::commit::run(&message)?;
        }
        Commands::Log => {
            commands::log_cmd::run()?;
        }
        Commands::Restore { commit, paths } => {
            commands::restore::run(&commit, &paths)?;
        }
        Commands::Config { args } => {
            commands::config_cmd::run(&args)?;
        }
    }

    Ok(())
}
