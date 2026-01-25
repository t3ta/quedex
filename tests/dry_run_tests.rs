//! Integration tests for dry-run functionality

use quedex::dry_run::{detect_lock_conflicts, generate_execution_waves};
use quedex::plan::{CodexConfig, Plan, RunConfig, Task, TaskMode};
use std::collections::HashMap;

fn create_test_plan(tasks: Vec<Task>) -> Plan {
    Plan {
        version: 1,
        run: RunConfig {
            name: Some("test-plan".to_string()),
            cwd: None,
            worktree: None,
            env: HashMap::new(),
            max_concurrency: None,
            fail_fast: None,
            default_timeout_sec: None,
            notifications: None,
        },
        groups: HashMap::new(),
        tasks,
    }
}

fn create_task(id: &str, deps: Vec<&str>, locks: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        group: None,
        deps: deps.into_iter().map(|s| s.to_string()).collect(),
        locks: locks.into_iter().map(|s| s.to_string()).collect(),
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: Some(CodexConfig {
            prompt: "test prompt".to_string(),
            output_last_message: None,
            verify_after: false,
            sandbox: None,
            ask_for_approval: None,
            json: false,
        }),
        claude_code: None,
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
            auto_commit: true,
        squash: false,
}
}

#[test]
fn waves_with_diamond_dependency() {
    // Diamond pattern: A -> B, A -> C, B -> D, C -> D
    let tasks = vec![
        create_task("A", vec![], vec![]),
        create_task("B", vec!["A"], vec![]),
        create_task("C", vec!["A"], vec![]),
        create_task("D", vec!["B", "C"], vec![]),
    ];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 4);

    assert_eq!(waves.len(), 3);
    assert_eq!(waves[0], vec!["A"]);
    assert_eq!(waves[1], vec!["B", "C"]); // B and C can run in parallel
    assert_eq!(waves[2], vec!["D"]);
}

#[test]
fn waves_respect_max_concurrency_with_diamond() {
    // Same diamond pattern but with max_concurrency=1
    let tasks = vec![
        create_task("A", vec![], vec![]),
        create_task("B", vec!["A"], vec![]),
        create_task("C", vec!["A"], vec![]),
        create_task("D", vec!["B", "C"], vec![]),
    ];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 1);

    // With max_concurrency=1, B and C must be in separate waves
    assert_eq!(waves.len(), 4);
    assert_eq!(waves[0], vec!["A"]);
    // B comes first alphabetically
    assert_eq!(waves[1], vec!["B"]);
    assert_eq!(waves[2], vec!["C"]);
    assert_eq!(waves[3], vec!["D"]);
}

#[test]
fn lock_conflict_with_multiple_locks() {
    // A has locks [db, cache], B has lock [db], C has lock [cache]
    // All three compete but have no dependencies
    let tasks = vec![
        create_task("A", vec![], vec!["db", "cache"]),
        create_task("B", vec![], vec!["db"]),
        create_task("C", vec![], vec!["cache"]),
    ];
    let plan = create_test_plan(tasks);

    let conflicts = detect_lock_conflicts(&plan);

    // Should detect conflicts for both 'db' and 'cache'
    assert_eq!(conflicts.len(), 2);

    let db_conflict = conflicts.iter().find(|c| c.lock_name == "db").unwrap();
    assert!(db_conflict.task_ids.contains(&"A".to_string()));
    assert!(db_conflict.task_ids.contains(&"B".to_string()));

    let cache_conflict = conflicts.iter().find(|c| c.lock_name == "cache").unwrap();
    assert!(cache_conflict.task_ids.contains(&"A".to_string()));
    assert!(cache_conflict.task_ids.contains(&"C".to_string()));
}

#[test]
fn waves_handle_multiple_independent_chains() {
    // Two independent chains: A1 -> B1 -> C1, A2 -> B2 -> C2
    let tasks = vec![
        create_task("A1", vec![], vec![]),
        create_task("A2", vec![], vec![]),
        create_task("B1", vec!["A1"], vec![]),
        create_task("B2", vec!["A2"], vec![]),
        create_task("C1", vec!["B1"], vec![]),
        create_task("C2", vec!["B2"], vec![]),
    ];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 4);

    assert_eq!(waves.len(), 3);
    assert_eq!(waves[0], vec!["A1", "A2"]);
    assert_eq!(waves[1], vec!["B1", "B2"]);
    assert_eq!(waves[2], vec!["C1", "C2"]);
}

#[test]
fn waves_with_shared_lock_serialize_execution() {
    // A1, A2, A3 all independent but share lock "db"
    let tasks = vec![
        create_task("A1", vec![], vec!["db"]),
        create_task("A2", vec![], vec!["db"]),
        create_task("A3", vec![], vec!["db"]),
    ];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 4);

    // Even with max_concurrency=4, they should be serialized due to lock
    assert_eq!(waves.len(), 3);
    assert_eq!(waves[0], vec!["A1"]);
    assert_eq!(waves[1], vec!["A2"]);
    assert_eq!(waves[2], vec!["A3"]);
}

#[test]
fn empty_plan_produces_no_waves() {
    let plan = create_test_plan(vec![]);
    let waves = generate_execution_waves(&plan, 4);
    assert!(waves.is_empty());
}

#[test]
fn empty_plan_produces_no_conflicts() {
    let plan = create_test_plan(vec![]);
    let conflicts = detect_lock_conflicts(&plan);
    assert!(conflicts.is_empty());
}

#[test]
fn single_task_produces_single_wave() {
    let tasks = vec![create_task("A", vec![], vec![])];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 4);

    assert_eq!(waves.len(), 1);
    assert_eq!(waves[0], vec!["A"]);
}

#[test]
fn partial_lock_overlap_allows_some_parallelism() {
    // A has lock [x], B has lock [y], C has locks [x, y]
    // A and B can run together, C must wait
    let tasks = vec![
        create_task("A", vec![], vec!["x"]),
        create_task("B", vec![], vec!["y"]),
        create_task("C", vec![], vec!["x", "y"]),
    ];
    let plan = create_test_plan(tasks);

    let waves = generate_execution_waves(&plan, 4);

    assert_eq!(waves.len(), 2);
    assert_eq!(waves[0], vec!["A", "B"]); // A and B have no lock overlap
    assert_eq!(waves[1], vec!["C"]); // C conflicts with both
}
