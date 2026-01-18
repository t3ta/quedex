use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use anyhow::Result;
use tempdir::TempDir;

use quedex::plan::{ShellConfig, Task, TaskMode};
use quedex::runner::shell::ShellRunner;
use quedex::runner::{RunContext, Runner};
use quedex::store::fs::FsStore;
use quedex::store::{LogStream, Store};

fn shell_task(id: &str, command: &str) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        kind: None,
        codex: None,
        shell: Some(ShellConfig {
            command: command.to_string(),
        }),
        claude_code: None,
    }
}

#[tokio::test]
async fn shell_runner_writes_stdout_and_stderr() -> Result<()> {
    let temp = TempDir::new("quedex-runner")?;
    let store: Arc<dyn Store> = Arc::new(FsStore::new(temp.path(), "run-stdout")?);

    let ctx = RunContext {
        cwd: temp.path().to_path_buf(),
        env: HashMap::new(),
        store: Arc::clone(&store),
    };

    let task = shell_task("task1", "printf 'out'; printf 'err' 1>&2");
    let runner = ShellRunner::new();
    let handle = runner.spawn(&task, &ctx)?;
    let status = handle.wait()?;
    assert!(status.success());

    let stdout = fs::read_to_string(&handle.stdout_path)?;
    let stderr = fs::read_to_string(&handle.stderr_path)?;

    assert_eq!(stdout, "out");
    assert_eq!(stderr, "err");
    assert_eq!(handle.stdout_path, store.log_path(&task.id, LogStream::Stdout));
    assert_eq!(handle.stderr_path, store.log_path(&task.id, LogStream::Stderr));

    Ok(())
}

#[tokio::test]
async fn shell_runner_respects_cwd_and_env() -> Result<()> {
    let temp = TempDir::new("quedex-runner")?;
    let work_dir = temp.path().join("work");
    fs::create_dir_all(&work_dir)?;

    let store: Arc<dyn Store> = Arc::new(FsStore::new(temp.path(), "run-env")?);
    let ctx = RunContext {
        cwd: work_dir.clone(),
        env: HashMap::from([("QUEDX_TEST".to_string(), "hello".to_string())]),
        store: Arc::clone(&store),
    };

    let task = shell_task(
        "task-env",
        "printf '%s\n%s\n' \"$QUEDX_TEST\" \"$(pwd)\"",
    );
    let runner = ShellRunner::new();
    let handle = runner.spawn(&task, &ctx)?;
    let status = handle.wait()?;
    assert!(status.success());

    let stdout = fs::read_to_string(&handle.stdout_path)?;
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "hello");
    let expected_cwd = fs::canonicalize(&work_dir)?;
    assert_eq!(lines[1], expected_cwd.to_string_lossy());

    Ok(())
}
