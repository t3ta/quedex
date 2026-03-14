mod cli;

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use clap::Parser;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use uuid::Uuid;

use cli::{Cli, Commands, GlobalOptions, RecoveryOptions};
use quedex::config::{Config, EffectiveOptions};
use quedex::dry_run::{detect_lock_conflicts, generate_execution_waves};
use quedex::git::{self, GitManager};
use quedex::notifier::Notifier;
use quedex::plan::{Plan, PlanFormat, Task};
use quedex::runner::claude_code::ClaudeCodeRunner;
use quedex::runner::codex::CodexRunner;
use quedex::runner::opencode::OpencodeRunner;
use quedex::runner::{ChildHandle, RunContext, Runner, resolve_command_path};
use quedex::scheduler::{
    ScheduleReport, Scheduler, SchedulerOptions, TaskRecord, TaskResult, TaskRunner, TaskSpec,
};
use quedex::store::fs::FsStore;
use quedex::store::recovery::recover_running_tasks;
use quedex::store::{Event, LogStream, RunStatus, SkipReason, State, Store, TaskState, TaskStatus};
use quedex::tui;
use quedex::worktree::{
    WorktreeConfig,
    manager::{TaskWorkdir, WorktreeManager},
};

fn build_mode_concurrency(plan: &Plan) -> HashMap<quedex::plan::TaskMode, usize> {
    use quedex::plan::TaskMode;

    let mut map = HashMap::new();
    if let Some(ref by_mode) = plan.run.max_concurrency_by_mode {
        for (mode_str, &limit) in by_mode {
            let mode = match mode_str.as_str() {
                "research" => TaskMode::Research,
                "implement" => TaskMode::Implement,
                "verify" => TaskMode::Verify,
                _ => continue,
            };
            map.insert(mode, limit);
        }
    }
    map
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match dispatch(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn dispatch(cli: Cli) -> Result<i32> {
    // Load configuration from quedex.toml
    let config = Config::load().unwrap_or_else(|err| {
        eprintln!("Warning: failed to load config: {err}");
        Config::default()
    });

    // Create effective options by merging CLI with config
    let effective = EffectiveOptions::new(
        cli.global.store.clone(),
        cli.global.max_concurrency,
        cli.global.fail_fast,
        cli.global.no_fail_fast,
        &config,
    );

    if cli.global.verbose {
        eprintln!(
            "[verbose] Config loaded: max_concurrency={:?}, fail_fast={:?}, store={:?}",
            config.max_concurrency, config.fail_fast, config.store
        );
        eprintln!(
            "[verbose] Effective options: max_concurrency={:?}, fail_fast={}, store={:?}",
            effective.max_concurrency, effective.fail_fast, effective.store
        );
    }

    match cli.command {
        Commands::Init { output, force } => handle_init(&cli.global, output, force),
        Commands::Run {
            plan,
            recovery,
            run_id,
            base_dir,
            dry_run,
        } => {
            if dry_run {
                handle_dry_run(&cli.global, &plan)
            } else {
                handle_run(
                    &cli.global,
                    &effective,
                    &config,
                    &plan,
                    recovery,
                    run_id,
                    base_dir,
                )
                .await
            }
        }
        Commands::Status {
            run_id,
            group,
            json,
        } => handle_status(&effective, run_id, group, json),
        Commands::Tui { run_id } => handle_tui(&effective, run_id),
        Commands::Logs {
            run_id,
            task_id,
            follow,
            stderr,
        } => handle_logs(&effective, &run_id, &task_id, follow, stderr),
        Commands::Outputs { run_id, task_id } => handle_outputs(&effective, &run_id, task_id),
        Commands::Retry {
            run_id,
            task_id,
            group,
            reload_plan,
        } => {
            handle_retry(
                &cli.global,
                &effective,
                &config,
                &run_id,
                task_id,
                group,
                reload_plan,
            )
            .await
        }
        Commands::Cancel {
            run_id,
            task_id,
            group,
        } => handle_cancel(&effective, &run_id, task_id, group),
        Commands::Clean {
            run_id,
            all,
            fix_orphans,
        } => handle_clean(&cli.global, &effective, run_id, all, fix_orphans),
        Commands::Graph {
            target,
            mermaid,
            ascii,
        } => handle_graph(&effective, &target, mermaid, ascii),
        Commands::History { limit, all, json } => {
            handle_history(&cli.global, &effective, limit, all, json)
        }
        Commands::Schema { output } => handle_schema(output),
        Commands::Stats { since, json } => handle_stats(&cli.global, &effective, since, json),
        Commands::DryRun {
            plan,
            show_order,
            check_locks,
            mermaid,
        } => handle_dry_run_extended(
            &cli.global,
            &effective,
            &plan,
            show_order,
            check_locks,
            mermaid,
        ),
        Commands::Serve { run_id, port, host } => {
            handle_serve(&cli.global, run_id, host, port, &config).await
        }
    }
}

fn handle_init(global: &GlobalOptions, output: Option<PathBuf>, force: bool) -> Result<i32> {
    let output_path = output.unwrap_or_else(|| PathBuf::from("plan.json"));

    if output_path.exists() && !force {
        return Err(anyhow!(
            "file {} already exists (use --force to overwrite)",
            output_path.display()
        ));
    }

    if global.verbose {
        eprintln!(
            "[verbose] Creating plan template at {}",
            output_path.display()
        );
    }

    let is_yaml = matches!(
        output_path.extension().and_then(|ext| ext.to_str()),
        Some("yaml") | Some("yml")
    );

    let template = if is_yaml {
        r#"version: 1
run:
  name: my-plan
  max_concurrency: 2
  fail_fast: true
tasks:
  - id: task-1
    mode: implement
    deps: []
    kind: codex
    codex:
      prompt: |
        Describe what this task should do.
        You can use multi-line strings in YAML.
"#
    } else {
        r#"{
  "version": 1,
  "run": {
    "name": "my-plan",
    "max_concurrency": 2,
    "fail_fast": true
  },
  "tasks": [
    {
      "id": "task-1",
      "mode": "implement",
      "deps": [],
      "codex": {
        "prompt": "Describe what this task should do"
      }
    }
  ]
}
"#
    };

    fs::write(&output_path, template)
        .with_context(|| format!("write plan template to {}", output_path.display()))?;

    println!("Created {}", output_path.display());
    Ok(0)
}

fn handle_dry_run(global: &GlobalOptions, plan_arg: &str) -> Result<i32> {
    let (plan, _plan_base_dir) = load_plan(plan_arg)?;

    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }

    println!("Dry run mode - no tasks will be executed");
    println!();
    println!("Plan: {}", plan.run.name.as_deref().unwrap_or("(unnamed)"));
    println!("Tasks: {}", plan.tasks.len());

    if global.verbose {
        eprintln!("[verbose] Max concurrency: {:?}", plan.run.max_concurrency);
        eprintln!("[verbose] Fail fast: {:?}", plan.run.fail_fast);
    }

    // Build dependency graph to determine execution order
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for task in &plan.tasks {
        in_degree.entry(task.id.as_str()).or_insert(0);
        for dep in &task.deps {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task.id.as_str());
            *in_degree.entry(task.id.as_str()).or_insert(0) += 1;
        }
    }

    // Topological sort using Kahn's algorithm
    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| *id)
        .collect();
    queue.sort(); // Sort for deterministic output

    let mut order = Vec::new();
    while let Some(task_id) = queue.pop() {
        order.push(task_id);
        if let Some(deps) = dependents.get(task_id) {
            for &dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dep);
                        queue.sort();
                    }
                }
            }
        }
    }

    println!();
    println!("Execution order:");
    for (i, task_id) in order.iter().enumerate() {
        let task = plan.tasks.iter().find(|t| t.id == *task_id).unwrap();
        let deps_str = if task.deps.is_empty() {
            "none".to_string()
        } else {
            task.deps.join(", ")
        };
        let runner = if task.codex.is_some() {
            "codex"
        } else if task.claude_code.is_some() {
            "claude_code"
        } else {
            "opencode"
        };
        println!("  {}. {} (deps: {}) [{}]", i + 1, task_id, deps_str, runner);
    }

    Ok(0)
}

/// Extended dry-run handler with wave analysis, lock checking, and mermaid output
fn handle_dry_run_extended(
    global: &GlobalOptions,
    effective: &EffectiveOptions,
    plan_arg: &str,
    show_order: bool,
    check_locks: bool,
    mermaid: bool,
) -> Result<i32> {
    let (plan, _plan_base_dir) = load_plan(plan_arg)?;

    if let Err(err) = plan.validate() {
        eprintln!("plan validation error: {err}");
        return Ok(3);
    }

    // If no specific option is given, show basic dry-run output
    let show_basic = !show_order && !check_locks && !mermaid;

    if show_basic {
        println!("Dry run mode - no tasks will be executed");
        println!();
    }

    println!("Plan: {}", plan.run.name.as_deref().unwrap_or("(unnamed)"));
    println!("Tasks: {}", plan.tasks.len());

    // Show group count if any groups exist
    let groups = plan.resolve_groups();
    if !groups.is_empty() {
        println!("Groups: {}", groups.len());
    }

    let max_concurrency = plan
        .run
        .max_concurrency
        .or(effective.max_concurrency)
        .unwrap_or(1);

    if global.verbose {
        eprintln!("[verbose] Max concurrency: {max_concurrency}");
        eprintln!("[verbose] Fail fast: {:?}", plan.run.fail_fast);
    }

    // Mermaid output
    if mermaid {
        println!();
        print_mermaid_graph(&plan);
        return Ok(0);
    }

    // Generate waves for execution order analysis
    let waves = generate_execution_waves(&plan, max_concurrency);

    // Build task_id -> group_name mapping for display
    let mut task_to_group: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for (group_name, task_ids) in &groups {
        for task_id in task_ids {
            task_to_group.insert(task_id.as_str(), group_name.as_str());
        }
    }

    // Show execution order in waves
    if show_order || show_basic {
        println!();
        println!("Execution order (max_concurrency={max_concurrency}):");
        for (wave_idx, wave) in waves.iter().enumerate() {
            // Format task IDs with group prefix if groups exist
            let task_display: Vec<String> = wave
                .iter()
                .map(|task_id| {
                    if let Some(group) = task_to_group.get(task_id.as_str()) {
                        format!("[{group}]{task_id}")
                    } else {
                        task_id.clone()
                    }
                })
                .collect();

            // Collect dependencies for all tasks in this wave
            let all_deps: Vec<String> = wave
                .iter()
                .filter_map(|task_id| {
                    plan.tasks
                        .iter()
                        .find(|t| t.id == *task_id)
                        .map(|t| t.deps.clone())
                })
                .flatten()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            // Build annotation
            let annotation = if wave.len() > 1 {
                if all_deps.is_empty() {
                    " (parallel)".to_string()
                } else {
                    let mut deps_sorted: Vec<_> = all_deps;
                    deps_sorted.sort();
                    format!(" (parallel, depends on {})", deps_sorted.join(", "))
                }
            } else if !all_deps.is_empty() {
                let mut deps_sorted: Vec<_> = all_deps;
                deps_sorted.sort();
                format!(" (depends on {})", deps_sorted.join(", "))
            } else {
                String::new()
            };

            println!(
                "  Wave {}: [{}]{}",
                wave_idx + 1,
                task_display.join(", "),
                annotation
            );

            // Show dependencies for each task in verbose mode
            if global.verbose {
                for task_id in wave {
                    if let Some(task) = plan.tasks.iter().find(|t| t.id == *task_id) {
                        let deps_str = if task.deps.is_empty() {
                            "none".to_string()
                        } else {
                            task.deps.join(", ")
                        };
                        let locks_str = if task.locks.is_empty() {
                            "none".to_string()
                        } else {
                            task.locks.join(", ")
                        };
                        eprintln!(
                            "    [verbose] {task_id} - deps: [{deps_str}], locks: [{locks_str}]"
                        );
                    }
                }
            }
        }
    }

    // Check for lock conflicts
    if check_locks {
        println!();
        let conflicts = detect_lock_conflicts(&plan);
        if conflicts.is_empty() {
            println!("Lock conflicts: none detected");
        } else {
            println!("Potential lock conflicts detected:");
            for conflict in &conflicts {
                println!(
                    "  Lock '{}': tasks [{}] may compete",
                    conflict.lock_name,
                    conflict.task_ids.join(", ")
                );
                if !conflict.notes.is_empty() {
                    println!("    Note: {}", conflict.notes);
                }
            }
        }
    }

    Ok(0)
}

fn handle_history(
    global: &GlobalOptions,
    effective: &EffectiveOptions,
    limit: usize,
    all: bool,
    json: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;

    if global.verbose {
        eprintln!("[verbose] Reading history from {}", store_root.display());
    }

    let mut states = list_states(&store_root)?;
    states.sort_by_key(|state| std::cmp::Reverse(state.started_at));

    let display_limit = if all { states.len() } else { limit };
    let states: Vec<_> = states.into_iter().take(display_limit).collect();

    if json {
        let text = serde_json::to_string_pretty(&states)?;
        println!("{text}");
    } else {
        if states.is_empty() {
            println!("No execution history found");
            return Ok(0);
        }
        println!(
            "{:<36} {:<10} {:<20} {:<20} name",
            "run_id", "status", "started_at", "completed_at"
        );
        for state in &states {
            let completed_str = state
                .completed_at
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{:<36} {:<10} {:<20} {:<20} {}",
                state.run_id,
                format!("{:?}", state.status),
                state.started_at.format("%Y-%m-%d %H:%M:%S"),
                completed_str,
                state.run_name
            );
        }
    }

    Ok(0)
}

fn handle_schema(output: Option<PathBuf>) -> Result<i32> {
    let schema = quedex::plan::plan_json_schema();
    let schema_json = serde_json::to_string_pretty(&schema)?;

    if let Some(output_path) = output {
        fs::write(&output_path, &schema_json)
            .with_context(|| format!("write schema to {}", output_path.display()))?;
        println!("Schema written to {}", output_path.display());
    } else {
        println!("{schema_json}");
    }

    Ok(0)
}

/// Statistics output structure for JSON serialization
#[derive(serde::Serialize)]
struct StatsOutput {
    period: StatsPeriod,
    total_runs: usize,
    successful_runs: usize,
    failed_runs: usize,
    success_rate: f64,
    avg_duration_seconds: Option<i64>,
    most_failed_task: Option<TaskFailureInfo>,
    longest_task: Option<TaskDurationInfo>,
}

#[derive(serde::Serialize)]
struct StatsPeriod {
    since: Option<String>,
    until: String,
}

#[derive(serde::Serialize)]
struct TaskFailureInfo {
    task_id: String,
    failure_count: usize,
}

#[derive(serde::Serialize)]
struct TaskDurationInfo {
    task_id: String,
    avg_duration_seconds: i64,
}

/// Parse duration string like "7d", "24h", "1w" into chrono::Duration
fn parse_duration(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("Empty duration string"));
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str
        .parse()
        .with_context(|| format!("Invalid duration number: {num_str}"))?;

    match unit {
        "s" => Ok(chrono::Duration::seconds(num)),
        "m" => Ok(chrono::Duration::minutes(num)),
        "h" => Ok(chrono::Duration::hours(num)),
        "d" => Ok(chrono::Duration::days(num)),
        "w" => Ok(chrono::Duration::weeks(num)),
        _ => Err(anyhow!(
            "Invalid duration format: {s}. Use s, m, h, d, or w suffix"
        )),
    }
}

/// Format duration in human-readable format (e.g., "2m 34s")
fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        let mins = seconds / 60;
        let secs = seconds % 60;
        if secs > 0 {
            format!("{mins}m {secs}s")
        } else {
            format!("{mins}m")
        }
    } else {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        if mins > 0 {
            format!("{hours}h {mins}m")
        } else {
            format!("{hours}h")
        }
    }
}

fn handle_stats(
    global: &GlobalOptions,
    effective: &EffectiveOptions,
    since: Option<String>,
    json_output: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;

    if global.verbose {
        eprintln!("[verbose] Reading stats from {}", store_root.display());
    }

    let mut states = list_states(&store_root)?;

    // Parse and apply period filter
    let cutoff = if let Some(ref since_str) = since {
        let duration = parse_duration(since_str)?;
        Some(Utc::now() - duration)
    } else {
        None
    };

    if let Some(cutoff_time) = cutoff {
        states.retain(|s| s.started_at >= cutoff_time);
        if global.verbose {
            eprintln!(
                "[verbose] Filtered to {} runs since {}",
                states.len(),
                cutoff_time
            );
        }
    }

    // Calculate statistics
    let total_runs = states.len();
    let successful_runs = states
        .iter()
        .filter(|s| s.status == RunStatus::Completed)
        .count();
    let failed_runs = states
        .iter()
        .filter(|s| s.status == RunStatus::Failed)
        .count();
    let success_rate = if total_runs > 0 {
        (successful_runs as f64 / total_runs as f64) * 100.0
    } else {
        0.0
    };

    // Calculate average run duration
    let durations: Vec<i64> = states
        .iter()
        .filter_map(|s| s.completed_at.map(|end| (end - s.started_at).num_seconds()))
        .collect();
    let avg_duration = if !durations.is_empty() {
        Some(durations.iter().sum::<i64>() / durations.len() as i64)
    } else {
        None
    };

    // Find most failed task
    let mut task_failures: HashMap<String, usize> = HashMap::new();
    for state in &states {
        for (task_id, task) in &state.tasks {
            if task.status == TaskStatus::Failed {
                *task_failures.entry(task_id.clone()).or_default() += 1;
            }
        }
    }
    let most_failed = task_failures
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(id, count)| TaskFailureInfo {
            task_id: id.clone(),
            failure_count: *count,
        });

    // Find longest task (by average duration)
    let mut task_durations: HashMap<String, Vec<i64>> = HashMap::new();
    for state in &states {
        for (task_id, task) in &state.tasks {
            if let (Some(start), Some(end)) = (task.started_at, task.completed_at) {
                task_durations
                    .entry(task_id.clone())
                    .or_default()
                    .push((end - start).num_seconds());
            }
        }
    }
    let longest_task = task_durations
        .iter()
        .filter_map(|(id, durs)| {
            if durs.is_empty() {
                return None;
            }
            let avg = durs.iter().sum::<i64>() / durs.len() as i64;
            Some(TaskDurationInfo {
                task_id: id.clone(),
                avg_duration_seconds: avg,
            })
        })
        .max_by_key(|info| info.avg_duration_seconds);

    // Output results
    if json_output {
        let output = StatsOutput {
            period: StatsPeriod {
                since: cutoff.map(|t| t.to_rfc3339()),
                until: Utc::now().to_rfc3339(),
            },
            total_runs,
            successful_runs,
            failed_runs,
            success_rate: (success_rate * 100.0).round() / 100.0,
            avg_duration_seconds: avg_duration,
            most_failed_task: most_failed,
            longest_task,
        };
        let text = serde_json::to_string_pretty(&output)?;
        println!("{text}");
    } else {
        // Text output
        let period_str = if let Some(ref s) = since {
            format!("last {s}")
        } else {
            "all time".to_string()
        };
        println!("Execution Statistics ({period_str})");
        println!("{}", "=".repeat(40));
        println!("Total runs:       {total_runs}");
        println!("Success rate:     {success_rate:.1}% ({successful_runs}/{total_runs})");
        if let Some(avg) = avg_duration {
            println!("Avg duration:     {}", format_duration(avg));
        } else {
            println!("Avg duration:     -");
        }
        if let Some(ref mf) = most_failed {
            println!(
                "Most failed task: {} ({} failures)",
                mf.task_id, mf.failure_count
            );
        } else {
            println!("Most failed task: -");
        }
        if let Some(ref lt) = longest_task {
            println!(
                "Longest task:     {} (avg {})",
                lt.task_id,
                format_duration(lt.avg_duration_seconds)
            );
        } else {
            println!("Longest task:     -");
        }
    }

    Ok(0)
}

fn expand_env_placeholders(value: &str, base_env: &HashMap<String, String>) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '$' {
            output.push(ch);
            continue;
        }

        match chars.peek() {
            Some('{') => {
                chars.next();
                let mut token = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    token.push(c);
                }
                let key = token.strip_prefix("env.").unwrap_or(&token);
                if closed {
                    if let Some(val) = base_env.get(key) {
                        output.push_str(val);
                    } else {
                        output.push_str("${");
                        output.push_str(&token);
                        output.push('}');
                    }
                } else {
                    output.push_str("${");
                    output.push_str(&token);
                }
            }
            Some(c) if c.is_ascii_alphanumeric() || *c == '_' => {
                let mut key = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        chars.next();
                        key.push(c);
                    } else {
                        break;
                    }
                }
                if let Some(val) = base_env.get(&key) {
                    output.push_str(val);
                } else {
                    output.push('$');
                    output.push_str(&key);
                }
            }
            _ => {
                output.push('$');
            }
        }
    }

    output
}

fn build_run_env(plan_env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let base_env: HashMap<String, String> = std::env::vars().collect();
    let mut env = base_env.clone();
    if let Some(plan_env) = plan_env {
        for (key, value) in plan_env {
            let expanded = expand_env_placeholders(value, &base_env);
            env.insert(key.clone(), expanded);
        }
    }
    env
}

async fn handle_run(
    _global: &GlobalOptions,
    effective: &EffectiveOptions,
    config: &Config,
    plan_arg: &str,
    recovery: RecoveryOptions,
    run_id: Option<String>,
    base_dir: Option<PathBuf>,
) -> Result<i32> {
    let (mut plan, mut plan_base_dir) = load_plan(plan_arg)?;
    if let Some(base_dir) = base_dir {
        plan_base_dir = base_dir;
    }
    let store_root = resolve_store_path(effective.store.as_ref())?;
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
            return Err(anyhow!("run {run_id} has no state to resume"));
        }
        if snapshot_path.exists() {
            plan = load_plan_snapshot(&store_root, &run_id)?;
        }
    } else if state_path.exists() {
        return Err(anyhow!(
            "run {run_id} already exists (use --resume or --clean-start)"
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

    #[allow(clippy::collapsible_if)]
    if plan.tasks.iter().any(|task| {
        matches!(task.kind.as_deref(), Some("claude_code")) || task.claude_code.is_some()
    }) {
        if let Err(err) = check_claude_code_available() {
            eprintln!("environment error: {err}");
            return Ok(4);
        }
    }

    #[allow(clippy::collapsible_if)]
    if plan
        .tasks
        .iter()
        .any(|task| matches!(task.kind.as_deref(), Some("opencode")) || task.opencode.is_some())
    {
        if let Err(err) = check_opencode_available() {
            eprintln!("environment error: {err}");
            return Ok(4);
        }
    }

    if !recovery.resume || !snapshot_path.exists() {
        let _ = write_plan_snapshot(&store_root, &run_id, &plan)?;
    }

    let store = Arc::new(FsStore::new(&store_root, &run_id)?);
    let cwd = resolve_run_cwd(&plan, plan_base_dir)?;
    #[allow(deprecated)]
    let worktree_manager = plan.run.worktree.as_ref().and_then(|wt_config| {
        if wt_config.enabled {
            Some(Arc::new(WorktreeManager::new(
                cwd.clone(),
                WorktreeConfig {
                    enabled: true,
                    base_dir: wt_config.base_dir.clone(),
                    shallow_depth: wt_config.shallow_depth,
                },
            )))
        } else {
            None
        }
    });

    // Merge system environment variables with plan.run.env
    // plan.run.env takes precedence over system env vars
    let env = build_run_env(plan.run.env.as_ref());

    // Resolve system_prompt: plan-level overrides config-level
    let system_prompt = plan
        .run
        .system_prompt
        .clone()
        .or_else(|| config.system_prompt.clone());

    let ctx = RunContext {
        cwd,
        env: env.clone(),
        store: store.clone(),
        worktree_manager,
        system_prompt,
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
                    output_files: None,
                    pid: None,
                    skip_reason: None,
                },
            );
        }

        let state = State {
            run_id: run_id.clone(),
            run_name: plan.run.name.clone().unwrap_or_else(|| run_id.clone()),
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
            output_files: task.output_files.clone(),
            condition: task.condition.clone(),
            title: task.title.clone(),
            mode: task.mode,
            auto_commit: task.auto_commit,
            squash: task.squash,
        })
        .collect();

    let max_concurrency = plan
        .run
        .max_concurrency
        .or(effective.max_concurrency)
        .unwrap_or(1);
    let mode_concurrency = build_mode_concurrency(&plan);
    let fail_fast = plan.run.fail_fast.unwrap_or(effective.fail_fast);

    // Create notifier for webhook notifications
    let run_name = plan.run.name.clone().unwrap_or_else(|| run_id.clone());
    let notifier = Notifier::new(plan.run.notifications.clone(), run_id.clone(), run_name);

    // Send run started notification
    if let Some(ref n) = notifier {
        n.notify_start();
    }

    let runner = PlanTaskRunner::new(
        Arc::new(tasks_map),
        Arc::new(plan.profiles.clone()),
        ctx,
        state_handle.clone(),
        cancel.clone(),
        notifier.clone(),
    );

    // Create GitManager for auto-commit functionality
    let git_manager = match crate::git::GitManager::open() {
        Ok(gm) => Some(Arc::new(std::sync::Mutex::new(gm))),
        Err(_) => None, // Not in a git repository, skip auto-commit
    };

    let scheduler = if let Some(initial_states) = initial_states {
        let sched = Scheduler::new_with_initial_state(
            task_specs,
            SchedulerOptions {
                max_concurrency,
                fail_fast,
                mode_concurrency: mode_concurrency.clone(),
            },
            runner,
            initial_states,
        );
        if let Some(gm) = git_manager {
            sched.with_git_manager_for_state(gm)
        } else {
            sched
        }
    } else {
        let sched = Scheduler::new(
            task_specs,
            SchedulerOptions {
                max_concurrency,
                fail_fast,
                mode_concurrency: mode_concurrency.clone(),
            },
            runner,
        );
        if let Some(gm) = git_manager {
            sched.with_git_manager(gm)
        } else {
            sched
        }
    };

    // Collect environment variables for condition evaluation
    // Start with system environment, then overlay plan-defined env vars
    let env_vars = env.clone();

    let report = scheduler.run(&env_vars).await;
    reconcile_state(&state_handle, &report)?;

    // Handle squash tasks and create final commit
    let squash_result = handle_squash_tasks(&plan, &report);

    if let Err(ref err) = squash_result {
        eprintln!("Warning: squash failed: {}", err);
    }

    state = state_handle.snapshot();
    let (run_status, exit_code) = finalize_run_status(&state);
    state_handle.update_run_status(run_status)?;
    remove_run_pid(&store_root, &run_id);

    // Send completion/failure notification
    if let Some(ref n) = notifier {
        let total = state.tasks.len();
        let succeeded = state
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Succeeded)
            .count();
        let failed = state
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Failed)
            .count();

        if run_status == RunStatus::Completed {
            n.notify_complete(total, succeeded);
        } else if run_status == RunStatus::Failed {
            n.notify_failure(total, failed, succeeded);
        }
    }

    Ok(exit_code)
}

fn handle_status(
    effective: &EffectiveOptions,
    run_id: Option<String>,
    group: Option<String>,
    json: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;
    if let Some(run_id) = run_id {
        let mut state = read_state(&store_root, &run_id)?;

        // Filter by group if specified
        if let Some(ref group_name) = group {
            let plan = load_plan_snapshot(&store_root, &run_id)?;
            let resolved_groups = plan.resolve_groups();
            let group_task_ids = resolved_groups.get(group_name).ok_or_else(|| {
                anyhow!(
                    "group '{}' not found. Available groups: {:?}",
                    group_name,
                    resolved_groups.keys().collect::<Vec<_>>()
                )
            })?;

            let group_set: std::collections::HashSet<_> = group_task_ids.iter().collect();
            state.tasks.retain(|task_id, _| group_set.contains(task_id));
        }

        if json {
            let text = serde_json::to_string_pretty(&state)?;
            println!("{text}");
        } else {
            print_state(&state);
        }
        return Ok(0);
    }

    // run_id未指定時はグループフィルタは無視
    if group.is_some() {
        eprintln!("Warning: --group requires run_id, ignoring");
    }

    let mut states = list_states(&store_root)?;
    states.sort_by_key(|state| std::cmp::Reverse(state.started_at));

    if json {
        let text = serde_json::to_string_pretty(&states)?;
        println!("{text}");
    } else {
        print_states_table(&states);
    }
    Ok(0)
}

fn handle_tui(effective: &EffectiveOptions, run_id: Option<String>) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;

    // run_idが指定されている場合は従来通りTUI起動
    if run_id.is_some() {
        return tui::run(store_root, run_id);
    }

    // run_idが指定されていない場合、インタラクティブ選択
    let runs = list_runs_with_activity(&store_root)?;

    if runs.is_empty() {
        println!("no runs found");
        return Ok(0);
    }

    // アクティブなrunを抽出
    let active_runs: Vec<_> = runs.iter().filter(|r| r.is_active).collect();

    // アクティブなrunが1件のみの場合は自動選択
    if active_runs.len() == 1 {
        let run_id = active_runs[0].state.run_id.clone();
        return tui::run(store_root, Some(run_id));
    }

    // 複数のrunがある場合はインタラクティブ選択UI
    match select_run_interactive(&runs)? {
        Some(selected_run_id) => tui::run(store_root, Some(selected_run_id)),
        None => {
            println!("canceled");
            Ok(0)
        }
    }
}

fn handle_logs(
    effective: &EffectiveOptions,
    run_id: &str,
    task_id: &str,
    follow: bool,
    stderr: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;
    let path = log_path(&store_root, run_id, task_id, stderr);
    if !path.exists() {
        return Err(anyhow!("log file not found: {}", path.display()));
    }
    stream_log(&path, follow)?;
    Ok(0)
}

const OUTPUT_PREVIEW_LINES: usize = 5;

fn handle_outputs(
    effective: &EffectiveOptions,
    run_id: &str,
    task_id: Option<String>,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;
    let run_root = run_dir(&store_root, run_id);
    if !run_root.exists() {
        return Err(anyhow!("run {run_id} not found"));
    }

    let outputs_root = run_root.join("outputs");
    if !outputs_root.exists() {
        println!("no outputs found");
        return Ok(0);
    }

    let tasks = resolve_output_tasks(&outputs_root, task_id.as_deref())?;
    if tasks.is_empty() {
        println!("no outputs found");
        return Ok(0);
    }

    let mut printed_header = false;
    let mut any_files = false;

    for (task_id, task_dir) in tasks {
        let mut files = collect_output_files(&task_dir)?;
        files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        if files.is_empty() {
            continue;
        }

        if !printed_header {
            println!("{:<16} {:<40} {:>10}", "task", "file", "size");
            printed_header = true;
        }

        for path in files {
            let relative = path.strip_prefix(&task_dir).unwrap_or(&path);
            let size = fs::metadata(&path)
                .with_context(|| format!("stat {}", path.display()))?
                .len();
            println!("{:<16} {:<40} {:>10}", task_id, relative.display(), size);
            let preview = read_preview_lines(&path, OUTPUT_PREVIEW_LINES)?;
            for line in preview {
                println!("  {}", line);
            }
            any_files = true;
        }
    }

    if !any_files {
        println!("no outputs found");
    }

    Ok(0)
}

fn resolve_output_tasks(
    outputs_root: &Path,
    task_id: Option<&str>,
) -> Result<Vec<(String, PathBuf)>> {
    if let Some(task_id) = task_id {
        let task_dir = outputs_root.join(task_id);
        if !task_dir.exists() {
            return Err(anyhow!("outputs for task {task_id} not found"));
        }
        return Ok(vec![(task_id.to_string(), task_dir)]);
    }

    let mut tasks = Vec::new();
    for entry in
        fs::read_dir(outputs_root).with_context(|| format!("read {}", outputs_root.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            tasks.push((name, entry.path()));
        }
    }
    tasks.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tasks)
}

fn collect_output_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }

    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in
            fs::read_dir(&current).with_context(|| format!("read {}", current.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_type.is_file() {
                files.push(path);
                continue;
            }
            if file_type.is_symlink()
                && let Ok(metadata) = fs::metadata(&path)
                && metadata.is_file()
            {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn read_preview_lines(path: &Path, max_lines: usize) -> Result<Vec<String>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut buffer = Vec::new();

    while lines.len() < max_lines {
        buffer.clear();
        let bytes = reader.read_until(b'\n', &mut buffer)?;
        if bytes == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buffer);
        lines.push(text.trim_end_matches(['\n', '\r']).to_string());
    }

    Ok(lines)
}

/// Collect all downstream tasks (direct and indirect dependents) from the given target task IDs.
/// Returns a set of task IDs that depend (directly or transitively) on any of the target tasks.
fn collect_downstream_tasks(plan: &Plan, target_ids: &HashSet<String>) -> HashSet<String> {
    // Build dependents map: task_id -> list of tasks that depend on it
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in &plan.tasks {
        for dep in &task.deps {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task.id.as_str());
        }
    }

    // BFS to collect all downstream tasks
    let mut downstream: HashSet<String> = HashSet::new();
    let mut queue: Vec<&str> = target_ids.iter().map(|s| s.as_str()).collect();

    while let Some(task_id) = queue.pop() {
        if let Some(deps) = dependents.get(task_id) {
            for &dep in deps {
                if !target_ids.contains(dep) && downstream.insert(dep.to_string()) {
                    queue.push(dep);
                }
            }
        }
    }

    downstream
}

async fn handle_retry(
    _global: &GlobalOptions,
    effective: &EffectiveOptions,
    config: &Config,
    run_id: &str,
    task_id: Option<String>,
    group: Option<String>,
    reload_plan: bool,
) -> Result<i32> {
    // Validate that either task_id or group is provided
    if task_id.is_none() && group.is_none() {
        return Err(anyhow!("either task_id or --group must be specified"));
    }

    let store_root = resolve_store_path(effective.store.as_ref())?;

    if reload_plan {
        eprintln!(
            "Note: Reloading plan from run directory's plan.json (any manual edits will be used)"
        );
    }
    let plan = load_plan_snapshot(&store_root, run_id)?;
    let mut state = read_state(&store_root, run_id)?;

    if state.status == RunStatus::Running {
        return Err(anyhow!("run {run_id} is still running"));
    }

    // Resolve target tasks from task_id or group
    let target_task_ids: Vec<String> = if let Some(ref group_name) = group {
        let resolved_groups = plan.resolve_groups();
        let group_task_ids = resolved_groups.get(group_name).ok_or_else(|| {
            anyhow!(
                "group '{}' not found. Available groups: {:?}",
                group_name,
                resolved_groups.keys().collect::<Vec<_>>()
            )
        })?;
        group_task_ids.clone()
    } else {
        vec![task_id.unwrap()]
    };

    // Collect tasks to retry (Failed/Canceled/Skipped with satisfied dependencies)
    let mut tasks_to_retry: Vec<Task> = Vec::new();

    for tid in &target_task_ids {
        let task = plan
            .tasks
            .iter()
            .find(|t| &t.id == tid)
            .ok_or_else(|| anyhow!("task {tid} not found in plan"))?;

        let task_state = state
            .tasks
            .get(tid)
            .ok_or_else(|| anyhow!("task {tid} not found in state for run {run_id}"))?;

        // Check if task is in a retryable state
        if !matches!(
            task_state.status,
            TaskStatus::Failed | TaskStatus::Canceled | TaskStatus::Skipped
        ) {
            if group.is_some() {
                // For group mode, skip non-retryable tasks silently
                continue;
            }
            return Err(anyhow!(
                "task {} must be Failed, Canceled or Skipped to retry (current: {:?})",
                tid,
                task_state.status
            ));
        }

        // Check dependencies are satisfied
        let mut deps_satisfied = true;
        for dep in &task.deps {
            let dep_state = state
                .tasks
                .get(dep)
                .ok_or_else(|| anyhow!("task {tid} dependency {dep} not found in state"))?;
            if dep_state.status != TaskStatus::Succeeded {
                if group.is_some() {
                    deps_satisfied = false;
                    break;
                }
                return Err(anyhow!(
                    "task {} dependency {} not satisfied (current: {:?})",
                    tid,
                    dep,
                    dep_state.status
                ));
            }
        }

        if !deps_satisfied {
            continue;
        }

        tasks_to_retry.push(task.clone());
    }

    if tasks_to_retry.is_empty() {
        if let Some(group_name) = group.as_ref() {
            println!("No retryable tasks found in group '{}'", group_name);
            return Ok(0);
        }
        return Err(anyhow!("no tasks to retry"));
    }

    // Collect downstream tasks that were skipped due to dependency failure or fail-fast
    let retry_target_ids: HashSet<String> = tasks_to_retry.iter().map(|t| t.id.clone()).collect();
    let downstream_task_ids = collect_downstream_tasks(&plan, &retry_target_ids);

    let mut downstream_tasks: Vec<Task> = Vec::new();
    for task in &plan.tasks {
        if downstream_task_ids.contains(&task.id)
            && let Some(task_state) = state.tasks.get(&task.id)
        {
            // Only include Skipped tasks with DependencyFailed or FailFast reason
            // ConditionNotMet tasks should NOT be auto-retried (condition should be re-evaluated)
            if task_state.status == TaskStatus::Skipped {
                match &task_state.skip_reason {
                    Some(SkipReason::DependencyFailed) | Some(SkipReason::FailFast) => {
                        // Check if all non-retry-set dependencies are Succeeded
                        let task_statuses: HashMap<String, TaskStatus> = state
                            .tasks
                            .iter()
                            .map(|(id, ts)| (id.clone(), ts.status))
                            .collect();

                        if has_unsatisfied_external_deps(
                            &task.deps,
                            &retry_target_ids,
                            &task_statuses,
                        ) {
                            eprintln!(
                                "Note: Excluding downstream task '{}' from retry - has unsatisfied dependencies outside retry set",
                                task.id
                            );
                        } else {
                            downstream_tasks.push(task.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Combine all tasks for runner availability check
    let all_retry_tasks: Vec<&Task> = tasks_to_retry
        .iter()
        .chain(downstream_tasks.iter())
        .collect();

    // Check runner availability for all tasks (including downstream)
    for task in &all_retry_tasks {
        if matches!(task.kind.as_deref(), Some("codex")) || task.codex.is_some() {
            check_codex_available()?;
        }
        if matches!(task.kind.as_deref(), Some("claude_code")) || task.claude_code.is_some() {
            check_claude_code_available()?;
        }
        if matches!(task.kind.as_deref(), Some("opencode")) || task.opencode.is_some() {
            check_opencode_available()?;
        }
    }

    // Reset task states for retry targets
    for task in &tasks_to_retry {
        let task_state = state.tasks.get_mut(&task.id).unwrap();
        task_state.status = TaskStatus::Pending;
        task_state.exit_code = None;
        task_state.stderr_tail = None;
        task_state.started_at = None;
        task_state.completed_at = None;
        task_state.pid = None;
    }

    // Reset task states for downstream tasks
    for task in &downstream_tasks {
        let task_state = state.tasks.get_mut(&task.id).unwrap();
        task_state.status = TaskStatus::Pending;
        task_state.exit_code = None;
        task_state.stderr_tail = None;
        task_state.started_at = None;
        task_state.completed_at = None;
        task_state.pid = None;
        task_state.skip_reason = None;
    }

    state.status = RunStatus::Running;
    state.completed_at = None;

    let store = Arc::new(FsStore::new(&store_root, run_id)?);
    store.write_state(state.clone())?;
    write_run_pid(&store_root, run_id)?;

    let base_dir = env::current_dir().context("resolve current dir for retry")?;
    let cwd = resolve_run_cwd(&plan, base_dir)?;
    #[allow(deprecated)]
    let worktree_manager = plan.run.worktree.as_ref().and_then(|wt_config| {
        if wt_config.enabled {
            Some(Arc::new(WorktreeManager::new(
                cwd.clone(),
                WorktreeConfig {
                    enabled: true,
                    base_dir: wt_config.base_dir.clone(),
                    shallow_depth: wt_config.shallow_depth,
                },
            )))
        } else {
            None
        }
    });

    // Merge system environment variables with plan.run.env
    // plan.run.env takes precedence over system env vars
    let env = build_run_env(plan.run.env.as_ref());

    // Resolve system_prompt: plan-level overrides config-level
    let system_prompt = plan
        .run
        .system_prompt
        .clone()
        .or_else(|| config.system_prompt.clone());

    let ctx = RunContext {
        cwd,
        env: env.clone(),
        store: store.clone(),
        worktree_manager,
        system_prompt,
    };

    let state_handle = StateHandle::new(store.clone(), state);
    let cancel = CancelHandle::new();
    spawn_cancel_listener(cancel.clone());

    // Build task map and specs for all tasks to retry (including downstream)
    let mut tasks_map = HashMap::new();
    let mut task_specs = Vec::new();

    // Collect all task IDs that are part of this retry operation
    let all_retry_task_ids: HashSet<String> = tasks_to_retry
        .iter()
        .chain(downstream_tasks.iter())
        .map(|t| t.id.clone())
        .collect();

    // Add retry target tasks (no dependencies - their deps are already satisfied)
    for task in &tasks_to_retry {
        tasks_map.insert(task.id.clone(), task.clone());
        task_specs.push(TaskSpec {
            id: task.id.clone(),
            deps: Vec::new(), // Dependencies already satisfied
            locks: task.locks.clone(),
            output_files: task.output_files.clone(),
            condition: task.condition.clone(),
            title: task.title.clone(),
            mode: task.mode,
            auto_commit: task.auto_commit,
            squash: task.squash,
        });
    }

    // Add downstream tasks with filtered dependencies
    // Keep only dependencies that are within the retry set
    for task in &downstream_tasks {
        tasks_map.insert(task.id.clone(), task.clone());

        // Filter dependencies: only keep those in the retry set
        let filtered_deps: Vec<String> = task
            .deps
            .iter()
            .filter(|dep| all_retry_task_ids.contains(*dep))
            .cloned()
            .collect();

        task_specs.push(TaskSpec {
            id: task.id.clone(),
            deps: filtered_deps,
            locks: task.locks.clone(),
            output_files: task.output_files.clone(),
            condition: task.condition.clone(),
            title: task.title.clone(),
            mode: task.mode,
            auto_commit: task.auto_commit,
            squash: task.squash,
        });
    }

    // Print retry message with downstream task count
    let downstream_count = downstream_tasks.len();
    if downstream_count > 0 {
        println!(
            "Retrying {} task(s) + {} downstream task(s)...",
            tasks_to_retry.len(),
            downstream_count
        );
    } else {
        println!("Retrying {} task(s)...", tasks_to_retry.len());
    }

    let max_concurrency = plan
        .run
        .max_concurrency
        .or(effective.max_concurrency)
        .unwrap_or(1);
    let mode_concurrency = build_mode_concurrency(&plan);
    let fail_fast = plan.run.fail_fast.unwrap_or(effective.fail_fast);

    // Notifier for retry (no notifications for retry operations)
    let runner = PlanTaskRunner::new(
        Arc::new(tasks_map),
        Arc::new(plan.profiles.clone()),
        ctx,
        state_handle.clone(),
        cancel,
        None,
    );

    // Create GitManager for auto-commit functionality
    let git_manager = match crate::git::GitManager::open() {
        Ok(gm) => Some(Arc::new(std::sync::Mutex::new(gm))),
        Err(_) => None, // Not in a git repository, skip auto-commit
    };

    let scheduler = Scheduler::new(
        task_specs,
        SchedulerOptions {
            max_concurrency,
            fail_fast,
            mode_concurrency,
        },
        runner,
    );

    let scheduler = if let Some(gm) = git_manager {
        scheduler.with_git_manager(gm)
    } else {
        scheduler
    };

    // Collect environment variables for condition evaluation
    let env_vars = env.clone();

    let report = scheduler.run(&env_vars).await;
    reconcile_state(&state_handle, &report)?;

    let state = state_handle.snapshot();
    let (run_status, exit_code) = finalize_run_status(&state);
    state_handle.update_run_status(run_status)?;
    remove_run_pid(&store_root, run_id);
    Ok(exit_code)
}

fn handle_cancel(
    effective: &EffectiveOptions,
    run_id: &str,
    task_id: Option<String>,
    group: Option<String>,
) -> Result<i32> {
    // Validate that task_id and group are not both specified
    if task_id.is_some() && group.is_some() {
        return Err(anyhow!("cannot specify both task_id and --group"));
    }

    let store_root = resolve_store_path(effective.store.as_ref())?;

    // Handle --group option
    if let Some(ref group_name) = group {
        let plan = load_plan_snapshot(&store_root, run_id)?;
        let resolved_groups = plan.resolve_groups();
        let group_task_ids = resolved_groups.get(group_name).ok_or_else(|| {
            anyhow!(
                "group '{}' not found. Available groups: {:?}",
                group_name,
                resolved_groups.keys().collect::<Vec<_>>()
            )
        })?;

        let mut state = read_state(&store_root, run_id)?;
        let group_task_set: std::collections::HashSet<_> = group_task_ids.iter().collect();

        let mut running_cancelled = 0;
        let mut pending_cancelled = 0;

        // First pass: terminate Running tasks
        for (tid, task_state) in &state.tasks {
            if !group_task_set.contains(tid) {
                continue;
            }
            if task_state.status == TaskStatus::Running
                && let Some(pid) = task_state.pid
            {
                if let Err(err) = terminate_pid(pid) {
                    eprintln!("Warning: failed to terminate pid {pid} for task {tid}: {err}");
                } else {
                    running_cancelled += 1;
                }
            }
        }

        // Second pass: mark Pending tasks as Canceled
        for tid in group_task_ids {
            if let Some(task_state) = state.tasks.get_mut(tid)
                && task_state.status == TaskStatus::Pending
            {
                task_state.status = TaskStatus::Canceled;
                pending_cancelled += 1;
            }
        }

        // Write updated state if any pending tasks were cancelled
        if pending_cancelled > 0 {
            let store = FsStore::new(&store_root, run_id)?;
            store.write_state(state)?;
        }

        let total = running_cancelled + pending_cancelled;
        if total == 0 {
            println!("No cancellable tasks found in group '{group_name}'");
        } else {
            println!(
                "Cancelled {total} task(s) in group '{group_name}' ({running_cancelled} running, {pending_cancelled} pending)"
            );
        }
        return Ok(0);
    }

    // Handle single task cancel
    if let Some(task_id) = task_id {
        let state = read_state(&store_root, run_id)?;
        if let Some(task_state) = state.tasks.get(&task_id) {
            if let Some(pid) = task_state.pid {
                terminate_pid(pid)?;
                return Ok(0);
            }
            return Err(anyhow!("task {task_id} has no pid to cancel"));
        }
        return Err(anyhow!("task {task_id} not found in run {run_id}"));
    }

    // Cancel entire run
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
                eprintln!("Warning: failed to terminate pid {pid}: {err}");
            }
        }
    }
    Ok(0)
}

enum CleanResult {
    Removed,
    SkippedRunning { reason: String },
}

fn handle_clean(
    global: &GlobalOptions,
    effective: &EffectiveOptions,
    run_id: Option<String>,
    all: bool,
    fix_orphans: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;

    // Handle --fix-orphans mode
    if fix_orphans {
        let fixed = fix_orphaned_runs(&store_root, global.verbose)?;
        if fixed.is_empty() {
            println!("No orphaned runs found");
        } else {
            println!("Fixed {} orphaned run(s):", fixed.len());
            for run_id in &fixed {
                println!("  - {run_id}");
            }
        }
        return Ok(0);
    }

    if let Some(run_id) = run_id {
        if all {
            return Err(anyhow!("--all cannot be used with run_id"));
        }
        clean_run_dir(&store_root, &run_id, true)?;
        return Ok(0);
    }

    if !all {
        return Err(anyhow!("clean requires run_id or --all or --fix-orphans"));
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
        return Err(anyhow!("run {run_id} not found"));
    }

    // Check if run is active
    let active_check = run_active_reason(store_root, run_id);
    match active_check {
        Ok(Some(reason)) => {
            if strict {
                return Err(anyhow!("run {run_id} is still running ({reason})"));
            }
            return Ok(CleanResult::SkippedRunning { reason });
        }
        Ok(None) => {
            // Run is not active, proceed with removal
        }
        Err(err) => {
            if strict {
                return Err(err);
            }
            // In non-strict mode, treat unverifiable runs as potentially active
            eprintln!("Warning: failed to verify run {run_id} status: {err}");
            return Ok(CleanResult::SkippedRunning {
                reason: format!("could not verify status: {err}"),
            });
        }
    }

    // Remove the run directory
    let remove_result =
        fs::remove_dir_all(&run_dir).with_context(|| format!("remove {}", run_dir.display()));
    match remove_result {
        Ok(()) => Ok(CleanResult::Removed),
        Err(err) => {
            if strict {
                Err(err)
            } else {
                eprintln!(
                    "Warning: failed to clean run {} ({}): {err}",
                    run_id,
                    run_dir.display()
                );
                Ok(CleanResult::SkippedRunning {
                    reason: format!("failed to remove directory: {err}"),
                })
            }
        }
    }
}

fn run_active_reason(store_root: &Path, run_id: &str) -> Result<Option<String>> {
    let pid_path = run_pid_path(store_root, run_id);
    if pid_path.exists() {
        let pid = read_pid(&pid_path)?;
        match pid_is_alive(pid) {
            Ok(true) => return Ok(Some(format!("pid {pid}"))),
            Ok(false) => {}
            Err(err) => {
                return Err(anyhow!("cannot verify run {run_id} pid {pid}: {err}"));
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
                    Ok(true) => alive_tasks.push(format!("{task_id} (pid {pid})")),
                    Ok(false) => {}
                    Err(err) => {
                        return Err(anyhow!("cannot verify task {task_id} pid {pid}: {err}"));
                    }
                }
            }
            if !alive_tasks.is_empty() {
                return Ok(Some(format!("running tasks: {}", alive_tasks.join(", "))));
            }
        }
    }

    Ok(None)
}

/// Fix orphaned runs by marking them as failed.
///
/// An orphaned run is one that has status=Running but the parent quedex process
/// is no longer alive. This can happen when quedex crashes or is killed without
/// proper cleanup.
fn fix_orphaned_runs(store_root: &Path, verbose: bool) -> Result<Vec<String>> {
    use quedex::store::{Store, fs::FsStore};

    let mut fixed = Vec::new();
    let states = list_states(store_root)?;

    for state in states {
        // Only process Running runs
        if state.status != RunStatus::Running {
            continue;
        }

        // Check if the run is actually active
        let is_active = match run_active_reason(store_root, &state.run_id) {
            Ok(Some(reason)) => {
                if verbose {
                    eprintln!("[verbose] Run {} is still active: {}", state.run_id, reason);
                }
                true
            }
            Ok(None) => false,
            Err(err) => {
                eprintln!(
                    "Warning: could not verify run {} status: {}",
                    state.run_id, err
                );
                // Skip if we can't verify - don't mark potentially active runs as failed
                continue;
            }
        };

        if is_active {
            continue;
        }

        // This run is orphaned - fix it
        if verbose {
            eprintln!("[verbose] Fixing orphaned run: {}", state.run_id);
        }

        let store = FsStore::new(store_root, &state.run_id)?;
        let mut updated_state = state.clone();
        let now = Utc::now();

        // Update run status
        updated_state.status = RunStatus::Failed;
        updated_state.completed_at = Some(now);

        // Update any Running tasks to Failed
        for (task_id, task_state) in updated_state.tasks.iter_mut() {
            if task_state.status == TaskStatus::Running {
                if verbose {
                    eprintln!(
                        "[verbose]   Marking task {} as failed (was running with pid {:?})",
                        task_id, task_state.pid
                    );
                }
                task_state.status = TaskStatus::Failed;
                task_state.completed_at = Some(now);
                task_state.exit_code = Some(-1);
                task_state.pid = None;
            }
        }

        // Write the updated state
        store.write_state(updated_state)?;

        // Remove the stale run.pid file if it exists
        remove_run_pid(store_root, &state.run_id);

        fixed.push(state.run_id.clone());
    }

    Ok(fixed)
}

fn handle_graph(
    effective: &EffectiveOptions,
    target: &str,
    mermaid: bool,
    ascii: bool,
) -> Result<i32> {
    let store_root = resolve_store_path(effective.store.as_ref())?;
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

/// Handle squash tasks after plan execution
fn handle_squash_tasks(plan: &Plan, report: &ScheduleReport) -> Result<()> {
    // Find a squash task
    let squash_task = plan.tasks.iter().find(|t| t.squash);

    if squash_task.is_none() {
        return Ok(());
    }

    if report.commit_hashes.is_empty() {
        return Ok(());
    }

    // Try to open git manager
    let git_manager = match GitManager::open() {
        Ok(gm) => gm,
        Err(e) => {
            eprintln!("Warning: Could not open git repository for squash: {}", e);
            return Ok(());
        }
    };

    // Ensure on a branch
    if let Err(e) = git_manager.ensure_on_branch() {
        eprintln!("Warning: Not on a branch, skipping squash: {}", e);
        return Ok(());
    }

    let task_title = squash_task
        .unwrap()
        .title
        .as_deref()
        .unwrap_or("Integration");

    // Collect commit information for squash message
    let task_summaries: Vec<(String, String)> = report
        .commit_hashes
        .iter()
        .map(|(id, _, title)| {
            let title_str = title.clone().unwrap_or_else(|| id.clone());
            (id.clone(), title_str)
        })
        .collect();

    if task_summaries.is_empty() {
        return Ok(());
    }

    // Generate squash message
    let squash_message = git::generate_squash_message(&task_summaries, task_title);

    // Squash commits
    let count = report.commit_hashes.len();
    match git_manager.squash_commits(count, &squash_message) {
        Ok(new_commit_id) => {
            println!("Squashed {} commits into: {}", count, new_commit_id);
        }
        Err(e) => {
            eprintln!("Warning: Failed to squash commits: {}", e);
        }
    }

    Ok(())
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
    let contents =
        fs::read_to_string(&path).with_context(|| format!("read plan file {}", path.display()))?;
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
    // cwd must be absolute (validated in Plan::validate), or defaults to base_dir
    let cwd = plan.run.cwd.clone().unwrap_or(base_dir);
    Ok(cwd)
}

fn check_codex_available() -> Result<()> {
    let codex_path = resolve_command_path("codex")?;
    let status = std::process::Command::new(codex_path)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(_) => Ok(()),
        Err(err) => Err(anyhow!(err).context("codex not found")),
    }
}

fn check_claude_code_available() -> Result<()> {
    let claude_path = resolve_command_path("claude")?;
    let status = std::process::Command::new(claude_path)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(_) => Ok(()),
        Err(err) => Err(anyhow!(err).context("claude not found")),
    }
}

fn check_opencode_available() -> Result<()> {
    let opencode_path = resolve_command_path("opencode")?;
    let status = std::process::Command::new(opencode_path)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(_) => Ok(()),
        Err(err) => Err(anyhow!(err).context("opencode not found")),
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
            Err(err) => eprintln!("skip run {run_id}: {err}"),
        }
    }
    Ok(states)
}

struct RunInfo {
    state: State,
    is_active: bool,
    active_reason: Option<String>,
}

fn list_runs_with_activity(store_root: &Path) -> Result<Vec<RunInfo>> {
    let states = list_states(store_root)?;
    let mut runs: Vec<RunInfo> = states
        .into_iter()
        .map(|state| {
            let active_reason = run_active_reason(store_root, &state.run_id).ok().flatten();
            let is_active = active_reason.is_some();
            RunInfo {
                state,
                is_active,
                active_reason,
            }
        })
        .collect();
    // Sort: active runs first (by started_at desc), then inactive runs (by started_at desc)
    runs.sort_by(|a, b| match (a.is_active, b.is_active) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => b.state.started_at.cmp(&a.state.started_at),
    });
    Ok(runs)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        ".".repeat(max_len)
    } else {
        let truncated: String = s.chars().take(max_len - 3).collect();
        format!("{}...", truncated)
    }
}

fn print_run_selection_ui(runs: &[RunInfo], selected: usize) {
    // Clear screen and move cursor to top
    print!("\x1B[2J\x1B[H");
    println!("Select a run to monitor:\n");

    let active_runs: Vec<_> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_active)
        .collect();
    let inactive_runs: Vec<_> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_active)
        .collect();

    if !active_runs.is_empty() {
        println!("Running:");
        for (idx, run) in &active_runs {
            let marker = if *idx == selected { ">" } else { " " };
            let reason = run.active_reason.as_deref().unwrap_or("");
            let short_id: String = run.state.run_id.chars().take(8).collect();
            let status_str = format!("{:?}", run.state.status);
            let name = truncate_str(&run.state.run_name, 25);
            let reason_display = truncate_str(reason, 20);
            println!(
                "{} [{:<10}] {:<8} {:<25} ({})",
                marker, status_str, short_id, name, reason_display
            );
        }
        println!();
    }

    if !inactive_runs.is_empty() {
        println!("Recent:");
        for (idx, run) in &inactive_runs {
            let marker = if *idx == selected { ">" } else { " " };
            let short_id: String = run.state.run_id.chars().take(8).collect();
            let status_str = format!("{:?}", run.state.status);
            let name = truncate_str(&run.state.run_name, 25);
            println!(
                "{} [{:<10}] {:<8} {:<25}",
                marker, status_str, short_id, name
            );
        }
        println!();
    }

    println!("[↑↓] Select  [Enter] Open TUI  [q] Cancel");
}

fn select_run_interactive(runs: &[RunInfo]) -> Result<Option<String>> {
    if runs.is_empty() {
        return Ok(None);
    }

    enable_raw_mode().context("enable raw mode")?;
    let result = (|| {
        let mut selected = 0usize;

        loop {
            print_run_selection_ui(runs, selected);
            io::stdout().flush()?;

            #[allow(clippy::collapsible_if)]
            if event::poll(Duration::from_millis(100))? {
                if let CrosstermEvent::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            selected = selected.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') if selected < runs.len() - 1 => {
                            selected += 1;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {}
                        KeyCode::Enter => {
                            return Ok(Some(runs[selected].state.run_id.clone()));
                        }
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(None);
                        }
                        _ => {}
                    }
                }
            }
        }
    })();

    disable_raw_mode().context("disable raw mode")?;
    // Clear screen after exiting selection
    print!("\x1B[2J\x1B[H");
    io::stdout().flush()?;
    result
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
    println!(
        "{:<36} {:<10} {:<20} name",
        "run_id", "status", "started_at"
    );
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
        println!("completed_at: {completed_at}");
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
    stdout.write_all(&buffer).context("write log output")?;
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
            eprintln!("Warning: SIGTERM failed for pid {pid}, trying SIGKILL");
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
            eprintln!("Warning: pid {pid} still running after SIGTERM, escalating to SIGKILL");
            let kill_status = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .status()
                .context("spawn kill -KILL command")?;

            if !kill_status.success() {
                return Err(anyhow!("failed to kill pid {pid} even with SIGKILL"));
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
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
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
    let mut has_skipped_not_condition = false;
    let mut has_canceled = false;

    for task in state.tasks.values() {
        match task.status {
            TaskStatus::Failed => has_failed = true,
            TaskStatus::Skipped
                if !matches!(task.skip_reason, Some(SkipReason::ConditionNotMet { .. })) =>
            {
                // Condition-based skips are intentional and not failures
                has_skipped_not_condition = true;
            }
            TaskStatus::Skipped => {}
            TaskStatus::Canceled => has_canceled = true,
            _ => {}
        }
    }

    if has_failed || has_skipped_not_condition {
        (RunStatus::Failed, 1)
    } else if has_canceled {
        (RunStatus::Canceled, 2)
    } else {
        (RunStatus::Completed, 0)
    }
}

fn print_mermaid_graph(plan: &Plan) {
    println!("graph TD");

    // Resolve groups: merge Plan.groups and Task.group
    let groups = plan.resolve_groups();

    // Build set of grouped task IDs
    let grouped_task_ids: std::collections::HashSet<&str> = groups
        .values()
        .flat_map(|ids| ids.iter().map(String::as_str))
        .collect();

    // Output subgraphs for each group
    let mut group_names: Vec<_> = groups.keys().collect();
    group_names.sort(); // Deterministic output
    for group_name in &group_names {
        if let Some(task_ids) = groups.get(*group_name) {
            // Sanitize group name for Mermaid ID (replace non-alphanumeric with underscore)
            let sanitized_id: String = group_name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            println!("  subgraph {sanitized_id} [{group_name}]");
            let mut sorted_ids: Vec<_> = task_ids.iter().collect();
            sorted_ids.sort();
            for task_id in sorted_ids {
                println!("    {task_id}");
            }
            println!("  end");
        }
    }

    // Output ungrouped tasks (tasks with no group)
    for task in &plan.tasks {
        if !grouped_task_ids.contains(task.id.as_str()) && task.deps.is_empty() {
            println!("  {};", task.id);
        }
    }

    // Output all dependency edges
    for task in &plan.tasks {
        for dep in &task.deps {
            println!("  {} --> {};", dep, task.id);
        }
    }
}

fn print_ascii_graph(plan: &Plan) {
    // Resolve groups: merge Plan.groups and Task.group
    let groups = plan.resolve_groups();

    // Build task_id -> group_name mapping
    let mut task_to_group: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for (group_name, task_ids) in &groups {
        for task_id in task_ids {
            task_to_group.insert(task_id.as_str(), group_name.as_str());
        }
    }

    // Collect ungrouped tasks
    let ungrouped: Vec<&Task> = plan
        .tasks
        .iter()
        .filter(|t| !task_to_group.contains_key(t.id.as_str()))
        .collect();

    // Sort group names for deterministic output
    let mut group_names: Vec<_> = groups.keys().collect();
    group_names.sort();

    let mut first_section = true;

    // Output each group
    for group_name in &group_names {
        if let Some(task_ids) = groups.get(*group_name) {
            if !first_section {
                println!();
            }
            first_section = false;

            println!("=== {group_name} ===");
            let mut sorted_ids: Vec<_> = task_ids.iter().collect();
            sorted_ids.sort();
            for task_id in sorted_ids {
                if let Some(task) = plan.tasks.iter().find(|t| t.id == *task_id) {
                    if task.deps.is_empty() {
                        println!("  {}", task.id);
                    }
                    for dep in &task.deps {
                        println!("  {} -> {}", dep, task.id);
                    }
                }
            }
        }
    }

    // Output ungrouped tasks
    if !ungrouped.is_empty() {
        if !first_section {
            println!();
        }
        println!("=== (ungrouped) ===");
        for task in &ungrouped {
            if task.deps.is_empty() {
                println!("  {}", task.id);
            }
            for dep in &task.deps {
                println!("  {} -> {}", dep, task.id);
            }
        }
    } else if groups.is_empty() {
        // No groups at all - output simple format
        for task in &plan.tasks {
            if task.deps.is_empty() {
                println!("{}", task.id);
            }
            for dep in &task.deps {
                println!("{} -> {}", dep, task.id);
            }
        }
    }
}

fn spawn_cancel_listener(cancel: CancelHandle) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
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
                    task_state.skip_reason = record.skip_reason.clone();
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
                    skip_reason: task_state.skip_reason.clone(),
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
                eprintln!(
                    "Error: state lock poisoned - returning potentially corrupted state (data may be inconsistent)"
                );
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
                    eprintln!(
                        "Error: state lock poisoned - updating potentially corrupted state (data may be inconsistent)"
                    );
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

    fn task_finished(
        &self,
        task_id: &str,
        status: TaskStatus,
        exit_code: Option<i32>,
    ) -> Result<()> {
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

    fn task_outputs_saved(&self, task_id: &str, outputs: Vec<String>) -> Result<()> {
        self.update(|state| {
            if let Some(task) = state.tasks.get_mut(task_id) {
                task.output_files = Some(outputs);
            }
        })
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
/// when a termination signal is received (SIGTERM/SIGINT). Tasks register
/// their child handles here so they can be killed on cancellation.
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
                eprintln!(
                    "Error: active lock poisoned - cannot safely kill child processes (some processes may not be terminated)"
                );
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
                eprintln!(
                    "Error: active lock poisoned - cannot register task {task_id} (cancellation may not work correctly)"
                );
            }
        }
    }

    fn unregister(&self, task_id: &str) {
        match self.active.lock() {
            Ok(mut guard) => {
                guard.remove(task_id);
            }
            Err(_) => {
                eprintln!(
                    "Error: active lock poisoned - cannot unregister task {task_id} (may cause issues with future cancellations)"
                );
            }
        }
    }
}

struct PlanTaskRunner {
    tasks: Arc<HashMap<String, Task>>,
    profiles: Arc<HashMap<String, quedex::plan::AgentProfile>>,
    ctx: RunContext,
    state: StateHandle,
    cancel: CancelHandle,
    codex: CodexRunner,
    claude_code: ClaudeCodeRunner,
    opencode: OpencodeRunner,
    notifier: Option<Notifier>,
}

impl PlanTaskRunner {
    fn new(
        tasks: Arc<HashMap<String, Task>>,
        profiles: Arc<HashMap<String, quedex::plan::AgentProfile>>,
        ctx: RunContext,
        state: StateHandle,
        cancel: CancelHandle,
        notifier: Option<Notifier>,
    ) -> Self {
        Self {
            tasks,
            profiles,
            ctx,
            state,
            cancel,
            codex: CodexRunner::new(),
            claude_code: ClaudeCodeRunner::new(),
            opencode: OpencodeRunner::new(),
            notifier,
        }
    }
}

impl TaskRunner for PlanTaskRunner {
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = TaskResult> + Send>>;

    fn spawn(&self, task_spec: TaskSpec) -> Self::Future {
        let tasks = Arc::clone(&self.tasks);
        let profiles = Arc::clone(&self.profiles);
        let ctx = self.ctx.clone();
        let state = self.state.clone();
        let cancel = self.cancel.clone();
        let codex = self.codex;
        let claude_code = self.claude_code;
        let opencode = self.opencode;
        let notifier = self.notifier.clone();

        Box::pin(async move {
            let Some(task) = tasks.get(&task_spec.id) else {
                return TaskResult::failed(1);
            };

            // Resolve agent profile for this task
            let profile = task.profile.as_ref().and_then(|name| profiles.get(name));

            // Clone task for potential profile-based model override
            let mut task = task.clone();
            if let Some(profile) = profile {
                // Profile model override: only if task runner config doesn't specify a model
                if let Some(ref profile_model) = profile.model {
                    if let Some(ref mut cc) = task.claude_code
                        && cc.model.is_none()
                    {
                        cc.model = Some(profile_model.clone());
                    }
                    if let Some(ref mut oc) = task.opencode
                        && oc.model.is_none()
                    {
                        oc.model = Some(profile_model.clone());
                    }
                }
            }

            let task_id = task.id.clone();
            let output_files = task_spec.output_files.clone();
            let retry_count = task.retry_count;
            let retry_delay_sec = task.retry_delay_sec;

            if cancel.is_canceled() {
                let _ = state.task_finished(&task_id, TaskStatus::Canceled, None);
                return TaskResult::canceled();
            }

            let workdir = if let Some(manager) = &ctx.worktree_manager {
                match manager.acquire(&task_id, task.no_worktree) {
                    Ok(workdir) => workdir,
                    Err(err) => {
                        eprintln!("task {task_id} worktree acquire error: {err:#}");
                        let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                        return TaskResult::failed(1);
                    }
                }
            } else {
                TaskWorkdir::Shared(ctx.cwd.clone())
            };

            let mut task_ctx = ctx.clone();
            task_ctx.cwd = workdir.path().to_path_buf();

            // Apply profile system_prompt override
            // Priority: task runner config (N/A for system_prompt) > profile > run.system_prompt > quedex.toml
            if let Some(profile) = profile
                && let Some(ref profile_prompt) = profile.system_prompt
            {
                task_ctx.system_prompt = Some(profile_prompt.clone());
            }

            // Inject shared context from upstream tasks into prompt
            if let Some(ref context_config) = task.context
                && let Some(ref injections) = context_config.inject
            {
                let mut context_prefix = String::new();
                for inject in injections {
                    match task_ctx.store.get_context(&inject.from) {
                        Ok(content) => {
                            let label = inject.r#as.as_deref().unwrap_or(&inject.from);
                            let text = String::from_utf8_lossy(&content);
                            context_prefix.push_str(&format!(
                                "--- {} ---\n{}\n--- end {} ---\n\n",
                                label,
                                text.trim(),
                                label
                            ));
                        }
                        Err(err) => {
                            eprintln!(
                                "task {task_id} context inject warning: failed to load context '{}': {err}",
                                inject.from
                            );
                        }
                    }
                }
                if !context_prefix.is_empty() {
                    // Prepend injected context to the task's prompt
                    if let Some(ref mut cc) = task.claude_code {
                        cc.prompt = format!("{}{}", context_prefix, cc.prompt);
                    } else if let Some(ref mut cx) = task.codex {
                        cx.prompt = format!("{}{}", context_prefix, cx.prompt);
                    } else if let Some(ref mut oc) = task.opencode {
                        oc.prompt = format!("{}{}", context_prefix, oc.prompt);
                    }
                }
            }

            let mut attempt = 0u32;
            let max_attempts = retry_count + 1; // Original attempt + retries
            let mut executed = false;
            let retry_strategy = task.retry_strategy.clone();

            // Save original prompts before retry loop to prevent accumulation
            let original_prompt_cc = task.claude_code.as_ref().map(|c| c.prompt.clone());
            let original_prompt_cx = task.codex.as_ref().map(|c| c.prompt.clone());
            let original_prompt_oc = task.opencode.as_ref().map(|c| c.prompt.clone());
            // Save original models for potential escalation reset
            let original_model_cc = task.claude_code.as_ref().and_then(|c| c.model.clone());
            let original_model_oc = task.opencode.as_ref().and_then(|c| c.model.clone());

            let result = loop {
                attempt += 1;

                if cancel.is_canceled() {
                    let _ = state.task_finished(&task_id, TaskStatus::Canceled, None);
                    break TaskResult::canceled();
                }

                if attempt > 1 {
                    // Calculate delay with backoff and jitter if strategy is configured
                    let delay_sec = if let Some(ref strategy) = retry_strategy {
                        strategy.calculate_delay(retry_delay_sec, attempt - 1)
                    } else {
                        retry_delay_sec
                    };

                    eprintln!(
                        "task {task_id} retry attempt {}/{} after failure (delay: {}s)",
                        attempt - 1,
                        retry_count,
                        delay_sec
                    );
                    if delay_sec > 0 {
                        tokio::time::sleep(Duration::from_secs(delay_sec)).await;
                    }

                    // Restore original prompts and models to prevent accumulation
                    if let (Some(cc), Some(orig)) = (&mut task.claude_code, &original_prompt_cc) {
                        cc.prompt = orig.clone();
                        cc.model = original_model_cc.clone();
                    }
                    if let (Some(cx), Some(orig)) = (&mut task.codex, &original_prompt_cx) {
                        cx.prompt = orig.clone();
                    }
                    if let (Some(oc), Some(orig)) = (&mut task.opencode, &original_prompt_oc) {
                        oc.prompt = orig.clone();
                        oc.model = original_model_oc.clone();
                    }

                    // Adaptive retry: inject error context and escalate model
                    if let Some(ref strategy) = retry_strategy {
                        // Inject stderr from previous attempt into prompt
                        if strategy.inject_error_context {
                            let stderr_path = task_ctx.store.log_path(&task_id, LogStream::Stderr);
                            if let Ok(stderr_content) = std::fs::read_to_string(&stderr_path) {
                                let max_lines = strategy.max_stderr_lines;
                                let lines: Vec<&str> = stderr_content.lines().collect();
                                let tail: Vec<&str> = if lines.len() > max_lines {
                                    lines[lines.len() - max_lines..].to_vec()
                                } else {
                                    lines
                                };
                                if !tail.is_empty() {
                                    let error_context = tail.join("\n");
                                    let prefix = format!(
                                        "[RETRY CONTEXT] The previous attempt failed with the following error output:\n```\n{}\n```\nPlease fix the issues and try again.\n\n",
                                        error_context
                                    );
                                    // Inject error context into runner prompt
                                    if let Some(ref mut cc) = task.claude_code {
                                        cc.prompt = format!("{}{}", prefix, cc.prompt);
                                    } else if let Some(ref mut cx) = task.codex {
                                        cx.prompt = format!("{}{}", prefix, cx.prompt);
                                    } else if let Some(ref mut oc) = task.opencode {
                                        oc.prompt = format!("{}{}", prefix, oc.prompt);
                                    }
                                }
                            }
                        }

                        // Escalate model on retry
                        if let Some(ref escalate_model) = strategy.escalate_model {
                            if let Some(ref mut cc) = task.claude_code {
                                cc.model = Some(escalate_model.clone());
                            } else if let Some(ref mut oc) = task.opencode {
                                oc.model = Some(escalate_model.clone());
                            }
                        }
                    }
                }

                // Select runner and spawn child process
                let child = if task.codex.is_some() {
                    codex.spawn(&task, &task_ctx)
                } else if task.claude_code.is_some() {
                    claude_code.spawn(&task, &task_ctx)
                } else {
                    opencode.spawn(&task, &task_ctx)
                };
                let child = match child {
                    Ok(child) => {
                        executed = true;
                        child
                    }
                    Err(err) => {
                        eprintln!("task {task_id} spawn error: {err:#}");
                        if attempt >= max_attempts {
                            let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                            break TaskResult::failed(1);
                        }
                        continue;
                    }
                };

                cancel.register(&task_id, child.clone());
                let _ = state.task_started(&task_id, child.pid);

                let wait_future = tokio::task::spawn_blocking(move || child.wait());
                let wait_result = wait_future.await;

                cancel.unregister(&task_id);

                let status = match wait_result {
                    Ok(Ok(status)) => status,
                    Ok(Err(err)) => {
                        eprintln!("task {task_id} wait error: {err}");
                        if attempt >= max_attempts {
                            let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                            break TaskResult::failed(1);
                        }
                        continue;
                    }
                    Err(err) => {
                        eprintln!("task {task_id} join error: {err}");
                        if attempt >= max_attempts {
                            let _ = state.task_finished(&task_id, TaskStatus::Failed, Some(1));
                            break TaskResult::failed(1);
                        }
                        continue;
                    }
                };

                let result = map_exit_status(status, cancel.is_canceled());

                // If failed but have retries remaining, check if we should retry
                if result.status == TaskStatus::Failed && attempt < max_attempts {
                    // Check if this is a permanent failure that should not be retried
                    if let Some(ref strategy) = retry_strategy
                        && strategy.skip_permanent_failures
                    {
                        let stderr_path = task_ctx.store.log_path(&task_id, LogStream::Stderr);
                        let stderr_tail = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                        let failure_type =
                            quedex::plan::FailureType::classify(result.exit_code, &stderr_tail);
                        if !failure_type.should_retry() {
                            eprintln!(
                                "task {task_id} failed with permanent failure (exit code {:?}), skipping retry",
                                result.exit_code
                            );
                            let _ = state.task_finished(&task_id, result.status, result.exit_code);
                            break result;
                        }
                    }

                    eprintln!(
                        "task {task_id} failed with exit code {:?}, will retry",
                        result.exit_code
                    );
                    continue;
                }

                let _ = state.task_finished(&task_id, result.status, result.exit_code);
                break result;
            };

            if executed
                && result.status != TaskStatus::Canceled
                && let Some(output_files) = output_files.as_ref()
            {
                let mut saved_outputs = Vec::new();
                for output_file in output_files {
                    let output_path = Path::new(output_file);
                    let source_path = if output_path.is_absolute() {
                        output_path.to_path_buf()
                    } else {
                        task_ctx.cwd.join(output_path)
                    };

                    if !source_path.exists() {
                        eprintln!(
                            "task {task_id} output file missing: {}",
                            source_path.display()
                        );
                        continue;
                    }

                    let metadata = match fs::metadata(&source_path) {
                        Ok(metadata) => metadata,
                        Err(err) => {
                            eprintln!(
                                "task {task_id} output file stat error for {}: {err}",
                                source_path.display()
                            );
                            continue;
                        }
                    };

                    if !metadata.is_file() {
                        eprintln!(
                            "task {task_id} output path is not a file: {}",
                            source_path.display()
                        );
                        continue;
                    }

                    let content = match fs::read(&source_path) {
                        Ok(content) => content,
                        Err(err) => {
                            eprintln!(
                                "task {task_id} output file read error for {}: {err}",
                                source_path.display()
                            );
                            continue;
                        }
                    };

                    match task_ctx.store.save_output(&task_id, output_file, &content) {
                        Ok(_) => saved_outputs.push(output_file.clone()),
                        Err(err) => {
                            eprintln!("task {task_id} output save error for {output_file}: {err:#}");
                        }
                    }
                }

                if !saved_outputs.is_empty()
                    && let Err(err) = state.task_outputs_saved(&task_id, saved_outputs)
                {
                    eprintln!("task {task_id} output state update error: {err:#}");
                }
            }

            // Publish shared context after successful completion
            if result.status == TaskStatus::Succeeded
                && let Some(ref context_config) = task.context
                && let Some(ref publish) = context_config.publish
            {
                let source_path = task_ctx.cwd.join(&publish.source);
                match std::fs::read(&source_path) {
                    Ok(content) => {
                        if let Err(err) = task_ctx.store.save_context(&publish.key, &content) {
                            eprintln!(
                                "task {task_id} context publish error for key '{}': {err:#}",
                                publish.key
                            );
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "task {task_id} context publish warning: failed to read source '{}': {err}",
                            source_path.display()
                        );
                    }
                }
            }

            if let Some(manager) = &ctx.worktree_manager {
                if result.status == TaskStatus::Succeeded {
                    if let Err(err) = manager.release_success(&task_id, workdir) {
                        eprintln!("task {task_id} worktree release error: {err:#}");
                    }
                } else {
                    manager.release_failure(&task_id, workdir);
                }
            }

            // Send task completion notification
            if let Some(ref notifier) = notifier {
                notifier.notify_task_complete(&task_id, result.status, result.exit_code);
            }

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

async fn handle_serve(
    global: &GlobalOptions,
    run_id: Option<String>,
    host: String,
    port: u16,
    config: &Config,
) -> Result<i32> {
    let store_root = global
        .store
        .clone()
        .or_else(|| config.store.clone())
        .unwrap_or_else(|| PathBuf::from(".quedex"));

    quedex::web::serve(quedex::web::ServerConfig {
        host,
        port,
        store_root,
        run_id,
    })
    .await?;

    Ok(0)
}

/// Check if a downstream task has unsatisfied dependencies outside the retry set.
/// Returns true if the task should be excluded from retry.
fn has_unsatisfied_external_deps(
    task_deps: &[String],
    retry_target_ids: &HashSet<String>,
    task_statuses: &HashMap<String, TaskStatus>,
) -> bool {
    task_deps.iter().any(|dep| {
        // Dependencies in retry set → OK (will be retried)
        if retry_target_ids.contains(dep) {
            return false;
        }
        // Dependencies outside retry set → must be Succeeded
        task_statuses
            .get(dep)
            .is_none_or(|status| *status != TaskStatus::Succeeded)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_has_unsatisfied_external_deps_all_in_retry_set() {
        // All dependencies are in the retry set → should NOT be excluded
        let task_deps = vec!["A".to_string(), "B".to_string()];
        let retry_target_ids: HashSet<String> = ["A", "B"].iter().map(|s| s.to_string()).collect();
        let task_statuses = HashMap::new();

        assert!(!has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_external_succeeded() {
        // External dependency is Succeeded → should NOT be excluded
        let task_deps = vec!["A".to_string(), "B".to_string()];
        let retry_target_ids: HashSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        let mut task_statuses = HashMap::new();
        task_statuses.insert("B".to_string(), TaskStatus::Succeeded);

        assert!(!has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_external_failed() {
        // External dependency is Failed → SHOULD be excluded
        let task_deps = vec!["A".to_string(), "B".to_string()];
        let retry_target_ids: HashSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        let mut task_statuses = HashMap::new();
        task_statuses.insert("B".to_string(), TaskStatus::Failed);

        assert!(has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_external_skipped() {
        // External dependency is Skipped → SHOULD be excluded
        let task_deps = vec!["A".to_string(), "B".to_string()];
        let retry_target_ids: HashSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        let mut task_statuses = HashMap::new();
        task_statuses.insert("B".to_string(), TaskStatus::Skipped);

        assert!(has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_mixed() {
        // Mixed: one Succeeded, one Failed outside retry set → SHOULD be excluded
        let task_deps = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let retry_target_ids: HashSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        let mut task_statuses = HashMap::new();
        task_statuses.insert("B".to_string(), TaskStatus::Succeeded);
        task_statuses.insert("C".to_string(), TaskStatus::Failed);

        assert!(has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_no_deps() {
        // No dependencies → should NOT be excluded
        let task_deps: Vec<String> = vec![];
        let retry_target_ids: HashSet<String> = HashSet::new();
        let task_statuses = HashMap::new();

        assert!(!has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }

    #[test]
    fn test_has_unsatisfied_external_deps_missing_status() {
        // External dependency has no status record → SHOULD be excluded (conservative)
        let task_deps = vec!["A".to_string(), "B".to_string()];
        let retry_target_ids: HashSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        let task_statuses = HashMap::new(); // B has no status

        assert!(has_unsatisfied_external_deps(
            &task_deps,
            &retry_target_ids,
            &task_statuses
        ));
    }
}
