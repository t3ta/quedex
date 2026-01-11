use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::plan::{Task, TaskMode};
use crate::runner::{ChildHandle, RunContext, Runner};
use crate::store::LogStream;

const VERIFY_AFTER_SUFFIX: &str = "実装後 build→lint→test を実行し、エラーがあれば修正して";

#[derive(Clone, Copy)]
pub struct CodexRunner;

impl CodexRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for CodexRunner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle> {
        let config = task
            .codex
            .as_ref()
            .context("codex config missing")?;

        let mut prompt = config.prompt.clone();
        if matches!(task.mode, TaskMode::Implement) && config.verify_after {
            prompt.push_str("\n\n");
            prompt.push_str(VERIFY_AFTER_SUFFIX);
        }

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

        let mut cmd = Command::new("codex");
        cmd.arg("exec").arg(prompt);
        match task.mode {
            TaskMode::Research => {
                let output = config
                    .output_last_message
                    .as_ref()
                    .context("output_last_message missing for research task")?;
                cmd.arg("--output-last-message").arg(output);
                if let Some(sandbox) = config.sandbox.as_deref() {
                    cmd.arg("-s").arg(sandbox);
                }
            }
            TaskMode::Implement | TaskMode::Verify => {
                cmd.arg("--full-auto");
                if let Some(sandbox) = config.sandbox.as_deref() {
                    if sandbox != "workspace-write" {
                        cmd.arg("-s").arg(sandbox);
                    }
                }
            }
        }

        let child = cmd
            .current_dir(&ctx.cwd)
            .envs(&ctx.env)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn codex")?;

        Ok(ChildHandle::new(child, stdout_path, stderr_path))
    }
}
