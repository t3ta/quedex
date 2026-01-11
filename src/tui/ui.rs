use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

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

    let text = Text::from(lines.join("\n"));
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((scroll as u16, 0));

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
    let mut lines = Vec::new();
    for task in &app.plan.tasks {
        if task.deps.is_empty() {
            lines.push(task.id.clone());
        }
        for dep in &task.deps {
            lines.push(format!("{dep} -> {}", task.id));
        }
    }
    if lines.is_empty() {
        lines.push("no graph data".to_string());
    }

    let text = Text::from(lines.join("\n"));
    let block = Block::default()
        .title("Graph (press g to return)")
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, frame.size());
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
