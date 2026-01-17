mod cli;

use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::Parser;
use uuid::Uuid;

use cli::{Cli, Commands, GlobalOptions, RecoveryOptions};
use quedex::plan::{Plan, PlanFormat, Task};
use quedex::runner::codex::CodexRunner;
use quedex::runner::shell::ShellRunner;
use quedex::runner::{ChildHandle, RunContext, Runner};
use quedex::scheduler::{
    ScheduleReport, Scheduler, SchedulerOptions, TaskRecord, TaskResult, TaskRunner, TaskSpec,
};
use quedex::store::fs::FsStore;
use quedex::store::recovery::recover_running_tasks;
use quedex::store::{Event, LogStream, RunStatus, State, TaskState, TaskStatus, Store};
use quedex::tui;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match dispatch(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{:#}", err);
            1
        }
    };
    std::process::exit(code);
}

async fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Commands::Run {
            plan,
            recovery,
            run_id,
            base_dir,
        } => handle_run(&cli.global, &plan, recovery, run_id, base_dir).await,
        Commands::Start {
            plan,
            recovery,
            run_id,
        } => handle_start(&cli.global, &plan, recovery, run_id).await,
        Commands::Status { run_id, json } => handle_status(&cli.global, run_id, json),
        Commands::Tui { run_id } => handle_tui(&cli.global, run_id),
        Commands::Logs {
            run_id,
            task_id,
            follow,
            stderr,
        } => handle_logs(&cli.global, &run_id, &task_id, follow, stderr),
        Commands::Retry { run_id, task_id } => handle_retry(&cli.global, &run_id, &task_id).await,
        Commands::Cancel { run_id, task_id } => handle_cancel(&cli.global, &run_id, task_id),
        Commands::Clean { run_id, all } => handle_clean(&cli.global, run_id, all),
        Commands::Graph {
            target,
            mermaid,
            ascii,
        } => handle_graph(&cli.global, &target, mermaid, ascii),
    }
}

async fn handle_run(
    global: &GlobalOptions,
    plan_arg: &str,
    recovery: RecoveryOptions,
    run_id: Option<String>,
    base_dir: Option<PathBuf>,
) -> Result<i32> {
    let (mut plan, mut plan_base_dir) = load_plan(plan_arg)?;
    if let Some(base_dir) = base_dir {
        plan_base_dir = base_dir;
    }
    let store_root = resolve_store_path(global.store.as_ref())?;
    if recovery.resume && run_id.is_none() {
        return Err(anyhow!("--resume requires --run-id"));
    }

    let run_id = run_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let run_dir = run_dir(&store_root, &run_id);
    if recovery.clean_start && run_dir.exists() {
        fs::remove_dir_all(&run_dir)
            .with_context(|| format!("clean start remove {}", run_dir.display()))?;
    }

    let state_path = run_dir.join("state.json");
    let snapshot_path = plan_snapshot_path(&store_root, &run_id);
    if recovery.resume {
        if !state_path.exists() {
            return Err(anyhow!("run {} has no state to resume", run_id));
        }
        if snapshot_path.exists() {
            plan = load_plan_snapshot(&store_root, &run_id)?;
        }
    } else if state_path.exists() {
        return Err(anyhow!(
            "run {} already exists (use --resume or --clean-start)",
            run_id
        ));
    }

    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }

    #[allow(clippy::collapsible_if)]
    if plan
        .tasks
        .iter()
        .any(|task| matches!(task.kind.as_deref(), Some("codex")) || task.codex.is_some())
    {
        if let Err(err) = check_codex_available() {
            eprintln!("environment error: {err}");
            return Ok(4);
        }
    }

    if !recovery.resume || !snapshot_path.exists() {
        let _ = write_plan_snapshot(&store_root, &run_id, &plan)?;
    }

    let store = Arc::new(FsStore::new(&store_root, &run_id)?);
    let cwd = resolve_run_cwd(&plan, plan_base_dir)?;
    let ctx = RunContext {
        cwd,
        env: plan.run.env.clone(),
        store: store.clone(),
    };

    let (mut state, initial_states) = if recovery.resume {
        let report = recover_running_tasks(store.as_ref())?;
        if !report.alive_tasks.is_empty() {
            return Err(anyhow!(
                "run {} still has running tasks: {}",
                run_id,
                report.alive_tasks.join(", ")
            ));
        }
        let mut state = report.state;
        let initial_states = build_initial_records(&state);
        if !has_pending_tasks(&initial_states) {
            let (run_status, exit_code) = finalize_run_status(&state);
            let now = Utc::now();
            state.status = run_status;
            state.completed_at = Some(now);
            store.write_state(state)?;
            remove_run_pid(&store_root, &run_id);
            return Ok(exit_code);
        }
        if state.status != RunStatus::Running || state.completed_at.is_some() {
            state.status = RunStatus::Running;
            state.completed_at = None;
            store.write_state(state.clone())?;
        }
        (state, Some(initial_states))
    } else {
        let now = Utc::now();
        let mut tasks_state = HashMap::new();
        for task in &plan.tasks {
            tasks_state.insert(
                task.id.clone(),
                TaskState {
                    status: TaskStatus::Pending,
                    exit_code: None,
                    stderr_tail: None,
                    started_at: None,
                    completed_at: None,
                    pid: None,
                },
            );
        }

        let state = State {
            run_id: run_id.clone(),
            run_name: plan
                .run
                .name
                .clone()
                .unwrap_or_else(|| run_id.clone()),
            status: RunStatus::Running,
            tasks: tasks_state,
            started_at: now,
            completed_at: None,
        };
        store.write_state(state.clone())?;
        store.append_event(Event::RunStarted {
            run_id: run_id.clone(),
            timestamp: now,
        })?;
        (state, None)
    };

    write_run_pid(&store_root, &run_id)?;

    let state_handle = StateHandle::new(store.clone(), state.clone());
    let cancel = CancelHandle::new();
    spawn_cancel_listener(cancel.clone());

    let tasks_map: HashMap<_, _> = plan
        .tasks
        .iter()
        .cloned()
        .map(|task| (task.id.clone(), task))
        .collect();
    let task_specs: Vec<TaskSpec> = plan
        .tasks
        .iter()
        .map(|task| TaskSpec {
            id: task.id.clone(),
            deps: task.deps.clone(),
            locks: task.locks.clone(),
        })
        .collect();

    let max_concurrency = plan
        .run
        .max_concurrency
        .or(global.max_concurrency)
        .unwrap_or(1);
    let fail_fast = plan.run.fail_fast.unwrap_or(global.effective_fail_fast());
    let default_timeout_sec = plan.run.default_timeout_sec;

    let runner = PlanTaskRunner::new(
        Arc::new(tasks_map),
        ctx,
        state_handle.clone(),
        cancel.clone(),
        default_timeout_sec,
    );
    let scheduler = if let Some(initial_states) = initial_states {
        Scheduler::new_with_initial_state(
            task_specs,
            SchedulerOptions {
                max_concurrency,
                fail_fast,
            },
            runner,
            initial_states,
        )
    } else {
        Scheduler::new(
            task_specs,
            SchedulerOptions {
                max_concurrency,
                fail_fast,
            },
            runner,
        )
    };

    let report = scheduler.run().await;
    reconcile_state(&state_handle, &report)?;

    state = state_handle.snapshot();
    let (run_status, exit_code) = finalize_run_status(&state);
    state_handle.update_run_status(run_status)?;
    remove_run_pid(&store_root, &run_id);

    Ok(exit_code)
}

async fn handle_start(
    global: &GlobalOptions,
    plan_arg: &str,
    recovery: RecoveryOptions,
    run_id: Option<String>,
) -> Result<i32> {
    if recovery.resume && run_id.is_none() {
        return Err(anyhow!("--resume requires --run-id"));
    }
    let (mut plan, base_dir) = load_plan(plan_arg)?;

    let store_root = resolve_store_path(global.store.as_ref())?;
    let run_id = run_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let snapshot_path = plan_snapshot_path(&store_root, &run_id);

    if recovery.resume {
        if !snapshot_path.exists() {
            return Err(anyhow!("run {} has no plan snapshot to resume", run_id));
        }
        plan = load_plan_snapshot(&store_root, &run_id)?;
        let store = FsStore::new(&store_root, &run_id)?;
        let report = recover_running_tasks(&store)?;
        if !report.alive_tasks.is_empty() {
            return Err(anyhow!(
                "run {} still has running tasks: {}",
                run_id,
                report.alive_tasks.join(", ")
            ));
        }
    }

    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }
    #[allow(clippy::collapsible_if)]
    if plan
        .tasks
        .iter()
        .any(|task| matches!(task.kind.as_deref(), Some("codex")) || task.codex.is_some())
    {
        if let Err(err) = check_codex_available() {
            eprintln!("environment error: {err}");
            return Ok(4);
        }
    }

    let plan_path = if recovery.resume {
        snapshot_path
    } else {
        write_plan_snapshot(&store_root, &run_id, &plan)?
    };

    let exe = env::current_exe().context("resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("run")
        .arg(plan_path)
        .arg("--run-id")
        .arg(&run_id)
        .arg("--base-dir")
        .arg(&base_dir);
    if recovery.resume {
        cmd.arg("--resume");
    }
    if recovery.clean_start {
        cmd.arg("--clean-start");
    }
    if let Some(store_path) = global.store.as_ref() {
        cmd.arg("--store").arg(store_path);
    }
    if let Some(max) = global.max_concurrency {
        cmd.arg("--max-concurrency").arg(max.to_string());
    }
    if !global.effective_fail_fast() {
        cmd.arg("--no-fail-fast");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn().context("spawn background run")?;

    println!("{run_id}");
    Ok(0)
}

fn handle_status(global: &GlobalOptions, run_id: Option<String>, json: bool) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    if let Some(run_id) = run_id {
        let state = read_state(&store_root, &run_id)?;
        if json {
            let text = serde_json::to_string_pretty(&state)?;
            println!("{text}");
        } else {
            print_state(&state);
        }
        return Ok(0);
    }

    let mut states = list_states(&store_root)?;
    states.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    if json {
        let text = serde_json::to_string_pretty(&states)?;
        println!("{text}");
    } else {
        print_states_table(&states);
    }
    Ok(0)
}

fn handle_tui(global: &GlobalOptions, run_id: Option<String>) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;

    // run_idが指定されていない場合、一覧を表示
    if run_id.is_none() {
        let mut states = list_states(&store_root)?;
        if states.is_empty() {
            println!("no runs found");
            return Ok(0);
        }
        states.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        println!("Available runs:");
        println!();
        print_states_table(&states);
        println!();
        println!("Usage: quedex tui <run_id>");
        return Ok(0);
    }

    tui::run(store_root, run_id)
}

fn handle_logs(
    global: &GlobalOptions,
    run_id: &str,
    task_id: &str,
    follow: bool,
    stderr: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    let path = log_path(&store_root, run_id, task_id, stderr);
    if !path.exists() {
        return Err(anyhow!("log file not found: {}", path.display()));
    }
    stream_log(&path, follow)?;
    Ok(0)
}

async fn handle_retry(global: &GlobalOptions, run_id: &str, task_id: &str) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    let plan = load_plan_snapshot(&store_root, run_id)?;
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| anyhow!("task {} not found in run {}", task_id, run_id))?;

    if matches!(task.kind.as_deref(), Some("codex")) || task.codex.is_some() {
        check_codex_available()?;
    }

    let mut state = read_state(&store_root, run_id)?;
    if state.status == RunStatus::Running {
        return Err(anyhow!("run {} is still running", run_id));
    }
    let Some(task_state) = state.tasks.get(task_id) else {
        return Err(anyhow!("task {} not found in state for run {}", task_id, run_id));
    };
    if !matches!(task_state.status, TaskStatus::Failed | TaskStatus::Canceled) {
        return Err(anyhow!(
            "task {} must be Failed or Canceled to retry (current: {:?})",
            task_id,
            task_state.status
        ));
    }
    for dep in &task.deps {
        let Some(dep_state) = state.tasks.get(dep) else {
            return Err(anyhow!("task {} dependency {} not found in state", task_id, dep));
        };
        if dep_state.status != TaskStatus::Succeeded {
            return Err(anyhow!(
                "task {} dependency {} not satisfied (current: {:?})",
                task_id,
                dep,
                dep_state.status
            ));
        }
    }

    let Some(task_state) = state.tasks.get_mut(task_id) else {
        return Err(anyhow!("task {} not found in state for run {}", task_id, run_id));
    };
    task_state.status = TaskStatus::Pending;
    task_state.exit_code = None;
    task_state.stderr_tail = None;
    task_state.started_at = None;
    task_state.completed_at = None;
    task_state.pid = None;
    state.status = RunStatus::Running;
    state.completed_at = None;

    let store = Arc::new(FsStore::new(&store_root, run_id)?);
    store.write_state(state.clone())?;
    write_run_pid(&store_root, run_id)?;

    let base_dir = env::current_dir().context("resolve current dir for retry")?;
    let cwd = resolve_run_cwd(&plan, base_dir)?;
    let ctx = RunContext {
        cwd,
        env: plan.run.env.clone(),
        store: store.clone(),
    };

    let state_handle = StateHandle::new(store.clone(), state);
    let cancel = CancelHandle::new();
    spawn_cancel_listener(cancel.clone());

    let mut tasks_map = HashMap::new();
    tasks_map.insert(task.id.clone(), task.clone());
    let task_specs = vec![TaskSpec {
        id: task.id.clone(),
        deps: Vec::new(),
        locks: task.locks.clone(),
    }];

    let max_concurrency = plan
        .run
        .max_concurrency
        .or(global.max_concurrency)
        .unwrap_or(1);
    let fail_fast = plan.run.fail_fast.unwrap_or(global.effective_fail_fast());
    let default_timeout_sec = plan.run.default_timeout_sec;

    let runner = PlanTaskRunner::new(
        Arc::new(tasks_map),
        ctx,
        state_handle.clone(),
        cancel,
        default_timeout_sec,
    );
    let scheduler = Scheduler::new(
        task_specs,
        SchedulerOptions {
            max_concurrency,
            fail_fast,
        },
        runner,
    );

    let report = scheduler.run().await;
    reconcile_state(&state_handle, &report)?;

    let state = state_handle.snapshot();
    let (run_status, exit_code) = finalize_run_status(&state);
    state_handle.update_run_status(run_status)?;
    remove_run_pid(&store_root, run_id);
    Ok(exit_code)
}

fn handle_cancel(global: &GlobalOptions, run_id: &str, task_id: Option<String>) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    if let Some(task_id) = task_id {
        let state = read_state(&store_root, run_id)?;
        if let Some(task_state) = state.tasks.get(&task_id) {
            if let Some(pid) = task_state.pid {
                terminate_pid(pid)?;
                return Ok(0);
            }
            return Err(anyhow!("task {} has no pid to cancel", task_id));
        }
        return Err(anyhow!("task {} not found in run {}", task_id, run_id));
    }

    let pid_path = run_pid_path(&store_root, run_id);
    if pid_path.exists() {
        let pid = fs::read_to_string(&pid_path)
            .context("read run pid")?
            .trim()
            .parse::<u32>()
            .context("parse run pid")?;
        terminate_pid(pid)?;
        return Ok(0);
    }

    let state = read_state(&store_root, run_id)?;
    for task_state in state.tasks.values() {
        #[allow(clippy::collapsible_if)]
        if let Some(pid) = task_state.pid {
            if let Err(err) = terminate_pid(pid) {
                eprintln!("Warning: failed to terminate pid {}: {}", pid, err);
            }
        }
    }
    Ok(0)
}

enum CleanResult {
    Removed,
    SkippedRunning { reason: String },
}

fn handle_clean(global: &GlobalOptions, run_id: Option<String>, all: bool) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    if let Some(run_id) = run_id {
        if all {
            return Err(anyhow!("--all cannot be used with run_id"));
        }
        clean_run_dir(&store_root, &run_id, true)?;
        return Ok(0);
    }

    if !all {
        return Err(anyhow!("clean requires run_id or --all"));
    }

    let run_ids = list_run_ids(&store_root)?;
    let mut skipped = Vec::new();
    for run_id in run_ids {
        match clean_run_dir(&store_root, &run_id, false)? {
            CleanResult::Removed => {}
            CleanResult::SkippedRunning { reason } => {
                skipped.push(format!("{run_id} ({reason})"));
            }
        }
    }
    if !skipped.is_empty() {
        eprintln!("Warning: skipped running runs: {}", skipped.join(", "));
    }
    Ok(0)
}

fn clean_run_dir(store_root: &Path, run_id: &str, strict: bool) -> Result<CleanResult> {
    let run_dir = run_dir(store_root, run_id);
    if !run_dir.exists() {
        return Err(anyhow!("run {} not found", run_id));
    }
    if let Some(reason) = run_active_reason(store_root, run_id)? {
        if strict {
            return Err(anyhow!("run {} is still running ({})", run_id, reason));
        }
        return Ok(CleanResult::SkippedRunning { reason });
    }

    fs::remove_dir_all(&run_dir).with_context(|| format!("remove {}", run_dir.display()))?;
    Ok(CleanResult::Removed)
}

fn run_active_reason(store_root: &Path, run_id: &str) -> Result<Option<String>> {
    let pid_path = run_pid_path(store_root, run_id);
    if pid_path.exists() {
        let pid = read_pid(&pid_path)?;
        match pid_is_alive(pid) {
            Ok(true) => return Ok(Some(format!("pid {pid}"))),
            Ok(false) => {}
            Err(err) => {
                return Err(anyhow!(
                    "cannot verify run {} pid {}: {err}",
                    run_id,
                    pid
                ));
            }
        }
    }

    let state_path = run_dir(store_root, run_id).join("state.json");
    if state_path.exists() {
        let state = read_state(store_root, run_id)?;
        if state.status == RunStatus::Running {
            let mut alive_tasks = Vec::new();
            for (task_id, task_state) in &state.tasks {
                if task_state.status != TaskStatus::Running {
                    continue;
                }
                let Some(pid) = task_state.pid else {
                    continue;
                };
                match pid_is_alive(pid) {
                    Ok(true) => alive_tasks.push(format!("{task_id}(pid {pid})")),
                    Ok(false) => {}
                    Err(err) => {
                        return Err(anyhow!(
                            "cannot verify task {} pid {}: {err}",
                            task_id,
                            pid
                        ));
                    }
                }
            }
            if !alive_tasks.is_empty() {
                return Ok(Some(format!(
                    "running tasks: {}",
                    alive_tasks.join(", ")
                )));
            }
        }
    }

    Ok(None)
}

fn handle_graph(
    global: &GlobalOptions,
    target: &str,
    mermaid: bool,
    ascii: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(global.store.as_ref())?;
    let plan = if Path::new(target).exists() {
        load_plan(target)?.0
    } else {
        let plan_path = plan_snapshot_path(&store_root, target);
        let contents = fs::read_to_string(&plan_path).with_context(|| {
            format!(
                "read plan snapshot for run {} at {}",
                target,
                plan_path.display()
            )
        })?;
        Plan::parse_str(&contents, PlanFormat::Json)?
    };

    let output_mermaid = mermaid && !ascii;
    if output_mermaid {
        print_mermaid_graph(&plan);
    } else {
        print_ascii_graph(&plan);
    }
    Ok(0)
}

fn resolve_store_path(store: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(store) = store {
        return Ok(store.clone());
    }
    let local = PathBuf::from(".quedex");
    if local.exists() {
        return Ok(local);
    }
    let home = env::var("HOME").context("read HOME for store path")?;
    Ok(PathBuf::from(home).join(".quedex"))
}

fn load_plan(plan_arg: &str) -> Result<(Plan, PathBuf)> {
    if plan_arg == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .context("read plan from stdin")?;
        let plan = parse_plan_with_fallback(&buf, None)?;
        let cwd = env::current_dir().context("resolve current dir")?;
        return Ok((plan, cwd));
    }

    let path = PathBuf::from(plan_arg);
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("read plan file {}", path.display()))?;
    let format = plan_format_from_path(&path);
    let plan = parse_plan_with_fallback(&contents, format)?;
    let abs_path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .context("resolve current dir")?
            .join(&path)
    };
    let base_dir = abs_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok((plan, base_dir))
}

fn parse_plan_with_fallback(input: &str, format: Option<PlanFormat>) -> Result<Plan> {
    if let Some(format) = format {
        return Plan::parse_str(input, format);
    }
    if let Ok(plan) = Plan::parse_str(input, PlanFormat::Json) {
        return Ok(plan);
    }
    Plan::parse_str(input, PlanFormat::Yaml)
}

fn plan_format_from_path(path: &Path) -> Option<PlanFormat> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => Some(PlanFormat::Json),
        Some("yaml") | Some("yml") => Some(PlanFormat::Yaml),
        _ => None,
    }
}

fn resolve_run_cwd(plan: &Plan, base_dir: PathBuf) -> Result<PathBuf> {
    let cwd = if let Some(cwd) = plan.run.cwd.as_ref() {
        if cwd.is_relative() {
            base_dir.join(cwd)
        } else {
            cwd.clone()
        }
    } else {
        base_dir
    };
    Ok(cwd)
}

fn check_codex_available() -> Result<()> {
    let status = std::process::Command::new("codex")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(_) => Ok(()),
        Err(err) => Err(anyhow!(err).context("codex not found")),
    }
}

fn write_plan_snapshot(store_root: &Path, run_id: &str, plan: &Plan) -> Result<PathBuf> {
    let run_dir = run_dir(store_root, run_id);
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create run dir {}", run_dir.display()))?;
    let path = run_dir.join("plan.json");
    let file = File::create(&path).with_context(|| format!("open {}", path.display()))?;
    serde_json::to_writer_pretty(file, plan).context("write plan snapshot")?;
    Ok(path)
}

fn read_state(store_root: &Path, run_id: &str) -> Result<State> {
    let path = run_dir(store_root, run_id).join("state.json");
    let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let state = serde_json::from_reader(file).context("deserialize state")?;
    Ok(state)
}

fn list_states(store_root: &Path) -> Result<Vec<State>> {
    let runs_dir = store_root.join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut states = Vec::new();
    for entry in fs::read_dir(&runs_dir).context("read runs directory")? {
        let entry = entry.context("read runs entry")?;
        if !entry.file_type().context("read run entry type")?.is_dir() {
            continue;
        }
        let run_id = entry.file_name().to_string_lossy().to_string();
        match read_state(store_root, &run_id) {
            Ok(state) => states.push(state),
            Err(err) => eprintln!("skip run {}: {err}", run_id),
        }
    }
    Ok(states)
}

fn list_run_ids(store_root: &Path) -> Result<Vec<String>> {
    let runs_dir = store_root.join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut run_ids = Vec::new();
    for entry in fs::read_dir(&runs_dir).context("read runs directory")? {
        let entry = entry.context("read runs entry")?;
        if !entry.file_type().context("read run entry type")?.is_dir() {
            continue;
        }
        let run_id = entry.file_name().to_string_lossy().to_string();
        run_ids.push(run_id);
    }
    Ok(run_ids)
}

fn print_states_table(states: &[State]) {
    println!("{:<36} {:<10} {:<20} name", "run_id", "status", "started_at");
    for state in states {
        println!(
            "{:<36} {:<10} {:<20} {}",
            state.run_id,
            format!("{:?}", state.status),
            state.started_at,
            state.run_name
        );
    }
}

fn print_state(state: &State) {
    println!("run_id: {}", state.run_id);
    println!("name: {}", state.run_name);
    println!("status: {:?}", state.status);
    println!("started_at: {}", state.started_at);
    if let Some(completed_at) = state.completed_at {
        println!("completed_at: {}", completed_at);
    }
    println!();
    println!("{:<16} {:<10} {:<6} started", "task", "status", "exit");
    for (task_id, task_state) in &state.tasks {
        println!(
            "{:<16} {:<10} {:<6} {}",
            task_id,
            format!("{:?}", task_state.status),
            task_state
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            task_state
                .started_at
                .map(|ts| ts.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
}

fn log_path(store_root: &Path, run_id: &str, task_id: &str, stderr: bool) -> PathBuf {
    let stream = if stderr {
        LogStream::Stderr
    } else {
        LogStream::Stdout
    };
    run_dir(store_root, run_id)
        .join("tasks")
        .join(task_id)
        .join(stream.file_name())
}

fn stream_log(path: &Path, follow: bool) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut stdout = io::stdout();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .with_context(|| format!("read {}", path.display()))?;
    stdout
        .write_all(&buffer)
        .context("write log output")?;
    stdout.flush().context("flush log output")?;
    if !follow {
        return Ok(());
    }
    loop {
        let mut chunk = [0u8; 8192];
        let n = file.read(&mut chunk)?;
        if n > 0 {
            stdout.write_all(&chunk[..n])?;
            stdout.flush()?;
        } else {
            let _ = file.stream_position()?;
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn terminate_pid(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        // First try SIGTERM for graceful shutdown
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .context("spawn kill command")?;

        if !status.success() {
            eprintln!("Warning: SIGTERM failed for pid {}, trying SIGKILL", pid);
        }
        
        // Give the process time to gracefully shut down before escalating to SIGKILL
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        // Check if process still exists and escalate to SIGKILL if needed
        let check_status = std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .context("check if process exists")?;
        
        if check_status.success() {
            // Process still exists, escalate to SIGKILL
            eprintln!("Warning: pid {} still running after SIGTERM, escalating to SIGKILL", pid);
            let kill_status = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .status()
                .context("spawn kill -KILL command")?;

            if !kill_status.success() {
                return Err(anyhow!("failed to kill pid {} even with SIGKILL", pid));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(anyhow!("cancel not supported on this platform"))
    }
}

fn read_pid(path: &Path) -> Result<u32> {
    let pid = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?
        .trim()
        .parse::<u32>()
        .context("parse pid")?;
    Ok(pid)
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> Result<bool> {
    let status = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .context("spawn kill -0 for pid check")?;
    Ok(status.success())
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> Result<bool> {
    Err(anyhow!("pid checks are not supported on this platform"))
}

fn run_dir(store_root: &Path, run_id: &str) -> PathBuf {
    store_root.join("runs").join(run_id)
}

fn run_pid_path(store_root: &Path, run_id: &str) -> PathBuf {
    run_dir(store_root, run_id).join("run.pid")
}

fn write_run_pid(store_root: &Path, run_id: &str) -> Result<()> {
    let path = run_pid_path(store_root, run_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = File::create(&path).with_context(|| format!("open {}", path.display()))?;
    writeln!(file, "{}", std::process::id()).context("write run pid")?;
    Ok(())
}

fn remove_run_pid(store_root: &Path, run_id: &str) {
    let path = run_pid_path(store_root, run_id);
    let _ = fs::remove_file(path);
}

fn plan_snapshot_path(store_root: &Path, run_id: &str) -> PathBuf {
    run_dir(store_root, run_id).join("plan.json")
}

fn load_plan_snapshot(store_root: &Path, run_id: &str) -> Result<Plan> {
    let plan_path = plan_snapshot_path(store_root, run_id);
    let contents = fs::read_to_string(&plan_path)
        .with_context(|| format!("read plan snapshot {}", plan_path.display()))?;
    Plan::parse_str(&contents, PlanFormat::Json)
}

fn finalize_run_status(state: &State) -> (RunStatus, i32) {
    let mut has_failed = false;
    let mut has_skipped = false;
    let mut has_canceled = false;

    for task in state.tasks.values() {
        match task.status {
            TaskStatus::Failed => has_failed = true,
            TaskStatus::Skipped => has_skipped = true,
            TaskStatus::Canceled => has_canceled = true,
            _ => {}
        }
    }

    if has_failed || has_skipped {
        (RunStatus::Failed, 1)
    } else if has_canceled {
        (RunStatus::Canceled, 2)
    } else {
        (RunStatus::Completed, 0)
    }
}

fn print_mermaid_graph(plan: &Plan) {
    println!("graph TD");
    for task in &plan.tasks {
        if task.deps.is_empty() {
            println!("  {};", task.id);
        }
        for dep in &task.deps {
            println!("  {} --> {};", dep, task.id);
        }
    }
}

fn print_ascii_graph(plan: &Plan) {
    for task in &plan.tasks {
        if task.deps.is_empty() {
            println!("{}", task.id);
        }
        for dep in &task.deps {
            println!("{} -> {}", dep, task.id);
        }
    }
}

fn spawn_cancel_listener(cancel: CancelHandle) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        tokio::spawn(async move {
            let mut term = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
            let mut interrupt = match signal(SignalKind::interrupt()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
            tokio::select! {
                _ = term.recv() => {},
                _ = interrupt.recv() => {},
            }
            cancel.trigger();
        });
    }
}

fn reconcile_state(state_handle: &StateHandle, report: &ScheduleReport) -> Result<()> {
    let now = Utc::now();
    state_handle.update(|state| {
        for (task_id, record) in &report.tasks {
            #[allow(clippy::collapsible_if)]
            if let Some(task_state) = state.tasks.get_mut(task_id) {
                if task_state.status != record.status {
                    task_state.status = record.status;
                    task_state.exit_code = record.exit_code;
                    if matches!(
                        record.status,
                        TaskStatus::Succeeded
                            | TaskStatus::Failed
                            | TaskStatus::Canceled
                            | TaskStatus::Skipped
                    ) {
                        task_state.completed_at = Some(now);
                    }
                }
            }
        }
    })?;
    Ok(())
}

fn build_initial_records(state: &State) -> HashMap<String, TaskRecord> {
    state
        .tasks
        .iter()
        .map(|(task_id, task_state)| {
            (
                task_id.clone(),
                TaskRecord {
                    status: task_state.status,
                    exit_code: task_state.exit_code,
                },
            )
        })
        .collect()
}

fn has_pending_tasks(records: &HashMap<String, TaskRecord>) -> bool {
    records.values().any(|record| {
        matches!(
            record.status,
            TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Running
        )
    })
}

/// Handle for managing shared state across tasks.
///
/// Provides thread-safe access to the run state and automatically
/// persists changes to the store. All state modifications should go
/// through this handle to ensure consistency.
#[derive(Clone)]
struct StateHandle {
    store: Arc<dyn Store>,
    state: Arc<Mutex<State>>,
}

impl StateHandle {
    fn new(store: Arc<dyn Store>, state: State) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn snapshot(&self) -> State {
        match self.state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                eprintln!("Error: state lock poisoned - returning potentially corrupted state (data may be inconsistent)");
                poisoned.into_inner().clone()
            }
        }
    }

    fn update<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State),
    {
        let snapshot = {
            let mut state = match self.state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    eprintln!("Error: state lock poisoned - updating potentially corrupted state (data may be inconsistent)");
                    poisoned.into_inner()
                }
            };
            update_fn(&mut state);
            state.clone()
        };
        self.store.write_state(snapshot)?;
        Ok(())
    }

    fn task_started(&self, task_id: &str, pid: u32) -> Result<()> {
        let now = Utc::now();
        self.update(|state| {
            if let Some(task) = state.tasks.get_mut(task_id) {
                task.status = TaskStatus::Running;
                task.pid = Some(pid);
                task.started_at = Some(now);
                task.completed_at = None;
                task.exit_code = None;
            }
        })?;
        self.store.append_event(Event::TaskStarted {
            task_id: task_id.to_string(),
            pid,
            timestamp: now,
        })?;
        Ok(())
    }

    fn task_finished(&self, task_id: &str, status: TaskStatus, exit_code: Option<i32>) -> Result<()> {
        let now = Utc::now();
        self.update(|state| {
            if let Some(task) = state.tasks.get_mut(task_id) {
                task.status = status;
                task.exit_code = exit_code;
                task.completed_at = Some(now);
                task.pid = None;
            }
        })?;
        match status {
            TaskStatus::Canceled => {
                self.store.append_event(Event::TaskCanceled {
                    task_id: task_id.to_string(),
                    timestamp: now,
                })?;
            }
            TaskStatus::Succeeded | TaskStatus::Failed => {
                let code = exit_code.unwrap_or(-1);
                self.store.append_event(Event::TaskExited {
                    task_id: task_id.to_string(),
                    exit_code: code,
                    timestamp: now,
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn update_run_status(&self, status: RunStatus) -> Result<()> {
        let now = Utc::now();
        self.update(|state| {
            state.status = status;
            state.completed_at = Some(now);
        })
    }
}

/// Handle for coordinating task cancellation across the system.
///
/// Tracks running tasks and provides a mechanism to cancel them all
/// when a termination signal is received (SIGTERM/SIGINT) or when
/// a timeout occurs. Tasks register their child handles here so they
/// can be killed on cancellation.
#[derive(Clone)]
struct CancelHandle {
    canceled: Arc<AtomicBool>,
    active: Arc<Mutex<HashMap<String, ChildHandle>>>,
}

impl CancelHandle {
    fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn trigger(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        let children = match self.active.lock() {
            Ok(guard) => guard,
            Err(_) => {
                eprintln!("Error: active lock poisoned - cannot safely kill child processes (some processes may not be terminated)");
                return;
            }
        };
        for child in children.values() {
            let _ = child.kill();
        }
    }

    fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    fn register(&self, task_id: &str, child: ChildHandle) {
        match self.active.lock() {
            Ok(mut guard) => {
                guard.insert(task_id.to_string(), child);
            }
            Err(_) => {
                eprintln!("Error: active lock poisoned - cannot register task {} (cancellation may not work correctly)", task_id);
            }
        }
    }

    fn unregister(&self, task_id: &str) {
        match self.active.lock() {
            Ok(mut guard) => {
                guard.remove(task_id);
            }
            Err(_) => {
                eprintln!("Error: active lock poisoned - cannot unregister task {} (may cause issues with future cancellations)", task_id);
            }
        }
    }
}

/// Task runner implementation for executing plan tasks.
///
/// Executes tasks using either the Codex runner (for LLM-assisted tasks)
/// or the Shell runner (for command execution). Handles task lifecycle
/// including spawn, execution, timeout enforcement, and state updates.
struct PlanTaskRunner {
    tasks: Arc<HashMap<String, Task>>,
    ctx: RunContext,
    state: StateHandle,
    cancel: CancelHandle,
    codex: CodexRunner,
    shell: ShellRunner,
    default_timeout_sec: Option<u64>,
}

impl PlanTaskRunner {
    fn new(
        tasks: Arc<HashMap<String, Task>>,
        ctx: RunContext,
        state: StateHandle,
        cancel: CancelHandle,
        default_timeout_sec: Option<u64>,
    ) -> Self {
        Self {
            tasks,
            ctx,
            state,
            cancel,
            codex: CodexRunner::new(),
            shell: ShellRunner::new(),
            default_timeout_sec,
        }
    }
}

impl TaskRunner for PlanTaskRunner {
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = TaskResult> + Send>>;

    fn spawn(&self, task_spec: TaskSpec) -> Self::Future {
        let tasks = Arc::clone(&self.tasks);
        let ctx = self.ctx.clone();
        let state = self.state.clone();
        let cancel = self.cancel.clone();
        let codex = self.codex;
        let shell = self.shell;
        let default_timeout_sec = self.default_timeout_sec;

        Box::pin(async move {
            let Some(task) = tasks.get(&task_spec.id) else {
                return TaskResult::failed(1);
            };

            if cancel.is_canceled() {
                let _ = state.task_finished(&task.id, TaskStatus::Canceled, None);
                return TaskResult::canceled();
            }

            let runner: &dyn Runner = if task.codex.is_some() {
                &codex
            } else {
                &shell
            };

            let child = match runner.spawn(task, &ctx) {
                Ok(child) => child,
                Err(err) => {
                    eprintln!("task {} spawn error: {err:#}", task.id);
                    let _ = state.task_finished(&task.id, TaskStatus::Failed, Some(1));
                    return TaskResult::failed(1);
                }
            };

            cancel.register(&task.id, child.clone());
            let _ = state.task_started(&task.id, child.pid);

            let task_id = task.id.clone();
            let timeout_sec = task.timeout_sec.or(default_timeout_sec);

            let wait_future = tokio::task::spawn_blocking(move || child.wait());
            let wait_result = if let Some(timeout_secs) = timeout_sec {
                let timeout_duration = Duration::from_secs(timeout_secs);
                match tokio::time::timeout(timeout_duration, wait_future).await {
                    Ok(result) => result,
                    Err(_) => {
                        // Write timeout message to task's stderr log file
                        let timeout_msg = format!("task {} timed out after {} seconds\n", task_id, timeout_secs);
                        if let Ok(mut stderr_log) = ctx.store.open_log(&task_id, LogStream::Stderr) {
                            let _ = stderr_log.write_all(timeout_msg.as_bytes());
                        }
                        eprintln!("{}", timeout_msg.trim());
                        
                        // Kill the process on timeout
                        #[allow(clippy::collapsible_if)]
                        if let Ok(active) = cancel.active.lock() {
                            if let Some(child_handle) = active.get(&task_id) {
                                let _ = child_handle.kill();
                            }
                        }
                        cancel.unregister(&task_id);
                        let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(124));
                        return TaskResult::failed(124);
                    }
                }
            } else {
                wait_future.await
            };

            cancel.unregister(&task_id);

            let status = match wait_result {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => {
                    eprintln!("task {} wait error: {err}", task_id);
                    let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                    return TaskResult::failed(1);
                }
                Err(err) => {
                    eprintln!("task {} join error: {err}", task_id);
                    let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                    return TaskResult::failed(1);
                }
            };

            let result = map_exit_status(status, cancel.is_canceled());
            let _ = state.task_finished(&task_id, result.status, result.exit_code);
            result
        })
    }
}

fn map_exit_status(status: std::process::ExitStatus, canceled: bool) -> TaskResult {
    if canceled {
        return TaskResult::canceled();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        #[allow(clippy::collapsible_if)]
        if let Some(signal) = status.signal() {
            if signal == 2 || signal == 15 {
                return TaskResult::canceled();
            }
        }
    }
    if status.success() {
        TaskResult::succeeded()
    } else {
        let code = status.code().unwrap_or(-1);
        TaskResult::failed(code)
    }
}
