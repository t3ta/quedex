use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use chrono::Utc;
use tempdir::TempDir;

use quedex::plan::{Plan, RunConfig, ShellConfig, Task, TaskMode};
use quedex::store::fs::FsStore;
use quedex::store::{RunStatus, State, Store, TaskState, TaskStatus};

fn shell_task(id: &str, command: String, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        timeout_sec: None,
        kind: None,
        codex: None,
        shell: Some(ShellConfig { command }),
    }
}

fn write_plan(path: &Path, plan: &Plan) -> Result<()> {
    let file = File::create(path).with_context(|| format!("open {}", path.display()))?;
    serde_json::to_writer_pretty(file, plan).context("write plan")?;
    Ok(())
}

fn run_dir(store_root: &Path, run_id: &str) -> PathBuf {
    store_root.join("runs").join(run_id)
}

fn quedex_bin() -> &'static str {
    env!("CARGO_BIN_EXE_quedex")
}

fn write_state_for_run(store_root: &Path, run_id: &str, status: RunStatus) -> Result<()> {
    let store = FsStore::new(store_root, run_id)?;
    let now = Utc::now();
    let (task_status, exit_code, completed_at) = match status {
        RunStatus::Completed => (TaskStatus::Succeeded, Some(0), Some(now)),
        RunStatus::Failed => (TaskStatus::Failed, Some(1), Some(now)),
        RunStatus::Canceled => (TaskStatus::Canceled, None, Some(now)),
        RunStatus::Running => (TaskStatus::Running, None, None),
    };
    let mut tasks = HashMap::new();
    tasks.insert(
        "task".to_string(),
        TaskState {
            status: task_status,
            exit_code,
            stderr_tail: None,
            started_at: Some(now),
            completed_at,
            pid: None,
        },
    );
    let state = State {
        run_id: run_id.to_string(),
        run_name: run_id.to_string(),
        status,
        tasks,
        started_at: now,
        completed_at,
    };
    store.write_state(state)?;
    Ok(())
}

#[test]
fn retry_command_reexecutes_failed_task() -> Result<()> {
    let temp = TempDir::new("quedex-retry")?;
    let store_root = temp.path().join("store");
    let work_dir = temp.path().join("work");
    fs::create_dir_all(&work_dir)?;

    let marker = work_dir.join("retry.marker");
    let command = format!(
        "if [ -f \"{path}\" ]; then echo ok; exit 0; else touch \"{path}\"; echo fail 1>&2; exit 1; fi",
        path = marker.display()
    );

    let plan = Plan {
        version: 1,
        run: RunConfig {
            cwd: Some(work_dir.clone()),
            ..Default::default()
        },
        tasks: vec![shell_task("retry-task", command, vec![])],
    };
    let plan_path = work_dir.join("plan.json");
    write_plan(&plan_path, &plan)?;

    let run_id = "retry-run";
    let status = Command::new(quedex_bin())
        .arg("--store")
        .arg(&store_root)
        .arg("run")
        .arg(&plan_path)
        .arg("--run-id")
        .arg(run_id)
        .status()
        .context("run plan")?;
    assert_eq!(status.code(), Some(1));

    let store = FsStore::new(&store_root, run_id)?;
    let state = store.read_state()?;
    assert_eq!(state.status, RunStatus::Failed);
    assert_eq!(state.tasks["retry-task"].status, TaskStatus::Failed);

    let status = Command::new(quedex_bin())
        .arg("--store")
        .arg(&store_root)
        .arg("retry")
        .arg(run_id)
        .arg("retry-task")
        .status()
        .context("retry task")?;
    assert!(status.success());

    let state = store.read_state()?;
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.tasks["retry-task"].status, TaskStatus::Succeeded);

    Ok(())
}

#[cfg(unix)]
#[test]
fn clean_all_removes_finished_runs_and_skips_running() -> Result<()> {
    let temp = TempDir::new("quedex-clean")?;
    let store_root = temp.path().join("store");
    fs::create_dir_all(&store_root)?;

    write_state_for_run(&store_root, "completed-run", RunStatus::Completed)?;
    write_state_for_run(&store_root, "failed-run", RunStatus::Failed)?;

    let running_run = "running-run";
    let running_dir = run_dir(&store_root, running_run);
    fs::create_dir_all(&running_dir)?;
    fs::write(running_dir.join("run.pid"), std::process::id().to_string())?;

    let status = Command::new(quedex_bin())
        .arg("--store")
        .arg(&store_root)
        .arg("clean")
        .arg("--all")
        .status()
        .context("clean all")?;
    assert!(status.success());

    assert!(!run_dir(&store_root, "completed-run").exists());
    assert!(!run_dir(&store_root, "failed-run").exists());
    assert!(run_dir(&store_root, "running-run").exists());

    Ok(())
}

#[test]
fn tui_starts_in_headless_mode() -> Result<()> {
    let temp = TempDir::new("quedex-tui")?;
    let store_root = temp.path().join("store");
    let work_dir = temp.path().join("work");
    fs::create_dir_all(&work_dir)?;

    let plan = Plan {
        version: 1,
        run: RunConfig::default(),
        tasks: vec![shell_task("task1", "printf 'ok'".to_string(), vec![])],
    };

    let run_id = "tui-run";
    let store = FsStore::new(&store_root, run_id)?;
    let run_path = run_dir(&store_root, run_id);
    fs::create_dir_all(&run_path)?;
    write_plan(&run_path.join("plan.json"), &plan)?;

    let now = Utc::now();
    let mut tasks = HashMap::new();
    tasks.insert(
        "task1".to_string(),
        TaskState {
            status: TaskStatus::Succeeded,
            exit_code: Some(0),
            stderr_tail: None,
            started_at: Some(now),
            completed_at: Some(now),
            pid: None,
        },
    );
    let state = State {
        run_id: run_id.to_string(),
        run_name: run_id.to_string(),
        status: RunStatus::Completed,
        tasks,
        started_at: now,
        completed_at: Some(now),
    };
    store.write_state(state)?;

    let status = Command::new(quedex_bin())
        .arg("--store")
        .arg(&store_root)
        .arg("tui")
        .arg(run_id)
        .env("QUEDX_TUI_HEADLESS", "1")
        .status()
        .context("run headless tui")?;
    assert!(status.success());

    Ok(())
}

#[test]
fn resume_recovers_running_tasks_and_runs_pending() -> Result<()> {
    let temp = TempDir::new("quedex-resume")?;
    let store_root = temp.path().join("store");
    let work_dir = temp.path().join("work");
    fs::create_dir_all(&work_dir)?;

    let a_file = work_dir.join("a.done");
    let b_file = work_dir.join("b.done");
    let plan = Plan {
        version: 1,
        run: RunConfig {
            cwd: Some(work_dir.clone()),
            fail_fast: Some(false),
            ..Default::default()
        },
        tasks: vec![
            shell_task(
                "a",
                format!("printf 'a' > \"{}\"", a_file.display()),
                vec![],
            ),
            shell_task(
                "b",
                format!("printf 'b' > \"{}\"", b_file.display()),
                vec![],
            ),
        ],
    };
    let plan_path = work_dir.join("plan.json");
    write_plan(&plan_path, &plan)?;

    let run_id = "resume-run";
    let store = FsStore::new(&store_root, run_id)?;
    let run_path = run_dir(&store_root, run_id);
    fs::create_dir_all(&run_path)?;
    write_plan(&run_path.join("plan.json"), &plan)?;

    let now = Utc::now();
    let mut tasks = HashMap::new();
    tasks.insert(
        "a".to_string(),
        TaskState {
            status: TaskStatus::Running,
            exit_code: None,
            stderr_tail: None,
            started_at: Some(now),
            completed_at: None,
            pid: None,
        },
    );
    tasks.insert(
        "b".to_string(),
        TaskState {
            status: TaskStatus::Pending,
            exit_code: None,
            stderr_tail: None,
            started_at: None,
            completed_at: None,
            pid: None,
        },
    );
    let state = State {
        run_id: run_id.to_string(),
        run_name: run_id.to_string(),
        status: RunStatus::Running,
        tasks,
        started_at: now,
        completed_at: None,
    };
    store.write_state(state)?;

    let status = Command::new(quedex_bin())
        .arg("--store")
        .arg(&store_root)
        .arg("run")
        .arg(&plan_path)
        .arg("--run-id")
        .arg(run_id)
        .arg("--resume")
        .status()
        .context("resume run")?;
    assert_eq!(status.code(), Some(1));

    let state = store.read_state()?;
    assert_eq!(state.tasks["a"].status, TaskStatus::Failed);
    assert_eq!(state.tasks["b"].status, TaskStatus::Succeeded);
    assert_eq!(state.status, RunStatus::Failed);
    assert!(b_file.exists());

    Ok(())
}
