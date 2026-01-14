use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table};
use std::collections::HashMap;

use crate::store::{LogStream, TaskStatus};

use super::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    if app.graph_mode {
        draw_graph(frame, app);
    } else {
        draw_main(frame, app);
    }
}

fn draw_main(frame: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(frame.size());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(root[0]);

    draw_tasks(frame, app, top[0]);
    draw_logs(frame, app, top[1]);
    draw_summary(frame, app, root[1]);
}

fn draw_tasks(frame: &mut Frame, app: &mut App, area: Rect) {
    let now = chrono::Utc::now();
    let rows = app.tasks.iter().map(|task| {
        let status = app.task_status(&task.id);
        let status_text = format!("{status:?}");
        let duration = app.task_duration(&task.id, now);
        let deps = app.deps_remaining(task).to_string();
        let title = task.title.clone();
        let row = Row::new(vec![
            Cell::from(task.id.clone()),
            Cell::from(title),
            Cell::from(status_text),
            Cell::from(duration),
            Cell::from(deps),
        ]);
        row.style(status_style(status))
    });

    let header = Row::new(vec!["id", "title", "status", "dur", "deps"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Length(10),
        Constraint::Min(16),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(5),
    ];

    let focus = if app.log_focus { " (log focus)" } else { "" };
    let block = Block::default()
        .title(format!("Tasks{focus}"))
        .borders(Borders::ALL);

    let highlight_style = if app.log_focus {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(highlight_style);

    frame.render_stateful_widget(table, area, &mut app.list_state);
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let task_label = app
        .selected_task_id()
        .unwrap_or("no task");
    let stream = match app.log_stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    };
    let focus = if app.log_focus { " (focus)" } else { "" };
    let title = format!("Logs: {task_label} [{stream}]{focus}");

    let lines = if app.log_lines.is_empty() {
        vec!["no log yet".to_string()]
    } else {
        app.log_lines.clone()
    };

    let height = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(height);
    let offset = app.log_offset.min(max_scroll);
    let scroll = max_scroll.saturating_sub(offset);

    let text_lines: Vec<Line> = lines.iter().map(|s| Line::from(s.as_str())).collect();
    let text = Text::from(text_lines);
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((scroll as u16, 0))
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn draw_summary(frame: &mut Frame, app: &mut App, area: Rect) {
    let stats = app.stats();
    let locks = app.active_locks();
    let locks_text = if locks.is_empty() {
        "none".to_string()
    } else {
        locks.join(",")
    };
    let mut summary = format!(
        "run: {} | status: {:?} | done: {}/{} | running: {} | failed: {} | locks: {}",
        app.run_id,
        app.state.status,
        stats.done,
        stats.total,
        stats.running,
        stats.failed,
        locks_text
    );
    if let Some(message) = app.status_message.as_ref() {
        summary.push_str(" | ");
        summary.push_str(message);
    }
    let paragraph = Paragraph::new(summary).block(Block::default().borders(Borders::ALL));
    frame.render_widget(paragraph, area);
}

fn draw_graph(frame: &mut Frame, app: &mut App) {
    let stats = app.stats();

    // レイアウト: 上部にGauge + 統計、下部にグラフ
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // プログレスバー
            Constraint::Length(3), // 統計情報
            Constraint::Min(10),   // グラフ
        ])
        .split(frame.size());

    // プログレスバーの描画
    let progress = if stats.total > 0 {
        (stats.done as f64 / stats.total as f64 * 100.0) as u16
    } else {
        0
    };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("全体進捗"))
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent(progress)
        .label(format!("{}/{} ({}%)", stats.done, stats.total, progress));
    frame.render_widget(gauge, layout[0]);

    // 統計情報の描画
    let locks = app.active_locks();
    let locks_text = if locks.is_empty() {
        "none".to_string()
    } else {
        locks.join(", ")
    };
    let stats_text = format!(
        "実行中: {} | 失敗: {} | ロック: {}",
        stats.running, stats.failed, locks_text
    );
    let stats_paragraph = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title("統計"));
    frame.render_widget(stats_paragraph, layout[1]);

    // 依存関係グラフの生成
    let graph_lines = build_dependency_graph(app);

    // グラフの描画
    let text = Text::from(graph_lines);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("依存関係グラフ (press g to return)"),
        )
        .scroll((0, 0));
    frame.render_widget(paragraph, layout[2]);
}

fn build_dependency_graph(app: &App) -> Vec<Line<'_>> {
    // 1. タスクの子を特定（このタスクに依存しているタスク）
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in &app.tasks {
        for dep in &task.deps {
            children
                .entry(dep.as_str())
                .or_default()
                .push(&task.id);
        }
    }

    // 2. ルートタスク（依存元がないタスク）を特定
    let roots: Vec<_> = app
        .tasks
        .iter()
        .filter(|task| task.deps.is_empty())
        .collect();

    if roots.is_empty() {
        return vec![Line::from("no tasks found")];
    }

    // 3. 再帰的にツリーを走査
    let mut lines = Vec::new();
    let now = chrono::Utc::now();
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from("")); // ルート間に空行を挿入
        }
        render_task_tree(root, &children, app, "", true, &mut lines, now);
    }

    lines
}

fn render_task_tree<'a>(
    task: &'a super::app::TaskInfo,
    children: &HashMap<&str, Vec<&str>>,
    app: &'a App,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<Line<'a>>,
    now: chrono::DateTime<chrono::Utc>,
) {
    let status = app.task_status(&task.id);
    let duration = app.task_duration(&task.id, now);

    // タスク行の生成
    let connector = if prefix.is_empty() {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    let line_text = format!(
        "{}{}{} {} [{:?}] {}",
        prefix, connector, "*", task.id, status, duration
    );

    // 色付けを適用
    let styled_line = Line::from(line_text).style(status_style(status));
    lines.push(styled_line);

    // 子タスクの描画
    if let Some(child_ids) = children.get(task.id.as_str()) {
        let child_count = child_ids.len();
        for (i, child_id) in child_ids.iter().enumerate() {
            // 親とのつながりを示す縦線を追加
            if i < child_count {
                let vertical_line = format!("{}│", prefix);
                lines.push(Line::from(vertical_line));
            }

            if let Some(child_task) = app.tasks.iter().find(|t| t.id == *child_id) {
                let is_last_child = i == child_count - 1;
                let new_prefix = if is_last {
                    format!("{}   ", prefix)
                } else {
                    format!("{}│  ", prefix)
                };
                render_task_tree(
                    child_task,
                    children,
                    app,
                    &new_prefix,
                    is_last_child,
                    lines,
                    now,
                );
            }
        }
    }
}

fn status_style(status: TaskStatus) -> Style {
    match status {
        TaskStatus::Running => Style::default().fg(Color::Yellow),
        TaskStatus::Succeeded => Style::default().fg(Color::Green),
        TaskStatus::Failed => Style::default().fg(Color::Red),
        TaskStatus::Canceled => Style::default().fg(Color::Magenta),
        TaskStatus::Skipped => Style::default().fg(Color::DarkGray),
        TaskStatus::Ready => Style::default().fg(Color::Cyan),
        TaskStatus::Pending => Style::default().fg(Color::Gray),
    }
}
