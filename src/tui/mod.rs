mod app;
mod input;
mod ui;

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;

use crate::plan::{Plan, PlanFormat};
use crate::store::State;

use app::App;
use input::{handle_key, Action};

pub fn run(store_root: PathBuf, run_id: Option<String>) -> Result<i32> {
    if headless_enabled() {
        return run_headless(store_root, run_id);
    }
    let run_id = resolve_run_id(&store_root, run_id)?;
    let plan = load_plan_snapshot(&store_root, &run_id)?;
    let mut app = App::new(store_root, run_id, plan)?;

    let mut terminal = init_terminal()?;
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result?;
    Ok(0)
}

pub fn run_headless(store_root: PathBuf, run_id: Option<String>) -> Result<i32> {
    let run_id = resolve_run_id(&store_root, run_id)?;
    let plan = load_plan_snapshot(&store_root, &run_id)?;
    let mut app = App::new(store_root, run_id, plan)?;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).context("init headless terminal")?;
    terminal.draw(|frame| ui::draw(frame, &mut app))?;
    Ok(0)
}

fn headless_enabled() -> bool {
    env::var_os("QUEDX_TUI_HEADLESS").is_some()
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    let watcher = match WatcherHandle::new(app.state_path(), app.tasks_dir()) {
        Ok(watcher) => Some(watcher),
        Err(err) => {
            app.set_status(format!("watch disabled: {err:#}"));
            None
        }
    };

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        #[allow(clippy::collapsible_if)]
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if let Some(action) = handle_key(key) {
                    apply_action(app, action)?;
                }
            }
        }

        let mut dirty = false;
        #[allow(clippy::collapsible_if)]
        if let Some(handle) = watcher.as_ref() {
            if handle.drain(app) {
                dirty = true;
            }
        }

        if last_tick.elapsed() >= tick_rate {
            dirty = true;
            last_tick = Instant::now();
        }

        if dirty {
            app.refresh_state()?;
            app.refresh_logs()?;
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn apply_action(app: &mut App, action: Action) -> Result<()> {
    let mut refresh_logs = false;
    match action {
        Action::Quit => app.should_quit = true,
        Action::Up => {
            if app.log_focus {
                app.scroll_log_up();
            } else if app.select_prev() {
                refresh_logs = true;
            }
        }
        Action::Down => {
            if app.log_focus {
                app.scroll_log_down();
            } else if app.select_next() {
                refresh_logs = true;
            }
        }
        Action::ToggleLogFocus => app.toggle_log_focus(),
        Action::ToggleStream => {
            app.toggle_log_stream();
            refresh_logs = true;
        }
        Action::Retry => {
            if let Err(err) = retry_task(app) {
                app.set_status(format!("retry error: {err:#}"));
            }
        }
        Action::CancelTask => {
            if let Err(err) = cancel_task(app) {
                app.set_status(format!("cancel task error: {err:#}"));
            }
        }
        Action::CancelRun => {
            if let Err(err) = cancel_run(app) {
                app.set_status(format!("cancel run error: {err:#}"));
            }
        }
        Action::ToggleGraph => app.graph_mode = !app.graph_mode,
        Action::ToggleGroupCollapse => app.toggle_selected_group_collapse(),
    }

    if refresh_logs {
        app.sync_log_path();
        app.refresh_logs()?;
    }
    Ok(())
}

fn retry_task(app: &mut App) -> Result<()> {
    let Some(task_id) = app.selected_task_id() else {
        return Err(anyhow!("no task selected"));
    };
    let Some(task_state) = app.state.tasks.get(task_id) else {
        return Err(anyhow!("task {task_id} not found in state"));
    };

    use crate::store::TaskStatus;
    if !matches!(task_state.status, TaskStatus::Failed | TaskStatus::Canceled) {
        return Err(anyhow!(
            "task {task_id} must be Failed or Canceled to retry (current: {:?})",
            task_state.status
        ));
    }

    // quedex retryコマンドをバックグラウンドで実行
    let run_id = app.run_id.clone();
    let task_id_str = task_id.to_string();
    let task_id_display = task_id_str.clone();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("quedex")
            .arg("retry")
            .arg(&run_id)
            .arg(&task_id_str)
            .output();
    });

    app.set_status(format!("retrying task {task_id_display}..."));
    Ok(())
}

fn cancel_task(app: &mut App) -> Result<()> {
    let Some(task_id) = app.selected_task_id() else {
        return Err(anyhow!("no task selected"));
    };
    let Some(state) = app.state.tasks.get(task_id) else {
        return Err(anyhow!("task {task_id} not found in state"));
    };
    let Some(pid) = state.pid else {
        return Err(anyhow!("task {task_id} has no pid to cancel"));
    };
    terminate_pid(pid)?;
    app.set_status(format!("sent cancel to task {task_id}"));
    Ok(())
}

fn cancel_run(app: &mut App) -> Result<()> {
    let pid_path = run_pid_path(&app.store_root, &app.run_id);
    if pid_path.exists() {
        let pid = fs::read_to_string(&pid_path)
            .context("read run pid")?
            .trim()
            .parse::<u32>()
            .context("parse run pid")?;
        terminate_pid(pid)?;
        app.set_status("sent cancel to run".to_string());
        return Ok(());
    }

    let mut canceled = false;
    for task_state in app.state.tasks.values() {
        if let Some(pid) = task_state.pid {
            terminate_pid(pid)?;
            canceled = true;
        }
    }
    if canceled {
        app.set_status("sent cancel to running tasks".to_string());
    } else {
        app.set_status("no running pids found".to_string());
    }
    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend).context("init terminal")?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor().context("show cursor")?;
    Ok(())
}

fn resolve_run_id(store_root: &Path, run_id: Option<String>) -> Result<String> {
    if let Some(run_id) = run_id {
        return Ok(run_id);
    }
    let mut states = list_states(store_root)?;
    states.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    states
        .first()
        .map(|state| state.run_id.clone())
        .ok_or_else(|| anyhow!("no runs found"))
}

fn load_plan_snapshot(store_root: &Path, run_id: &str) -> Result<Plan> {
    let path = run_dir(store_root, run_id).join("plan.json");
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("read plan snapshot {}", path.display()))?;
    Plan::parse_str(&contents, PlanFormat::Json)
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
            Err(err) => eprintln!("skip run {run_id}: {err}"),
        }
    }
    Ok(states)
}

fn read_state(store_root: &Path, run_id: &str) -> Result<State> {
    let path = run_dir(store_root, run_id).join("state.json");
    let file = fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let state = serde_json::from_reader(file).context("deserialize state")?;
    Ok(state)
}

fn run_dir(store_root: &Path, run_id: &str) -> PathBuf {
    store_root.join("runs").join(run_id)
}

fn run_pid_path(store_root: &Path, run_id: &str) -> PathBuf {
    run_dir(store_root, run_id).join("run.pid")
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
            return Err(anyhow!("failed to terminate pid {pid}"));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(anyhow!("cancel not supported on this platform"))
    }
}

struct WatcherHandle {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
}

impl WatcherHandle {
    fn new(state_path: PathBuf, tasks_dir: PathBuf) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher.watch(&state_path, RecursiveMode::NonRecursive)?;
        watcher.watch(&tasks_dir, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    fn drain(&self, app: &mut App) -> bool {
        let mut dirty = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Ok(_) => dirty = true,
                Err(err) => app.set_status(format!("watch error: {err:#}")),
            }
        }
        dirty
    }
}
