//! Lifecycle hook execution engine.
//!
//! Hooks are shell commands executed at specific points during a run or task lifecycle.
//! Hook failures produce warnings but do not interrupt task execution,
//! unless `fail_on_error` is set for run-level hooks.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::config::HooksConfig;
use crate::plan::TaskHooksConfig;

#[derive(Debug)]
pub enum HookError {
    NonZeroExit(i32),
    Timeout(u64),
    Spawn(std::io::Error),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::NonZeroExit(code) => write!(f, "hook command failed with exit code {code}"),
            HookError::Timeout(secs) => write!(f, "hook timed out after {secs}s"),
            HookError::Spawn(e) => write!(f, "hook spawn error: {e}"),
        }
    }
}

impl From<std::io::Error> for HookError {
    fn from(e: std::io::Error) -> Self {
        HookError::Spawn(e)
    }
}

/// Context variables passed to hook commands via environment variables.
pub struct HookContext {
    pub run_id: String,
    pub run_name: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub status: Option<String>,
    pub attempt: Option<u32>,
    pub exit_code: Option<i32>,
}

impl HookContext {
    /// Build environment variables from this context.
    pub fn to_env_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        vars.insert("QUEDEX_RUN_ID".to_string(), self.run_id.clone());
        vars.insert("QUEDEX_RUN_NAME".to_string(), self.run_name.clone());
        if let Some(ref task_id) = self.task_id {
            vars.insert("QUEDEX_TASK_ID".to_string(), task_id.clone());
        }
        if let Some(ref task_title) = self.task_title {
            vars.insert("QUEDEX_TASK_TITLE".to_string(), task_title.clone());
        }
        if let Some(ref status) = self.status {
            vars.insert("QUEDEX_STATUS".to_string(), status.clone());
        }
        if let Some(attempt) = self.attempt {
            vars.insert("QUEDEX_ATTEMPT".to_string(), attempt.to_string());
        }
        if let Some(exit_code) = self.exit_code {
            vars.insert("QUEDEX_EXIT_CODE".to_string(), exit_code.to_string());
        }
        vars
    }
}

const DEFAULT_HOOK_TIMEOUT_SEC: u64 = 30;

/// Execute a hook command via `sh -c`.
///
/// The command inherits `base_env` plus hook-specific context variables.
/// On timeout the child process is killed.
/// Returns `Ok(())` on success (exit code 0) or an appropriate `HookError`.
pub async fn run_hook(
    command: &str,
    ctx: &HookContext,
    base_env: &HashMap<String, String>,
    cwd: &Path,
    timeout_sec: u64,
) -> Result<(), HookError> {
    let hook_vars = ctx.to_env_vars();

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.current_dir(cwd);
    cmd.envs(base_env);
    cmd.envs(&hook_vars);

    let timeout_duration = Duration::from_secs(timeout_sec);

    let mut child = cmd.spawn().map_err(HookError::Spawn)?;

    match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => {
            if status.success() {
                Ok(())
            } else {
                Err(HookError::NonZeroExit(status.code().unwrap_or(1)))
            }
        }
        Ok(Err(e)) => Err(HookError::Spawn(e)),
        Err(_) => {
            let _ = child.kill().await;
            Err(HookError::Timeout(timeout_sec))
        }
    }
}

/// Resolve the effective hook command for a given hook point by merging
/// global (config) hooks and task-level hooks.
/// Task-level hooks take precedence; global hooks are used as fallback.
fn resolve_hook_command(
    hook_point: HookPoint,
    global: Option<&HooksConfig>,
    task: Option<&TaskHooksConfig>,
) -> Option<String> {
    let task_cmd = task.and_then(|t| match hook_point {
        HookPoint::BeforeTask => t.before_task.as_ref(),
        HookPoint::AfterTask => t.after_task.as_ref(),
        HookPoint::OnFailure => t.on_failure.as_ref(),
        _ => None,
    });

    if let Some(cmd) = task_cmd {
        return Some(cmd.clone());
    }

    global
        .and_then(|g| match hook_point {
            HookPoint::BeforeRun => g.before_run.as_ref(),
            HookPoint::AfterRun => g.after_run.as_ref(),
            HookPoint::BeforeTask => g.before_task.as_ref(),
            HookPoint::AfterTask => g.after_task.as_ref(),
            HookPoint::OnFailure => g.on_failure.as_ref(),
        })
        .cloned()
}

/// Hook insertion points in the lifecycle.
#[derive(Debug, Clone, Copy)]
pub enum HookPoint {
    BeforeRun,
    AfterRun,
    BeforeTask,
    AfterTask,
    OnFailure,
}

impl HookPoint {
    fn label(&self) -> &'static str {
        match self {
            HookPoint::BeforeRun => "before_run",
            HookPoint::AfterRun => "after_run",
            HookPoint::BeforeTask => "before_task",
            HookPoint::AfterTask => "after_task",
            HookPoint::OnFailure => "on_failure",
        }
    }
}

/// Execute a run-level hook (before_run / after_run).
///
/// If `fail_on_error` is true in the config, an error is returned on failure.
/// Otherwise, failures are logged as warnings and `Ok(())` is returned.
pub async fn run_run_hook(
    hook_point: HookPoint,
    global: Option<&HooksConfig>,
    ctx: &HookContext,
    base_env: &HashMap<String, String>,
    cwd: &Path,
) -> Result<(), HookError> {
    let command = match resolve_hook_command(hook_point, global, None) {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    let timeout_sec = global
        .and_then(|g| g.timeout_sec)
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_SEC);
    let fail_on_error = global.and_then(|g| g.fail_on_error).unwrap_or(false);

    let label = hook_point.label();
    eprintln!("[hook] running {label}: {command}");

    match run_hook(&command, ctx, base_env, cwd, timeout_sec).await {
        Ok(()) => {
            eprintln!("[hook] {label} completed successfully");
            Ok(())
        }
        Err(e) => {
            if fail_on_error {
                eprintln!("[hook] {label} failed (fail_on_error=true): {e}");
                Err(e)
            } else {
                eprintln!("[hook] warning: {label} failed: {e}");
                Ok(())
            }
        }
    }
}

/// Execute a task-level hook (before_task / after_task / on_failure).
///
/// Global hooks serve as fallback; task-level hooks override them.
/// Failures are always logged as warnings and never interrupt task execution.
pub async fn run_task_hook(
    hook_point: HookPoint,
    global: Option<&HooksConfig>,
    task_hooks: Option<&TaskHooksConfig>,
    ctx: &HookContext,
    base_env: &HashMap<String, String>,
    cwd: &Path,
) -> Result<(), HookError> {
    let command = match resolve_hook_command(hook_point, global, task_hooks) {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    let timeout_sec = global
        .and_then(|g| g.timeout_sec)
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_SEC);

    let task_label = ctx.task_id.as_deref().unwrap_or("unknown");
    let label = hook_point.label();
    eprintln!("[hook] running {label} for task {task_label}: {command}");

    match run_hook(&command, ctx, base_env, cwd, timeout_sec).await {
        Ok(()) => {
            eprintln!("[hook] {label} for task {task_label} completed successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("[hook] warning: {label} for task {task_label} failed: {e}");
            Err(e)
        }
    }
}
