use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ratatui::widgets::TableState;

use crate::plan::Plan;
use crate::store::fs::FsStore;
use crate::store::{LogStream, State, Store, TaskStatus};

const MAX_LOG_LINES: usize = 2000;

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: String,
    pub title: String,
    pub deps: Vec<String>,
    pub locks: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub done: usize,
    pub total: usize,
    pub running: usize,
    pub failed: usize,
}

pub struct App {
    pub store_root: PathBuf,
    pub run_id: String,
    #[allow(dead_code)]
    pub plan: Plan,
    pub state: State,
    pub tasks: Vec<TaskInfo>,
    pub list_state: TableState,
    pub log_stream: LogStream,
    pub log_lines: Vec<String>,
    pub log_offset: usize,
    pub log_focus: bool,
    pub graph_mode: bool,
    pub status_message: Option<String>,
    pub should_quit: bool,
    store: FsStore,
    log_path: PathBuf,
}

impl App {
    pub fn new(store_root: PathBuf, run_id: String, plan: Plan) -> Result<Self> {
        let store = FsStore::new(&store_root, &run_id)?;
        let state = store.read_state()?;
        let tasks = plan
            .tasks
            .iter()
            .map(|task| TaskInfo {
                id: task.id.clone(),
                title: task
                    .title
                    .clone()
                    .unwrap_or_else(|| task.id.clone()),
                deps: task.deps.clone(),
                locks: task.locks.clone(),
            })
            .collect::<Vec<_>>();

        let mut list_state = TableState::default();
        if !tasks.is_empty() {
            list_state.select(Some(0));
        }

        let mut app = Self {
            store_root,
            run_id,
            plan,
            state,
            tasks,
            list_state,
            log_stream: LogStream::Stdout,
            log_lines: Vec::new(),
            log_offset: 0,
            log_focus: false,
            graph_mode: false,
            status_message: None,
            should_quit: false,
            store,
            log_path: PathBuf::new(),
        };
        app.sync_log_path();
        app.refresh_logs()?;
        Ok(app)
    }

    pub fn sync_log_path(&mut self) -> bool {
        let new_path = match self.selected_task_id() {
            Some(task_id) => self.store.log_path(task_id, self.log_stream),
            None => PathBuf::new(),
        };
        if new_path != self.log_path {
            self.log_path = new_path;
            self.log_offset = 0;
            true
        } else {
            false
        }
    }

    pub fn refresh_state(&mut self) -> Result<()> {
        match self.store.read_state() {
            Ok(state) => self.state = state,
            Err(err) => self.set_status(format!("state read error: {err:#}")),
        }
        Ok(())
    }

    pub fn refresh_logs(&mut self) -> Result<()> {
        if self.log_path.as_os_str().is_empty() {
            self.log_lines = Vec::new();
            return Ok(());
        }
        match read_log_tail(&self.log_path, MAX_LOG_LINES) {
            Ok(lines) => self.log_lines = lines,
            Err(err) => self.set_status(format!("log read error: {err:#}")),
        }
        Ok(())
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
    }

    pub fn selected_task(&self) -> Option<&TaskInfo> {
        self.list_state
            .selected()
            .and_then(|idx| self.tasks.get(idx))
    }

    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task().map(|task| task.id.as_str())
    }

    pub fn select_next(&mut self) -> bool {
        if self.tasks.is_empty() {
            return false;
        }
        let next = match self.list_state.selected() {
            Some(idx) => (idx + 1) % self.tasks.len(),
            None => 0,
        };
        self.list_state.select(Some(next));
        true
    }

    pub fn select_prev(&mut self) -> bool {
        if self.tasks.is_empty() {
            return false;
        }
        let next = match self.list_state.selected() {
            Some(0) | None => self.tasks.len() - 1,
            Some(idx) => idx - 1,
        };
        self.list_state.select(Some(next));
        true
    }

    pub fn toggle_log_focus(&mut self) {
        self.log_focus = !self.log_focus;
        if !self.log_focus {
            self.log_offset = 0;
        }
    }

    pub fn toggle_log_stream(&mut self) {
        self.log_stream = match self.log_stream {
            LogStream::Stdout => LogStream::Stderr,
            LogStream::Stderr => LogStream::Stdout,
        };
    }

    pub fn scroll_log_up(&mut self) {
        self.log_offset = self.log_offset.saturating_add(1);
    }

    pub fn scroll_log_down(&mut self) {
        self.log_offset = self.log_offset.saturating_sub(1);
    }

    pub fn task_status(&self, task_id: &str) -> TaskStatus {
        self.state
            .tasks
            .get(task_id)
            .map(|state| state.status)
            .unwrap_or(TaskStatus::Pending)
    }

    pub fn task_duration(&self, task_id: &str, now: DateTime<Utc>) -> String {
        let Some(state) = self.state.tasks.get(task_id) else {
            return "-".to_string();
        };
        let Some(started_at) = state.started_at else {
            return "-".to_string();
        };
        let end = state.completed_at.unwrap_or(now);
        format_duration(started_at, end)
    }

    pub fn deps_remaining(&self, task: &TaskInfo) -> usize {
        task.deps
            .iter()
            .filter(|dep| {
                self.state
                    .tasks
                    .get(*dep)
                    .map(|state| state.status != TaskStatus::Succeeded)
                    .unwrap_or(true)
            })
            .count()
    }

    pub fn stats(&self) -> Stats {
        let mut stats = Stats {
            total: self.tasks.len(),
            ..Default::default()
        };
        for state in self.state.tasks.values() {
            match state.status {
                TaskStatus::Succeeded
                | TaskStatus::Failed
                | TaskStatus::Canceled
                | TaskStatus::Skipped => stats.done += 1,
                TaskStatus::Running => stats.running += 1,
                _ => {}
            }
            if state.status == TaskStatus::Failed {
                stats.failed += 1;
            }
        }
        stats
    }

    pub fn active_locks(&self) -> Vec<String> {
        let mut locks = BTreeSet::new();
        for task in &self.tasks {
            if self.task_status(&task.id) == TaskStatus::Running {
                for lock in &task.locks {
                    locks.insert(lock.clone());
                }
            }
        }
        locks.into_iter().collect()
    }

    pub fn state_path(&self) -> PathBuf {
        run_dir(&self.store_root, &self.run_id).join("state.json")
    }

    pub fn tasks_dir(&self) -> PathBuf {
        run_dir(&self.store_root, &self.run_id).join("tasks")
    }
}

fn read_log_tail(path: &Path, max_lines: usize) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path).with_context(|| format!("read log {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text
        .lines()
        .flat_map(format_log_line)
        .collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    Ok(lines)
}

fn format_log_line(line: &str) -> Vec<String> {
    // JSON形式の行かどうかを判定し、パースして読みやすく表示
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
        format_json_log(&json)
    } else {
        // JSON以外はそのまま表示
        vec![line.to_string()]
    }
}

fn format_json_log(json: &serde_json::Value) -> Vec<String> {
    let obj = match json.as_object() {
        Some(obj) => obj,
        None => return vec![json.to_string()],
    };

    let event_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match event_type {
        "tool_use" => {
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            vec![format!("🔧 Tool: {}", name)]
        }
        "tool_result" => {
            let name = obj
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            vec![format!("✓ Tool result: {}", name)]
        }
        "text" => {
            let text = obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
            text.lines().map(|line| line.to_string()).collect()
        }
        "thinking" => {
            vec!["💭 [thinking...]".to_string()]
        }
        "error" => {
            let message = obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            vec![format!("❌ Error: {}", message)]
        }
        _ => {
            // その他のイベントは簡易表示
            vec![format!("• {}: {}", event_type, json)]
        }
    }
}

fn run_dir(store_root: &Path, run_id: &str) -> PathBuf {
    store_root.join("runs").join(run_id)
}

fn format_duration(started_at: DateTime<Utc>, end: DateTime<Utc>) -> String {
    let delta = end.signed_duration_since(started_at);
    let mut secs = delta.num_seconds();
    if secs < 0 {
        secs = 0;
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}
