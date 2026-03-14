use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod cached;
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
    TaskStalled {
        task_id: String,
        stall_timeout_sec: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_files: Option<Vec<String>>,
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
    /// Path to the output directory for a task.
    fn output_dir(&self, task_id: &str) -> PathBuf;
    /// Save output content for a task under a relative filename.
    fn save_output(&self, task_id: &str, filename: &str, content: &[u8]) -> Result<PathBuf>;
    /// Load output content for a task from a relative filename.
    fn get_output(&self, task_id: &str, filename: &str) -> Result<Vec<u8>>;
    /// List output files for a task as relative paths.
    fn list_outputs(&self, task_id: &str) -> Result<Vec<String>>;
    /// Save shared context data under a key.
    fn save_context(&self, key: &str, content: &[u8]) -> Result<()>;
    /// Load shared context data by key.
    fn get_context(&self, key: &str) -> Result<Vec<u8>>;
    /// Save shared context data with versioning metadata.
    /// This enables optimistic locking and audit tracking.
    fn save_context_versioned(
        &self,
        key: &str,
        content: &[u8],
        updated_by: &str,
        expected_version: Option<u64>,
    ) -> Result<ContextMetadata>;
    /// Get metadata for a context entry without loading the content.
    fn get_context_metadata(&self, key: &str) -> Result<Option<ContextMetadata>>;
}

/// Metadata for versioned context entries.
/// Used for optimistic locking and audit tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextMetadata {
    /// Version number, incremented on each update.
    pub version: u64,
    /// Identifier of the task/entity that last updated this context.
    pub updated_by: String,
    /// Timestamp of the last update.
    pub updated_at: DateTime<Utc>,
}

impl ContextMetadata {
    /// Create a new metadata entry for the first version.
    pub fn new(updated_by: String) -> Self {
        Self {
            version: 1,
            updated_by,
            updated_at: Utc::now(),
        }
    }

    /// Create a new version by incrementing the version number.
    pub fn next_version(&self, updated_by: String) -> Self {
        Self {
            version: self.version + 1,
            updated_by,
            updated_at: Utc::now(),
        }
    }
}
