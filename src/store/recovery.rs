use std::process::Command;

use anyhow::{Context, Result};
#[cfg(not(unix))]
use anyhow::anyhow;
use chrono::Utc;

use crate::store::{Event, State, Store, TaskStatus};

#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub state: State,
    pub running_tasks: Vec<String>,
    pub alive_tasks: Vec<String>,
    pub failed_tasks: Vec<String>,
}

pub fn recover_running_tasks(store: &dyn Store) -> Result<RecoveryReport> {
    let mut state = store.read_state()?;
    let now = Utc::now();
    let mut running_tasks = Vec::new();
    let mut alive_tasks = Vec::new();
    let mut failed_tasks = Vec::new();

    for (task_id, task_state) in state.tasks.iter_mut() {
        if task_state.status != TaskStatus::Running {
            continue;
        }
        running_tasks.push(task_id.clone());
        let alive = match task_state.pid {
            Some(pid) => pid_is_alive(pid)?,
            None => false,
        };
        if alive {
            alive_tasks.push(task_id.clone());
            continue;
        }
        task_state.status = TaskStatus::Failed;
        task_state.exit_code = Some(-1);
        task_state.completed_at = Some(now);
        task_state.pid = None;
        failed_tasks.push(task_id.clone());
    }

    if !failed_tasks.is_empty() {
        store.write_state(state.clone())?;
        for task_id in &failed_tasks {
            store.append_event(Event::TaskExited {
                task_id: task_id.clone(),
                exit_code: -1,
                timestamp: now,
            })?;
        }
    }

    Ok(RecoveryReport {
        state,
        running_tasks,
        alive_tasks,
        failed_tasks,
    })
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> Result<bool> {
    let status = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .context("spawn kill -0 for pid check")?;
    Ok(status.success())
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> Result<bool> {
    Err(anyhow!("pid checks are not supported on this platform"))
}
