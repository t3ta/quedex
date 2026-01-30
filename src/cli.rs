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

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new plan template (JSON or YAML)
    Init {
        /// Output file path (default: plan.json, use .yaml/.yml for YAML format)
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
    /// Show run status
    Status {
        run_id: Option<String>,
        /// Filter by group name
        #[arg(long)]
        group: Option<String>,
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
    /// Show task output files
    Outputs {
        run_id: String,
        /// Filter by task ID
        #[arg(long = "task", value_name = "TASK_ID")]
        task_id: Option<String>,
    },
    /// Retry a failed task
    Retry {
        run_id: String,
        /// Task ID to retry (conflicts with --group)
        #[arg(conflicts_with = "group")]
        task_id: Option<String>,
        /// Group name to retry all failed/canceled/skipped tasks
        #[arg(long, conflicts_with = "task_id")]
        group: Option<String>,
        #[arg(long, help = "Reload plan from the run directory before retrying")]
        reload_plan: bool,
    },
    /// Cancel a running task or run
    Cancel {
        run_id: String,
        /// Task ID to cancel (conflicts with --group)
        #[arg(conflicts_with = "group")]
        task_id: Option<String>,
        /// Group name to cancel all running/pending tasks
        #[arg(long, conflicts_with = "task_id")]
        group: Option<String>,
    },
    /// Clean up run directories
    Clean {
        run_id: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        all: bool,
        /// Fix orphaned runs (mark as failed if parent process is dead)
        #[arg(long, action = ArgAction::SetTrue)]
        fix_orphans: bool,
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
    /// Output JSON schema for plan files (plan.json/plan.yaml)
    Schema {
        /// Output file path (default: stdout)
        #[arg(short = 'o', long, value_name = "path")]
        output: Option<PathBuf>,
    },
    /// Show execution statistics and metrics
    Stats {
        /// Time period to analyze (e.g., "7d", "24h", "1w")
        #[arg(long, value_name = "DURATION")]
        since: Option<String>,
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Analyze execution plan without running tasks
    DryRun {
        /// Path to plan file
        plan: String,
        /// Show execution order in waves (respecting max_concurrency and locks)
        #[arg(long, action = ArgAction::SetTrue)]
        show_order: bool,
        /// Check for potential lock conflicts
        #[arg(long, action = ArgAction::SetTrue)]
        check_locks: bool,
        /// Output dependency graph in Mermaid format
        #[arg(long, action = ArgAction::SetTrue)]
        mermaid: bool,
    },
    /// Start web dashboard server
    Serve {
        /// Run ID to monitor (optional, will monitor all runs if not specified)
        run_id: Option<String>,
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(Debug, Args, Clone, Copy)]
pub struct RecoveryOptions {
    #[arg(long, action = ArgAction::SetTrue)]
    pub resume: bool,
    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "resume")]
    pub clean_start: bool,
}
