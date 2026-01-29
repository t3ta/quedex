use std::collections::{BTreeSet, HashSet};
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
    pub group: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub done: usize,
    pub total: usize,
    pub running: usize,
    pub failed: usize,
}

/// Represents a row in the task list (either a group header or a task).
#[derive(Debug, Clone)]
pub enum DisplayRow {
    GroupHeader { name: String, task_count: usize },
    Task(TaskInfo),
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
    pub log_horizontal_offset: usize,
    pub log_focus: bool,
    pub graph_mode: bool,
    pub status_message: Option<String>,
    pub should_quit: bool,
    /// Collapsed groups (group names that are currently collapsed).
    pub collapsed_groups: HashSet<String>,
    /// Display rows (computed from tasks and collapse state).
    pub display_rows: Vec<DisplayRow>,
    store: FsStore,
    log_path: PathBuf,
}

impl App {
    pub fn new(store_root: PathBuf, run_id: String, plan: Plan) -> Result<Self> {
        let store = FsStore::new(&store_root, &run_id)?;
        let state = store.read_state()?;

        // Build a map from task ID to group name using Plan.groups
        let mut task_to_group: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for (group_name, task_ids) in &plan.groups {
            for task_id in task_ids {
                task_to_group.insert(task_id.as_str(), group_name.as_str());
            }
        }

        let tasks = plan
            .tasks
            .iter()
            .map(|task| {
                // Task.group takes precedence over Plan.groups
                let group = task.group.clone().or_else(|| {
                    task_to_group
                        .get(task.id.as_str())
                        .map(|s| s.to_string())
                });
                TaskInfo {
                    id: task.id.clone(),
                    title: task
                        .title
                        .clone()
                        .unwrap_or_else(|| task.id.clone()),
                    deps: task.deps.clone(),
                    locks: task.locks.clone(),
                    group,
                }
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
            log_horizontal_offset: 0,
            log_focus: false,
            graph_mode: false,
            status_message: None,
            should_quit: false,
            collapsed_groups: HashSet::new(),
            display_rows: Vec::new(),
            store,
            log_path: PathBuf::new(),
        };
        app.rebuild_display_rows();
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
        self.list_state.selected().and_then(|idx| {
            match self.display_rows.get(idx) {
                Some(DisplayRow::Task(task)) => Some(task),
                _ => None,
            }
        })
    }

    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task().map(|task| task.id.as_str())
    }

    /// Returns the currently selected display row.
    pub fn selected_display_row(&self) -> Option<&DisplayRow> {
        self.list_state
            .selected()
            .and_then(|idx| self.display_rows.get(idx))
    }

    pub fn select_next(&mut self) -> bool {
        if self.display_rows.is_empty() {
            return false;
        }
        let next = match self.list_state.selected() {
            Some(idx) => (idx + 1) % self.display_rows.len(),
            None => 0,
        };
        self.list_state.select(Some(next));
        true
    }

    pub fn select_prev(&mut self) -> bool {
        if self.display_rows.is_empty() {
            return false;
        }
        let next = match self.list_state.selected() {
            Some(0) | None => self.display_rows.len() - 1,
            Some(idx) => idx - 1,
        };
        self.list_state.select(Some(next));
        true
    }

    /// Toggle collapse state for a group.
    pub fn toggle_group_collapse(&mut self, group: &str) {
        if self.collapsed_groups.contains(group) {
            self.collapsed_groups.remove(group);
        } else {
            self.collapsed_groups.insert(group.to_string());
        }
        self.rebuild_display_rows();
    }

    /// Toggle collapse for the currently selected row's group.
    /// If a group header is selected, toggle that group.
    /// If a task is selected and belongs to a group, toggle that group.
    pub fn toggle_selected_group_collapse(&mut self) {
        let group_name = match self.selected_display_row() {
            Some(DisplayRow::GroupHeader { name, .. }) => Some(name.clone()),
            Some(DisplayRow::Task(task)) => task.group.clone(),
            None => None,
        };
        if let Some(name) = group_name {
            self.toggle_group_collapse(&name);
        }
    }

    /// Rebuild display_rows based on current tasks and collapsed state.
    pub fn rebuild_display_rows(&mut self) {
        use std::collections::HashMap;

        let mut rows = Vec::new();

        // Group tasks by their group name
        let mut grouped: HashMap<Option<String>, Vec<&TaskInfo>> = HashMap::new();
        for task in &self.tasks {
            grouped.entry(task.group.clone()).or_default().push(task);
        }

        // Collect and sort group names (Some groups first, then None)
        let mut group_names: Vec<Option<String>> = grouped.keys().cloned().collect();
        group_names.sort_by(|a, b| match (a, b) {
            (Some(a_name), Some(b_name)) => a_name.cmp(b_name),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });

        for group_name in group_names {
            let tasks = grouped.get(&group_name).unwrap();
            if let Some(ref name) = group_name {
                // Add group header
                rows.push(DisplayRow::GroupHeader {
                    name: name.clone(),
                    task_count: tasks.len(),
                });
                // Add tasks if not collapsed
                if !self.collapsed_groups.contains(name) {
                    for task in tasks {
                        rows.push(DisplayRow::Task((*task).clone()));
                    }
                }
            } else {
                // Ungrouped tasks (no header)
                for task in tasks {
                    rows.push(DisplayRow::Task((*task).clone()));
                }
            }
        }

        self.display_rows = rows;

        // Adjust selection if it's now out of bounds
        if let Some(selected) = self.list_state.selected() {
            if selected >= self.display_rows.len() {
                let new_selection = self.display_rows.len().saturating_sub(1);
                self.list_state.select(if self.display_rows.is_empty() {
                    None
                } else {
                    Some(new_selection)
                });
            }
        }
    }

    pub fn toggle_log_focus(&mut self) {
        self.log_focus = !self.log_focus;
        if !self.log_focus {
            self.log_offset = 0;
            self.log_horizontal_offset = 0;
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

    pub fn scroll_log_left(&mut self) {
        self.log_horizontal_offset = self.log_horizontal_offset.saturating_sub(1);
    }

    pub fn scroll_log_right(&mut self) {
        self.log_horizontal_offset = self.log_horizontal_offset.saturating_add(1);
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
        None => return format_json_value_lines(json, ""),
    };

    let event_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match event_type {
        "tool_use" => format_tool_use(json, obj),
        "tool_result" => format_tool_result(json, obj),
        "text" => format_text(json, obj),
        "thinking" => format_thinking(json, obj),
        "error" => format_error(json, obj),
        _ => format_json_value_lines(json, ""),
    }
}

fn format_tool_use(
    json: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<String> {
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let input = obj
        .get("input")
        .or_else(|| obj.get("parameters"))
        .or_else(|| obj.get("args"))
        .or_else(|| obj.get("arguments"));

    let mut lines = vec![format!("🔧 Tool: {}", name)];
    match input {
        Some(input) => {
            lines.push("  input:".to_string());
            lines.extend(format_json_value_lines(input, "    "));
        }
        None => {
            lines.push("  input: (missing)".to_string());
            lines.extend(format_json_value_lines(json, "  "));
        }
    }
    lines
}

fn format_tool_result(
    json: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<String> {
    let name = obj
        .get("tool_name")
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("tool"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let output = obj
        .get("output")
        .or_else(|| obj.get("result"))
        .or_else(|| obj.get("content"))
        .or_else(|| obj.get("data"));

    let mut lines = vec![format!("✓ Tool result: {}", name)];
    match output {
        Some(output) => {
            lines.push("  output:".to_string());
            lines.extend(format_json_value_lines(output, "    "));
        }
        None => {
            lines.push("  output: (missing)".to_string());
            lines.extend(format_json_value_lines(json, "  "));
        }
    }
    lines
}

fn format_text(
    json: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<String> {
    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
        return text.lines().map(|line| line.to_string()).collect();
    }
    if let Some(content) = obj.get("content") {
        return format_json_value_lines(content, "");
    }
    format_json_value_lines(json, "")
}

fn format_thinking(
    json: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<String> {
    let thinking = obj
        .get("thinking")
        .or_else(|| obj.get("text"))
        .or_else(|| obj.get("content"))
        .or_else(|| obj.get("message"));

    let mut lines = vec!["💭 Thinking:".to_string()];
    match thinking {
        Some(thinking) => lines.extend(format_json_value_lines(thinking, "  ")),
        None => {
            lines.push("  (missing)".to_string());
            lines.extend(format_json_value_lines(json, "  "));
        }
    }
    lines
}

fn format_error(
    json: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(message) = obj.get("message").and_then(|v| v.as_str()) {
        lines.push(format!("❌ Error: {message}"));
    } else {
        lines.push("❌ Error".to_string());
    }
    lines.extend(format_json_value_lines(json, "  "));
    lines
}

fn format_json_value_lines(value: &serde_json::Value, indent: &str) -> Vec<String> {
    match value {
        serde_json::Value::String(text) => text
            .lines()
            .map(|line| format!("{indent}{line}"))
            .collect(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            pretty
                .lines()
                .map(|line| format!("{indent}{line}"))
                .collect()
        }
        _ => vec![format!("{indent}{value}")],
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
