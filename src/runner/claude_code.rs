use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::plan::Task;
use crate::runner::{ChildHandle, RunContext, Runner, resolve_command_path};
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

        // Build prompt with optional system prompt prefix
        let mut prompt = String::new();
        if let Some(ref sys) = ctx.system_prompt {
            prompt.push_str(sys);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&config.prompt);

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

        let claude_path = resolve_command_path("claude")?;
        let mut cmd = Command::new(claude_path);
        cmd.arg("--print");
        cmd.arg("--dangerously-skip-permissions");

        // Model selection (default: sonnet)
        let model = config.model.as_deref().unwrap_or("sonnet");
        cmd.arg("--model").arg(model);

        // Output format (stream-json requires --verbose with --print)
        if config.json {
            cmd.arg("--verbose");
            cmd.arg("--output-format").arg("stream-json");
        }

        // Prompt as positional argument
        cmd.arg(&prompt);

        let child = cmd
            .current_dir(&ctx.cwd)
            .envs(&ctx.env)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn claude")?;

        Ok(ChildHandle::new(child, stdout_path, stderr_path))
    }
}
