use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};

use crate::plan::Task;
use crate::store::Store;

pub mod claude_code;
pub mod codex;
pub mod opencode;

pub trait Runner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle>;
}

pub struct RunContext {
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub store: Arc<dyn Store>,
}

impl Clone for RunContext {
    fn clone(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            store: Arc::clone(&self.store),
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
