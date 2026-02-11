use std::collections::HashMap;
use std::fs;
use std::io::Write;

use anyhow::Result;
use chrono::Utc;
use tempdir::TempDir;

use quedex::store::fs::FsStore;
use quedex::store::{Event, LogStream, RunStatus, State, TaskState, TaskStatus, Store};

#[test]
fn fs_store_appends_events_and_writes_state() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-events")?;
    let ts = Utc::now();

    store.append_event(Event::RunStarted {
        run_id: "run-events".to_string(),
        timestamp: ts,
    })?;
    store.append_event(Event::TaskStarted {
        task_id: "task1".to_string(),
        pid: 42,
        timestamp: ts,
    })?;

    let mut tasks = HashMap::new();
    tasks.insert(
        "task1".to_string(),
        TaskState {
            status: TaskStatus::Running,
            exit_code: None,
            stderr_tail: None,
            started_at: Some(ts),
            completed_at: None,
            output_files: None,
            pid: Some(42),
            skip_reason: None,
        },
    );

    let state = State {
        run_id: "run-events".to_string(),
        run_name: "test-run".to_string(),
        status: RunStatus::Running,
        tasks,
        started_at: ts,
        completed_at: None,
    };

    store.write_state(state.clone())?;
    let read_state = store.read_state()?;

    assert_eq!(read_state.run_id, state.run_id);
    assert_eq!(read_state.run_name, state.run_name);
    assert_eq!(read_state.status, state.status);
    assert_eq!(
        read_state
            .tasks
            .get("task1")
            .expect("task1 missing")
            .status,
        TaskStatus::Running
    );

    let events_path = temp
        .path()
        .join("runs")
        .join("run-events")
        .join("events.jsonl");
    let contents = fs::read_to_string(events_path)?;
    let events: Vec<Event> = contents
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse event"))
        .collect();

    assert_eq!(events.len(), 2);
    match &events[0] {
        Event::RunStarted { run_id, .. } => assert_eq!(run_id, "run-events"),
        _ => panic!("unexpected event type"),
    }
    match &events[1] {
        Event::TaskStarted { task_id, pid, .. } => {
            assert_eq!(task_id, "task1");
            assert_eq!(*pid, 42);
        }
        _ => panic!("unexpected event type"),
    }

    Ok(())
}

#[test]
fn fs_store_writes_logs_to_expected_paths() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-logs")?;

    let mut stdout = store.open_log("task1", LogStream::Stdout)?;
    let mut stderr = store.open_log("task1", LogStream::Stderr)?;

    write!(stdout, "hello")?;
    write!(stderr, "oops")?;
    stdout.flush()?;
    stderr.flush()?;

    let stdout_path = store.log_path("task1", LogStream::Stdout);
    let stderr_path = store.log_path("task1", LogStream::Stderr);

    let expected_dir = temp
        .path()
        .join("runs")
        .join("run-logs")
        .join("tasks")
        .join("task1");

    assert_eq!(stdout_path, expected_dir.join("stdout.log"));
    assert_eq!(stderr_path, expected_dir.join("stderr.log"));

    assert_eq!(fs::read_to_string(stdout_path)?, "hello");
    assert_eq!(fs::read_to_string(stderr_path)?, "oops");

    Ok(())
}

#[test]
fn fs_store_saves_and_gets_context() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-context")?;

    let content = b"This is the auth analysis result.";
    store.save_context("auth_analysis", content)?;

    let loaded = store.get_context("auth_analysis")?;
    assert_eq!(loaded, content);

    // Verify file exists at expected path
    let context_path = temp
        .path()
        .join("runs")
        .join("run-context")
        .join("context")
        .join("auth_analysis");
    assert!(context_path.exists());

    Ok(())
}

#[test]
fn fs_store_context_overwrite() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-overwrite")?;

    store.save_context("key1", b"first")?;
    store.save_context("key1", b"second")?;

    let loaded = store.get_context("key1")?;
    assert_eq!(loaded, b"second");

    Ok(())
}

#[test]
fn fs_store_context_rejects_empty_key() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-empty").unwrap();

    let result = store.save_context("", b"data");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("context key is empty"));
}

#[test]
fn fs_store_context_rejects_invalid_key() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-invalid").unwrap();

    let result = store.save_context("key/with/slash", b"data");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid characters"));
}

#[test]
fn fs_store_get_context_returns_error_for_missing_key() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-missing").unwrap();

    let result = store.get_context("nonexistent");
    assert!(result.is_err());
}
