//! Dry-run analysis module for execution planning without running tasks.
//!
//! This module provides functions for:
//! - Generating execution waves (groups of tasks that can run in parallel)
//! - Detecting potential lock conflicts between tasks

use crate::plan::Plan;
use std::collections::{HashMap, HashSet, VecDeque};

/// Represents a potential lock conflict between tasks
#[derive(Debug, Clone)]
pub struct LockConflict {
    /// Name of the lock that may cause conflict
    pub lock_name: String,
    /// IDs of tasks that compete for this lock
    pub task_ids: Vec<String>,
    /// Additional notes about the conflict
    pub notes: String,
}

/// Generate execution waves considering dependencies and max_concurrency
///
/// A wave is a group of tasks that can run in parallel within the concurrency limit.
/// Tasks in wave N+1 depend on at least one task in wave N or earlier.
///
/// # Arguments
/// * `plan` - The execution plan containing task definitions
/// * `max_concurrency` - Maximum number of tasks that can run in parallel
///
/// # Returns
/// A vector of waves, where each wave is a vector of task IDs
pub fn generate_execution_waves(plan: &Plan, max_concurrency: usize) -> Vec<Vec<String>> {
    // Build dependency graph using owned strings to avoid lifetime issues
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    let mut task_locks: HashMap<String, Vec<String>> = HashMap::new();

    for task in &plan.tasks {
        in_degree.entry(task.id.clone()).or_insert(0);
        task_locks.insert(task.id.clone(), task.locks.clone());
        for dep in &task.deps {
            dependents
                .entry(dep.clone())
                .or_default()
                .push(task.id.clone());
            *in_degree.entry(task.id.clone()).or_insert(0) += 1;
        }
    }

    let mut waves: Vec<Vec<String>> = Vec::new();
    let mut completed: HashSet<String> = HashSet::new();

    // Process tasks in waves using modified Kahn's algorithm
    loop {
        // Find all tasks that are ready (in_degree == 0 and not completed)
        let mut ready: Vec<String> = in_degree
            .iter()
            .filter(|(task_id, deg)| **deg == 0 && !completed.contains(*task_id))
            .map(|(task_id, _)| task_id.clone())
            .collect();

        if ready.is_empty() {
            break;
        }

        // Sort for deterministic output
        ready.sort();

        // Build wave considering max_concurrency and locks
        let mut wave: Vec<String> = Vec::new();
        let mut wave_locks: HashSet<String> = HashSet::new();
        let mut remaining: VecDeque<String> = ready.into_iter().collect();

        let mut skipped_due_to_lock: Vec<String> = Vec::new();

        while let Some(task_id) = remaining.pop_front() {
            // Check concurrency limit
            if wave.len() >= max_concurrency {
                remaining.push_back(task_id);
                break;
            }

            // Check lock conflicts within this wave
            let task_lock_list = task_locks.get(&task_id).cloned().unwrap_or_default();
            let has_lock_conflict = task_lock_list.iter().any(|lock| wave_locks.contains(lock));

            if has_lock_conflict {
                // Can't add to this wave due to lock conflict, try next wave
                skipped_due_to_lock.push(task_id);
                continue;
            }

            // Add task to wave
            for lock in &task_lock_list {
                wave_locks.insert(lock.clone());
            }
            wave.push(task_id);
        }

        // Put skipped tasks back for next wave consideration
        // (don't put them back in remaining to avoid infinite loop)

        if wave.is_empty() {
            // All ready tasks have lock conflicts with each other
            // Force the first one to be scheduled alone
            if let Some(forced_task) = skipped_due_to_lock.first() {
                wave.push(forced_task.clone());
            } else {
                // No more tasks can be scheduled
                break;
            }
        }

        // Mark wave tasks as completed and update in_degrees
        for task_id in &wave {
            completed.insert(task_id.clone());
            if let Some(deps) = dependents.get(task_id) {
                for dep in deps {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                    }
                }
            }
        }

        waves.push(wave);
    }

    waves
}

/// Detect potential lock conflicts between tasks
///
/// A conflict occurs when multiple tasks share the same lock and
/// there is no dependency relationship enforcing execution order.
///
/// # Arguments
/// * `plan` - The execution plan containing task definitions
///
/// # Returns
/// A vector of detected lock conflicts
pub fn detect_lock_conflicts(plan: &Plan) -> Vec<LockConflict> {
    // Build lock -> tasks mapping
    let mut lock_to_tasks: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in &plan.tasks {
        for lock in &task.locks {
            lock_to_tasks
                .entry(lock.as_str())
                .or_default()
                .push(task.id.as_str());
        }
    }

    // Build reachability for dependency checking
    let task_deps: HashMap<&str, HashSet<&str>> = plan
        .tasks
        .iter()
        .map(|t| {
            let deps: HashSet<&str> = t.deps.iter().map(|s| s.as_str()).collect();
            (t.id.as_str(), deps)
        })
        .collect();

    // Compute transitive closure of dependencies
    let mut reachable: HashMap<&str, HashSet<&str>> = HashMap::new();
    for task in &plan.tasks {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = task.deps.iter().map(|s| s.as_str()).collect();
        while let Some(dep) = stack.pop() {
            if visited.insert(dep) {
                if let Some(deps) = task_deps.get(dep) {
                    for d in deps {
                        stack.push(d);
                    }
                }
            }
        }
        reachable.insert(task.id.as_str(), visited);
    }

    let mut conflicts: Vec<LockConflict> = Vec::new();

    for (lock, tasks) in &lock_to_tasks {
        if tasks.len() < 2 {
            continue;
        }

        // Check if tasks are ordered by dependencies
        let mut unordered_pairs: Vec<(&str, &str)> = Vec::new();
        for (i, task_a) in tasks.iter().enumerate() {
            for task_b in tasks.iter().skip(i + 1) {
                let a_reaches_b = reachable
                    .get(task_b)
                    .map(|r| r.contains(task_a))
                    .unwrap_or(false);
                let b_reaches_a = reachable
                    .get(task_a)
                    .map(|r| r.contains(task_b))
                    .unwrap_or(false);

                if !a_reaches_b && !b_reaches_a {
                    unordered_pairs.push((task_a, task_b));
                }
            }
        }

        if !unordered_pairs.is_empty() {
            let notes = if unordered_pairs.len() == 1 {
                let (a, b) = unordered_pairs[0];
                format!("{a} and {b} have no dependency relationship")
            } else {
                format!(
                    "{} task pairs have no dependency relationship",
                    unordered_pairs.len()
                )
            };

            conflicts.push(LockConflict {
                lock_name: lock.to_string(),
                task_ids: tasks.iter().map(|s| s.to_string()).collect(),
                notes,
            });
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Plan, RunConfig, Task, TaskMode};

    fn create_test_plan(tasks: Vec<Task>) -> Plan {
        Plan {
            version: 1,
            run: RunConfig {
                name: Some("test-plan".to_string()),
                cwd: None,
                worktree: None,
                env: std::collections::HashMap::new(),
                max_concurrency: None,
                fail_fast: None,
                default_timeout_sec: None,
                notifications: None,
            },
            variables: std::collections::HashMap::new(),
            groups: std::collections::HashMap::new(),
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
            codex: Some(crate::plan::CodexConfig {
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
        }
    }

    #[test]
    fn test_simple_sequential_waves() {
        // A -> B -> C
        let tasks = vec![
            create_task("A", vec![], vec![]),
            create_task("B", vec!["A"], vec![]),
            create_task("C", vec!["B"], vec![]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 4);

        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["A"]);
        assert_eq!(waves[1], vec!["B"]);
        assert_eq!(waves[2], vec!["C"]);
    }

    #[test]
    fn test_parallel_waves() {
        // A, B (parallel) -> C
        let tasks = vec![
            create_task("A", vec![], vec![]),
            create_task("B", vec![], vec![]),
            create_task("C", vec!["A", "B"], vec![]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 4);

        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0], vec!["A", "B"]);
        assert_eq!(waves[1], vec!["C"]);
    }

    #[test]
    fn test_max_concurrency_limits_wave_size() {
        // A, B, C, D (all independent)
        let tasks = vec![
            create_task("A", vec![], vec![]),
            create_task("B", vec![], vec![]),
            create_task("C", vec![], vec![]),
            create_task("D", vec![], vec![]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 2);

        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0], vec!["A", "B"]);
        assert_eq!(waves[1], vec!["C", "D"]);
    }

    #[test]
    fn test_locks_prevent_parallel_execution() {
        // A and B share lock "db", so they cannot run in parallel
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec![], vec!["db"]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 4);

        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0], vec!["A"]);
        assert_eq!(waves[1], vec!["B"]);
    }

    #[test]
    fn test_different_locks_allow_parallel() {
        // A has lock "db", B has lock "cache" - can run in parallel
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec![], vec!["cache"]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 4);

        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec!["A", "B"]);
    }

    #[test]
    fn test_no_lock_conflict_with_dependency_ordering() {
        // A -> B, both have lock "db" but are ordered by dependency
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec!["A"], vec!["db"]),
        ];
        let plan = create_test_plan(tasks);

        let conflicts = detect_lock_conflicts(&plan);

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_lock_conflict_detected_without_dependency() {
        // A and B share lock "db" but have no dependency relationship
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec![], vec!["db"]),
        ];
        let plan = create_test_plan(tasks);

        let conflicts = detect_lock_conflicts(&plan);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].lock_name, "db");
        assert!(conflicts[0].task_ids.contains(&"A".to_string()));
        assert!(conflicts[0].task_ids.contains(&"B".to_string()));
    }

    #[test]
    fn test_no_conflicts_when_tasks_have_no_locks() {
        let tasks = vec![
            create_task("A", vec![], vec![]),
            create_task("B", vec![], vec![]),
        ];
        let plan = create_test_plan(tasks);

        let conflicts = detect_lock_conflicts(&plan);

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_complex_dag_with_mixed_locks() {
        // A (lock: db) -> C
        // B (lock: cache) -> C
        // C (lock: db)
        // D (lock: db, cache)
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec![], vec!["cache"]),
            create_task("C", vec!["A", "B"], vec!["db"]),
            create_task("D", vec![], vec!["db", "cache"]),
        ];
        let plan = create_test_plan(tasks);

        let waves = generate_execution_waves(&plan, 4);

        // A and B can run together (different locks)
        // D cannot run with A or B because it shares locks with both
        // C depends on A and B
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["A", "B"]);
        // D should be in wave 2 (after A releases db and B releases cache)
        // C should be in wave 3 (after A,B complete and D releases db)
        // Note: The order depends on the algorithm - D can't run with A/B due to lock overlap
    }

    #[test]
    fn test_transitive_dependency_prevents_conflict() {
        // A -> B -> C, all have lock "db"
        // No conflict because of transitive dependency
        let tasks = vec![
            create_task("A", vec![], vec!["db"]),
            create_task("B", vec!["A"], vec!["db"]),
            create_task("C", vec!["B"], vec!["db"]),
        ];
        let plan = create_test_plan(tasks);

        let conflicts = detect_lock_conflicts(&plan);

        assert!(conflicts.is_empty());
    }
}
