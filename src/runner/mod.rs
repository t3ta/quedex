use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};

use crate::plan::Task;
use crate::store::Store;
use crate::worktree::manager::WorktreeManager;

pub mod claude_code;
pub mod codex;
pub mod opencode;

/// Resolve the full path of a command using `which`.
/// This ensures commands are found even when PATH doesn't include
/// directories like ~/.local/bin (e.g., when running from IDE).
pub fn resolve_command_path(cmd: &str) -> Result<PathBuf> {
    which::which(cmd).with_context(|| format!("{} not found in PATH", cmd))
}

pub trait Runner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle>;
}

pub struct RunContext {
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub store: Arc<dyn Store>,
    pub worktree_manager: Option<Arc<WorktreeManager>>,
    /// System prompt to prepend to all task prompts
    pub system_prompt: Option<String>,
}

impl Clone for RunContext {
    fn clone(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            store: Arc::clone(&self.store),
            worktree_manager: self.worktree_manager.clone(),
            system_prompt: self.system_prompt.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ChildHandle {
    pub pid: u32,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    child: Arc<Mutex<Child>>,
}

impl ChildHandle {
    pub fn new(child: Child, stdout_path: PathBuf, stderr_path: PathBuf) -> Self {
        let pid = child.id();
        Self {
            pid,
            stdout_path,
            stderr_path,
            child: Arc::new(Mutex::new(child)),
        }
    }

    pub fn kill(&self) -> Result<()> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| anyhow!("child process lock poisoned"))?;
        child.kill().context("kill child process")?;
        Ok(())
    }

    pub fn wait(&self) -> Result<std::process::ExitStatus> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| anyhow!("child process lock poisoned"))?;
        let status = child.wait().context("wait child process")?;
        Ok(status)
    }
}
