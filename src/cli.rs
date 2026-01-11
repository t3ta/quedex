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
    Run {
        plan: String,
        #[command(flatten)]
        recovery: RecoveryOptions,
        #[arg(long, hide = true)]
        run_id: Option<String>,
        #[arg(long, hide = true)]
        base_dir: Option<PathBuf>,
    },
    Start {
        plan: String,
        #[command(flatten)]
        recovery: RecoveryOptions,
        #[arg(long, hide = true)]
        run_id: Option<String>,
    },
    Status {
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Tui {
        run_id: Option<String>,
    },
    Logs {
        run_id: String,
        task_id: String,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long)]
        stderr: bool,
    },
    Retry {
        run_id: String,
        task_id: String,
    },
    Cancel {
        run_id: String,
        task_id: Option<String>,
    },
    Graph {
        target: String,
        #[arg(long, conflicts_with = "ascii")]
        mermaid: bool,
        #[arg(long, conflicts_with = "mermaid")]
        ascii: bool,
    },
}

#[derive(Debug, Args, Clone, Copy)]
pub struct RecoveryOptions {
    #[arg(long, action = ArgAction::SetTrue)]
    pub resume: bool,
    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "resume")]
    pub clean_start: bool,
}
