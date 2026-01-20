use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "quedex")]
#[command(author, version, about)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOptions,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Args, Clone)]
pub struct GlobalOptions {
    #[arg(long, value_name = "path")]
    pub store: Option<PathBuf>,
    #[arg(long, value_name = "n")]
    pub max_concurrency: Option<usize>,
    #[arg(long, action = ArgAction::SetTrue, default_value_t = true)]
    pub fail_fast: bool,
    #[arg(long = "no-fail-fast", action = ArgAction::SetTrue, conflicts_with = "fail_fast")]
    pub no_fail_fast: bool,
    /// Enable verbose output for debugging
    #[arg(long, short = 'v', action = ArgAction::SetTrue, global = true)]
    pub verbose: bool,
}

impl GlobalOptions {
    pub fn effective_fail_fast(&self) -> bool {
        if self.no_fail_fast {
            false
        } else {
            self.fail_fast
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new plan.json template
    Init {
        /// Output file path (default: plan.json)
        #[arg(short = 'o', long, value_name = "path")]
        output: Option<PathBuf>,
        /// Overwrite existing file
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
    },
    /// Run a plan (blocking)
    Run {
        plan: String,
        #[command(flatten)]
        recovery: RecoveryOptions,
        #[arg(long, hide = true)]
        run_id: Option<String>,
        #[arg(long, hide = true)]
        base_dir: Option<PathBuf>,
        /// Show execution plan without running tasks
        #[arg(long, action = ArgAction::SetTrue)]
        dry_run: bool,
    },
    /// Start a plan in background
    Start {
        plan: String,
        #[command(flatten)]
        recovery: RecoveryOptions,
        #[arg(long, hide = true)]
        run_id: Option<String>,
    },
    /// Show run status
    Status {
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Open TUI monitor
    Tui {
        run_id: Option<String>,
    },
    /// Show task logs
    Logs {
        run_id: String,
        task_id: String,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long)]
        stderr: bool,
    },
    /// Retry a failed task
    Retry {
        run_id: String,
        task_id: String,
        #[arg(long, help = "Reload plan.json from the run directory before retrying")]
        reload_plan: bool,
    },
    /// Cancel a running task or run
    Cancel {
        run_id: String,
        task_id: Option<String>,
    },
    /// Clean up run directories
    Clean {
        run_id: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        all: bool,
    },
    /// Show task dependency graph
    Graph {
        target: String,
        #[arg(long, conflicts_with = "ascii")]
        mermaid: bool,
        #[arg(long, conflicts_with = "mermaid")]
        ascii: bool,
    },
    /// Show execution history
    History {
        /// Maximum number of entries to show
        #[arg(long, short = 'n', default_value = "10")]
        limit: usize,
        /// Show all history entries
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "limit")]
        all: bool,
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Output JSON schema for plan.json
    Schema {
        /// Output file path (default: stdout)
        #[arg(short = 'o', long, value_name = "path")]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Args, Clone, Copy)]
pub struct RecoveryOptions {
    #[arg(long, action = ArgAction::SetTrue)]
    pub resume: bool,
    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "resume")]
    pub clean_start: bool,
}
