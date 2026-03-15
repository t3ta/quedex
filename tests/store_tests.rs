use std::collections::HashMap;
use std::fs;
use std::io::Write;

use anyhow::Result;
use chrono::Utc;
use tempdir::TempDir;

use quedex::store::fs::FsStore;
use quedex::store::{Event, LogStream, RunStatus, State, Store, TaskState, TaskStatus};

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
        read_state.tasks.get("task1").expect("task1 missing").status,
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
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("context key is empty")
    );
}

#[test]
fn fs_store_context_rejects_invalid_key() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-invalid").unwrap();

    let result = store.save_context("key/with/slash", b"data");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid characters")
    );
}

#[test]
fn fs_store_get_context_returns_error_for_missing_key() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-missing").unwrap();

    let result = store.get_context("nonexistent");
    assert!(result.is_err());
}

// ==================== Atomic write tests ====================

#[test]
fn fs_store_write_state_atomic_recovery() -> Result<()> {
    // Test that if a temporary file exists, the main state file is still readable
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-atomic")?;
    let ts = Utc::now();

    // Write initial state
    let mut tasks = HashMap::new();
    tasks.insert(
        "task1".to_string(),
        TaskState {
            status: TaskStatus::Succeeded,
            exit_code: Some(0),
            stderr_tail: None,
            started_at: Some(ts),
            completed_at: Some(ts),
            output_files: None,
            pid: Some(100),
            skip_reason: None,
        },
    );
    let state = State {
        run_id: "run-atomic".to_string(),
        run_name: "test-run".to_string(),
        status: RunStatus::Completed,
        tasks,
        started_at: ts,
        completed_at: Some(ts),
    };
    store.write_state(state.clone())?;

    // Simulate a crash by manually creating a tmp file (simulating interrupted write)
    let tmp_path = temp
        .path()
        .join("runs")
        .join("run-atomic")
        .join("state.json.tmp");
    fs::write(&tmp_path, b"corrupted partial data")?;

    // Read should still return the valid state
    let read_state = store.read_state()?;
    assert_eq!(read_state.run_id, state.run_id);
    assert_eq!(read_state.status, RunStatus::Completed);

    Ok(())
}

#[test]
fn fs_store_write_state_multiple_writes() -> Result<()> {
    // Test that multiple consecutive writes work correctly
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-multi-write")?;
    let ts = Utc::now();

    for i in 0..10 {
        let mut tasks = HashMap::new();
        tasks.insert(
            format!("task{}", i),
            TaskState {
                status: TaskStatus::Succeeded,
                exit_code: Some(0),
                stderr_tail: None,
                started_at: Some(ts),
                completed_at: Some(ts),
                output_files: None,
                pid: Some(100 + i as u32),
                skip_reason: None,
            },
        );
        let state = State {
            run_id: "run-multi-write".to_string(),
            run_name: format!("test-run-{}", i),
            status: RunStatus::Running,
            tasks,
            started_at: ts,
            completed_at: None,
        };
        store.write_state(state)?;
    }

    let read_state = store.read_state()?;
    assert_eq!(read_state.run_name, "test-run-9");
    assert!(read_state.tasks.contains_key("task9"));

    Ok(())
}

// ==================== Output file tests ====================

#[test]
fn fs_store_saves_and_gets_output() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-output")?;

    let content = b"output data from task";
    let path = store.save_output("task1", "result.json", content)?;

    // Verify path is correct
    let expected = temp
        .path()
        .join("runs")
        .join("run-output")
        .join("outputs")
        .join("task1")
        .join("result.json");
    assert_eq!(path, expected);

    // Verify content
    let loaded = store.get_output("task1", "result.json")?;
    assert_eq!(loaded, content);

    Ok(())
}

#[test]
fn fs_store_lists_outputs() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-list-output")?;

    // Save multiple outputs
    store.save_output("task1", "file1.txt", b"content1")?;
    store.save_output("task1", "file2.json", b"content2")?;
    store.save_output("task1", "subdir/file3.md", b"content3")?;

    let outputs = store.list_outputs("task1")?;
    assert_eq!(outputs.len(), 3);
    assert!(outputs.contains(&"file1.txt".to_string()));
    assert!(outputs.contains(&"file2.json".to_string()));
    assert!(outputs.contains(&"subdir/file3.md".to_string()));

    Ok(())
}

#[test]
fn fs_store_output_overwrite() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-output-overwrite")?;

    store.save_output("task1", "result.txt", b"first")?;
    store.save_output("task1", "result.txt", b"second")?;

    let loaded = store.get_output("task1", "result.txt")?;
    assert_eq!(loaded, b"second");

    Ok(())
}

#[test]
fn fs_store_output_rejects_absolute_path() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-output-abs").unwrap();

    let result = store.save_output("task1", "/etc/passwd", b"data");
    assert!(result.is_err());
}

#[test]
fn fs_store_output_rejects_path_traversal() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-output-traversal").unwrap();

    let result = store.save_output("task1", "../escape.txt", b"data");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("must not contain '..'")
    );
}

#[test]
fn fs_store_get_output_returns_error_for_missing_file() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-output-missing").unwrap();

    let result = store.get_output("task1", "nonexistent.txt");
    assert!(result.is_err());
}

// ==================== Context versioning tests ====================

#[test]
fn fs_store_context_versioned_new() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-version")?;

    let content = b"initial content";
    let meta = store.save_context_versioned("mykey", content, "task_a", None)?;

    assert_eq!(meta.version, 1);
    assert_eq!(meta.updated_by, "task_a");

    // Content should be readable
    let loaded = store.get_context("mykey")?;
    assert_eq!(loaded, content);

    Ok(())
}

#[test]
fn fs_store_context_versioned_increment() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-version-inc")?;

    // First save
    let meta1 = store.save_context_versioned("mykey", b"v1", "task_a", None)?;
    assert_eq!(meta1.version, 1);

    // Second save
    let meta2 = store.save_context_versioned("mykey", b"v2", "task_b", None)?;
    assert_eq!(meta2.version, 2);
    assert_eq!(meta2.updated_by, "task_b");

    // Third save
    let meta3 = store.save_context_versioned("mykey", b"v3", "task_c", None)?;
    assert_eq!(meta3.version, 3);

    // Content should be latest
    let loaded = store.get_context("mykey")?;
    assert_eq!(loaded, b"v3");

    Ok(())
}

#[test]
fn fs_store_context_versioned_optimistic_lock_success() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-optlock")?;

    // Create initial version
    let meta1 = store.save_context_versioned("mykey", b"v1", "task_a", None)?;
    assert_eq!(meta1.version, 1);

    // Update with correct expected version
    let meta2 = store.save_context_versioned("mykey", b"v2", "task_b", Some(1))?;
    assert_eq!(meta2.version, 2);

    Ok(())
}

#[test]
fn fs_store_context_versioned_optimistic_lock_failure() {
    let temp = TempDir::new("quedex-store").unwrap();
    let store = FsStore::new(temp.path(), "run-ctx-optlock-fail").unwrap();

    // Create initial version
    store
        .save_context_versioned("mykey", b"v1", "task_a", None)
        .unwrap();

    // Try to update with wrong expected version
    let result = store.save_context_versioned("mykey", b"v2", "task_b", Some(5));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("version conflict"));
}

#[test]
fn fs_store_context_versioned_expect_zero_for_new() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-expect-zero")?;

    // Create new with expected version 0 (no existing entry)
    let meta = store.save_context_versioned("mykey", b"v1", "task_a", Some(0))?;
    assert_eq!(meta.version, 1);

    Ok(())
}

#[test]
fn fs_store_get_context_metadata() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-getmeta")?;

    // No metadata for non-existent key
    let meta = store.get_context_metadata("nonexistent")?;
    assert!(meta.is_none());

    // Create versioned context
    store.save_context_versioned("mykey", b"content", "task_a", None)?;

    // Metadata should exist
    let meta = store.get_context_metadata("mykey")?;
    assert!(meta.is_some());
    let meta = meta.unwrap();
    assert_eq!(meta.version, 1);
    assert_eq!(meta.updated_by, "task_a");

    Ok(())
}

#[test]
fn fs_store_context_versioned_with_regular_save() -> Result<()> {
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-ctx-mixed")?;

    // Save with regular method (no metadata)
    store.save_context("mykey", b"v1")?;

    // No metadata should exist
    let meta = store.get_context_metadata("mykey")?;
    assert!(meta.is_none());

    // Save versioned on top (creates metadata, version 1)
    let meta = store.save_context_versioned("mykey", b"v2", "task_a", None)?;
    assert_eq!(meta.version, 1); // First versioned save

    Ok(())
}

// ==================== State consistency tests ====================

#[test]
fn fs_store_state_round_trip() -> Result<()> {
    // Test that all State fields survive a round trip
    let temp = TempDir::new("quedex-store")?;
    let store = FsStore::new(temp.path(), "run-roundtrip")?;
    let ts = Utc::now();

    let mut tasks = HashMap::new();
    tasks.insert(
        "task1".to_string(),
        TaskState {
            status: TaskStatus::Succeeded,
            exit_code: Some(0),
            stderr_tail: Some("some error output".to_string()),
            started_at: Some(ts),
            completed_at: Some(ts),
            output_files: Some(vec!["out1.txt".to_string(), "out2.json".to_string()]),
            pid: Some(12345),
            skip_reason: None,
        },
    );
    tasks.insert(
        "task2".to_string(),
        TaskState {
            status: TaskStatus::Failed,
            exit_code: Some(1),
            stderr_tail: Some("error".to_string()),
            started_at: Some(ts),
            completed_at: Some(ts),
            output_files: None,
            pid: Some(12346),
            skip_reason: None,
        },
    );

    let state = State {
        run_id: "run-roundtrip".to_string(),
        run_name: "complex-test-run".to_string(),
        status: RunStatus::Failed,
        tasks,
        started_at: ts,
        completed_at: Some(ts),
    };

    store.write_state(state.clone())?;
    let read_state = store.read_state()?;

    // Verify all fields
    assert_eq!(read_state.run_id, state.run_id);
    assert_eq!(read_state.run_name, state.run_name);
    assert_eq!(read_state.status, state.status);
    assert_eq!(read_state.tasks.len(), 2);

    let task1 = read_state.tasks.get("task1").unwrap();
    assert_eq!(task1.status, TaskStatus::Succeeded);
    assert_eq!(task1.exit_code, Some(0));
    assert_eq!(task1.stderr_tail, Some("some error output".to_string()));
    assert_eq!(
        task1.output_files,
        Some(vec!["out1.txt".to_string(), "out2.json".to_string()])
    );

    let task2 = read_state.tasks.get("task2").unwrap();
    assert_eq!(task2.status, TaskStatus::Failed);
    assert_eq!(task2.exit_code, Some(1));

    Ok(())
}
