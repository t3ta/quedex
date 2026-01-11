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

use cli::{Cli, Commands, GlobalOptions};
use quedex::plan::{Plan, PlanFormat, Task};
use quedex::runner::codex::CodexRunner;
use quedex::runner::shell::ShellRunner;
use quedex::runner::{ChildHandle, RunContext, Runner};
use quedex::scheduler::{ScheduleReport, Scheduler, SchedulerOptions, TaskResult, TaskRunner, TaskSpec};
use quedex::store::fs::FsStore;
use quedex::store::{Event, LogStream, RunStatus, State, TaskState, TaskStatus, Store};

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
            run_id,
            base_dir,
        } => handle_run(&cli.global, &plan, run_id, base_dir).await,
        Commands::Start { plan } => handle_start(&cli.global, &plan).await,
        Commands::Status { run_id, json } => handle_status(&cli.global, run_id, json),
        Commands::Logs {
            run_id,
            task_id,
            follow,
            stderr,
        } => handle_logs(&cli.global, &run_id, &task_id, follow, stderr),
        Commands::Cancel { run_id, task_id } => handle_cancel(&cli.global, &run_id, task_id),
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
    run_id: Option<String>,
    base_dir: Option<PathBuf>,
) -> Result<i32> {
    let (plan, mut plan_base_dir) = load_plan(plan_arg)?;
    if let Some(base_dir) = base_dir {
        plan_base_dir = base_dir;
    }
    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }

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

    let store_root = resolve_store_path(global.store.as_ref())?;
    let run_id = run_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let _ = write_plan_snapshot(&store_root, &run_id, &plan)?;

    let store = Arc::new(FsStore::new(&store_root, &run_id)?);
    write_run_pid(&store_root, &run_id)?;

    let cwd = resolve_run_cwd(&plan, plan_base_dir)?;
    let ctx = RunContext {
        cwd,
        env: plan.run.env.clone(),
        store: store.clone(),
    };

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

    let mut state = State {
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

    let runner = PlanTaskRunner::new(
        Arc::new(tasks_map),
        ctx,
        state_handle.clone(),
        cancel.clone(),
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

    state = state_handle.snapshot();
    let (run_status, exit_code) = finalize_run_status(&state);
    state_handle.update_run_status(run_status)?;
    remove_run_pid(&store_root, &run_id);

    Ok(exit_code)
}

async fn handle_start(global: &GlobalOptions, plan_arg: &str) -> Result<i32> {
    let (plan, base_dir) = load_plan(plan_arg)?;
    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }
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

    let store_root = resolve_store_path(global.store.as_ref())?;
    let run_id = Uuid::new_v4().to_string();
    let plan_path = write_plan_snapshot(&store_root, &run_id, &plan)?;

    let exe = env::current_exe().context("resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("run")
        .arg(plan_path)
        .arg("--run-id")
        .arg(&run_id)
        .arg("--base-dir")
        .arg(&base_dir);
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
        if let Some(pid) = task_state.pid {
            let _ = terminate_pid(pid);
        }
    }
    Ok(0)
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
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .context("spawn kill command")?;
        if !status.success() {
            return Err(anyhow!("failed to terminate pid {}", pid));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(anyhow!("cancel not supported on this platform"))
    }
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
        self.state
            .lock()
            .expect("state lock poisoned")
            .clone()
    }

    fn update<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State),
    {
        let snapshot = {
            let mut state = self.state.lock().expect("state lock poisoned");
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
        let children = self.active.lock().expect("active lock poisoned");
        for child in children.values() {
            let _ = child.kill();
        }
    }

    fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    fn register(&self, task_id: &str, child: ChildHandle) {
        self.active
            .lock()
            .expect("active lock poisoned")
            .insert(task_id.to_string(), child);
    }

    fn unregister(&self, task_id: &str) {
        self.active
            .lock()
            .expect("active lock poisoned")
            .remove(task_id);
    }
}

struct PlanTaskRunner {
    tasks: Arc<HashMap<String, Task>>,
    ctx: RunContext,
    state: StateHandle,
    cancel: CancelHandle,
    codex: CodexRunner,
    shell: ShellRunner,
}

impl PlanTaskRunner {
    fn new(
        tasks: Arc<HashMap<String, Task>>,
        ctx: RunContext,
        state: StateHandle,
        cancel: CancelHandle,
    ) -> Self {
        Self {
            tasks,
            ctx,
            state,
            cancel,
            codex: CodexRunner::new(),
            shell: ShellRunner::new(),
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
            let wait_result = tokio::task::spawn_blocking(move || child.wait()).await;
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
