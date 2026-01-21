use std::collections::HashMap;
use std::process::Command;

use anyhow::Result;
use chrono::{Duration, Utc};
use tempdir::TempDir;

use quedex::store::fs::FsStore;
use quedex::store::{RunStatus, State, Store, TaskState, TaskStatus};

/// Helper to create a state with specific parameters
fn create_test_state(
    run_id: &str,
    run_name: &str,
    status: RunStatus,
    started_offset_hours: i64,
    duration_seconds: Option<i64>,
    tasks: Vec<(&str, TaskStatus, Option<i64>)>,
) -> State {
    let now = Utc::now();
    let started_at = now - Duration::hours(started_offset_hours);
    let completed_at = duration_seconds.map(|d| started_at + Duration::seconds(d));

    let mut task_states = HashMap::new();
    for (task_id, task_status, task_duration) in tasks {
        let task_started = Some(started_at);
        let task_completed = task_duration.map(|d| started_at + Duration::seconds(d));
        task_states.insert(
            task_id.to_string(),
            TaskState {
                status: task_status,
                exit_code: if task_status == TaskStatus::Failed {
                    Some(1)
                } else if task_status == TaskStatus::Succeeded {
                    Some(0)
                } else {
                    None
                },
                stderr_tail: None,
                started_at: task_started,
                completed_at: task_completed,
                pid: None,
                skip_reason: None,
            },
        );
    }

    State {
        run_id: run_id.to_string(),
        run_name: run_name.to_string(),
        status,
        tasks: task_states,
        started_at,
        completed_at,
    }
}

#[test]
fn stats_with_empty_store_shows_zero_runs() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Run quedex stats with the empty store
    let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Total runs:       0"));
    assert!(stdout.contains("Success rate:     0.0%"));

    Ok(())
}

#[test]
fn stats_calculates_success_rate_correctly() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Create 3 successful runs and 1 failed run
    for (i, status) in [
        RunStatus::Completed,
        RunStatus::Completed,
        RunStatus::Completed,
        RunStatus::Failed,
    ]
    .iter()
    .enumerate()
    {
        let run_id = format!("run-{}", i);
        let store = FsStore::new(temp.path(), &run_id)?;
        let state = create_test_state(
            &run_id,
            "test-run",
            *status,
            i as i64,
            Some(60),
            vec![("task1", TaskStatus::Succeeded, Some(30))],
        );
        store.write_state(state)?;
    }

    let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Total runs:       4"));
    assert!(stdout.contains("Success rate:     75.0%"));

    Ok(())
}

#[test]
fn stats_finds_most_failed_task() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Create runs where "build" fails 3 times and "test" fails 1 time
    let store1 = FsStore::new(temp.path(), "run-1")?;
    store1.write_state(create_test_state(
        "run-1",
        "test",
        RunStatus::Failed,
        1,
        Some(60),
        vec![
            ("build", TaskStatus::Failed, Some(30)),
            ("test", TaskStatus::Succeeded, Some(20)),
        ],
    ))?;

    let store2 = FsStore::new(temp.path(), "run-2")?;
    store2.write_state(create_test_state(
        "run-2",
        "test",
        RunStatus::Failed,
        2,
        Some(60),
        vec![
            ("build", TaskStatus::Failed, Some(30)),
            ("test", TaskStatus::Failed, Some(20)),
        ],
    ))?;

    let store3 = FsStore::new(temp.path(), "run-3")?;
    store3.write_state(create_test_state(
        "run-3",
        "test",
        RunStatus::Failed,
        3,
        Some(60),
        vec![
            ("build", TaskStatus::Failed, Some(30)),
            ("test", TaskStatus::Succeeded, Some(20)),
        ],
    ))?;

    let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Most failed task: build (3 failures)"));

    Ok(())
}

#[test]
fn stats_finds_longest_task() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Create runs where "slow-task" takes longer on average
    let store1 = FsStore::new(temp.path(), "run-1")?;
    store1.write_state(create_test_state(
        "run-1",
        "test",
        RunStatus::Completed,
        1,
        Some(200),
        vec![
            ("fast-task", TaskStatus::Succeeded, Some(30)),
            ("slow-task", TaskStatus::Succeeded, Some(180)),
        ],
    ))?;

    let store2 = FsStore::new(temp.path(), "run-2")?;
    store2.write_state(create_test_state(
        "run-2",
        "test",
        RunStatus::Completed,
        2,
        Some(220),
        vec![
            ("fast-task", TaskStatus::Succeeded, Some(40)),
            ("slow-task", TaskStatus::Succeeded, Some(200)),
        ],
    ))?;

    let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    // slow-task average is (180+200)/2 = 190 seconds = 3m 10s
    assert!(stdout.contains("Longest task:     slow-task"));

    Ok(())
}

#[test]
fn stats_json_output_is_valid() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    let store = FsStore::new(temp.path(), "run-1")?;
    store.write_state(create_test_state(
        "run-1",
        "test",
        RunStatus::Completed,
        1,
        Some(120),
        vec![("task1", TaskStatus::Succeeded, Some(60))],
    ))?;

    let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .arg("--json")
        .output()?;

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)?;

    assert_eq!(json["total_runs"], 1);
    assert_eq!(json["successful_runs"], 1);
    assert_eq!(json["failed_runs"], 0);
    assert!(json["period"]["until"].is_string());

    Ok(())
}

#[test]
fn stats_since_filter_works() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Create an old run (48 hours ago) and a recent run (1 hour ago)
    let store_old = FsStore::new(temp.path(), "run-old")?;
    store_old.write_state(create_test_state(
        "run-old",
        "old-run",
        RunStatus::Completed,
        48, // 48 hours ago
        Some(60),
        vec![("task1", TaskStatus::Succeeded, Some(30))],
    ))?;

    let store_new = FsStore::new(temp.path(), "run-new")?;
    store_new.write_state(create_test_state(
        "run-new",
        "new-run",
        RunStatus::Completed,
        1, // 1 hour ago
        Some(60),
        vec![("task1", TaskStatus::Succeeded, Some(30))],
    ))?;

    // Without filter: should see both runs
    let output_all = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .output()?;
    let stdout_all = String::from_utf8_lossy(&output_all.stdout);
    assert!(stdout_all.contains("Total runs:       2"));

    // With --since 24h: should see only the recent run
    let output_filtered = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .arg("--since")
        .arg("24h")
        .output()?;
    let stdout_filtered = String::from_utf8_lossy(&output_filtered.stdout);
    assert!(stdout_filtered.contains("Total runs:       1"));

    Ok(())
}

#[test]
fn stats_handles_various_duration_formats() -> Result<()> {
    let temp = TempDir::new("quedex-stats")?;

    // Create a run
    let store = FsStore::new(temp.path(), "run-1")?;
    store.write_state(create_test_state(
        "run-1",
        "test",
        RunStatus::Completed,
        1,
        Some(60),
        vec![("task1", TaskStatus::Succeeded, Some(30))],
    ))?;

    // Test various duration formats
    for duration in &["1h", "2d", "1w", "30m", "60s"] {
        let output = Command::new(env!("CARGO_BIN_EXE_quedex"))
            .arg("--store")
            .arg(temp.path())
            .arg("stats")
            .arg("--since")
            .arg(duration)
            .output()?;
        assert!(
            output.status.success(),
            "Failed for duration: {}",
            duration
        );
    }

    // Test invalid duration format
    let output_invalid = Command::new(env!("CARGO_BIN_EXE_quedex"))
        .arg("--store")
        .arg(temp.path())
        .arg("stats")
        .arg("--since")
        .arg("invalid")
        .output()?;
    assert!(!output_invalid.status.success());

    Ok(())
}
