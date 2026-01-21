use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod fs;
pub mod recovery;

#[derive(Debug, Clone, Copy)]
pub enum LogStream {
    Stdout,
    Stderr,
}

impl LogStream {
    pub fn file_name(&self) -> &'static str {
        match self {
            LogStream::Stdout => "stdout.log",
            LogStream::Stderr => "stderr.log",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    RunStarted {
        run_id: String,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskStarted {
        task_id: String,
        pid: u32,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskExited {
        task_id: String,
        #[serde(rename = "code")]
        exit_code: i32,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskCanceled {
        task_id: String,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Ready,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Skipped,
}

/// Reason for why a task was skipped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SkipReason {
    /// Skipped because a dependency failed or was canceled
    DependencyFailed,
    /// Skipped due to fail-fast mode being triggered
    FailFast,
    /// Skipped because the task's condition was not met
    ConditionNotMet {
        /// Human-readable description of the condition that was not met
        condition: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub run_id: String,
    pub run_name: String,
    pub status: RunStatus,
    pub tasks: HashMap<String, TaskState>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub stderr_tail: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Reason for why this task was skipped (if status is Skipped)
    #[serde(default)]
    pub skip_reason: Option<SkipReason>,
}

pub trait Store: Send + Sync {
    fn append_event(&self, event: Event) -> Result<()>;
    fn write_state(&self, state: State) -> Result<()>;
    fn read_state(&self) -> Result<State>;
    fn open_log(&self, task_id: &str, stream: LogStream) -> Result<File>;
    fn log_path(&self, task_id: &str, stream: LogStream) -> PathBuf;
}
