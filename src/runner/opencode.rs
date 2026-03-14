use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::plan::Task;
use crate::runner::{ChildHandle, RunContext, Runner, resolve_command_path};
use crate::store::LogStream;

#[derive(Clone, Copy)]
pub struct OpencodeRunner;

impl OpencodeRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpencodeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for OpencodeRunner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle> {
        let config = task.opencode.as_ref().context("opencode config missing")?;

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

        let opencode_path = resolve_command_path("opencode")?;
        let mut cmd = Command::new(opencode_path);
        cmd.arg("run");

        // Model selection (optional)
        if let Some(model) = config.model.as_ref() {
            cmd.arg("-m").arg(model);
        }

        // Output format
        if config.json {
            cmd.arg("--format").arg("json");
        }

        // Prompt as positional argument
        cmd.arg(&prompt);

        let child = cmd
            .current_dir(&ctx.cwd)
            .envs(&ctx.env)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn opencode")?;

        Ok(ChildHandle::new(child, stdout_path, stderr_path))
    }
}
