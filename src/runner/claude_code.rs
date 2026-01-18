use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::plan::{Task, TaskMode};
use crate::runner::{ChildHandle, RunContext, Runner};
use crate::store::LogStream;

#[derive(Clone, Copy)]
pub struct ClaudeCodeRunner;

impl ClaudeCodeRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeCodeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ClaudeCodeRunner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle> {
        let config = task
            .claude_code
            .as_ref()
            .context("claude_code config missing")?;

        let stdout_path = ctx.store.log_path(&task.id, LogStream::Stdout);
        let stderr_path = ctx.store.log_path(&task.id, LogStream::Stderr);
        let stdout = ctx
            .store
            .open_log(&task.id, LogStream::Stdout)
            .context("open stdout log")?;
        let stderr = ctx
            .store
            .open_log(&task.id, LogStream::Stderr)
            .context("open stderr log")?;

        let mut cmd = Command::new("claude");
        cmd.arg("--print");
        cmd.arg("--dangerously-skip-permissions");

        // Model selection (default: sonnet)
        let model = config.model.as_deref().unwrap_or("sonnet");
        cmd.arg("--model").arg(model);

        // Output format
        if config.json {
            cmd.arg("--output-format").arg("json");
        }

        // For research mode, save the last message to file
        if task.mode == TaskMode::Research {
            if let Some(output_path) = config.output_last_message.as_ref() {
                cmd.arg("--output-file").arg(output_path);
            }
        }

        // Prompt as positional argument
        cmd.arg(&config.prompt);

        let child = cmd
            .current_dir(&ctx.cwd)
            .envs(&ctx.env)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn claude")?;

        Ok(ChildHandle::new(child, stdout_path, stderr_path))
    }
}
