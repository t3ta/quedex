use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::plan::Task;
use crate::runner::{ChildHandle, RunContext, Runner};
use crate::store::LogStream;

#[derive(Clone, Copy)]
pub struct ShellRunner;

impl ShellRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShellRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ShellRunner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle> {
        let config = task
            .shell
            .as_ref()
            .context("shell config missing")?;

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

        let child = Command::new("bash")
            .arg("-lc")
            .arg(&config.command)
            .current_dir(&ctx.cwd)
            .envs(&ctx.env)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn shell command")?;

        Ok(ChildHandle::new(child, stdout_path, stderr_path))
    }
}
