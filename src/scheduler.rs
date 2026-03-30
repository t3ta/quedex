use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, mpsc};

use crate::plan::{TaskCondition, TaskMode, TaskResultCondition};
use crate::store::{SkipReason, TaskStatus};

pub type TaskId = String;
pub type LockTable = HashMap<String, Option<TaskId>>;

#[derive(Debug, Clone)]
pub struct TaskSpec {
    pub id: TaskId,
    pub deps: Vec<TaskId>,
    pub locks: Vec<String>,
    pub output_files: Option<Vec<String>>,
    pub condition: Option<TaskCondition>,
    // Task-related fields for commit generation
    pub title: Option<String>,
    pub mode: crate::plan::TaskMode,
    pub auto_commit: bool,
    pub commit_before_gates: bool,
    pub squash: bool,
}

impl Default for TaskSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            deps: Vec::new(),
            locks: Vec::new(),
            output_files: None,
            condition: None,
            title: None,
            mode: crate::plan::TaskMode::default(),
            auto_commit: true,
            commit_before_gates: false,
            squash: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub mode_concurrency: HashMap<TaskMode, usize>,
}

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
}

impl TaskResult {
    pub fn succeeded() -> Self {
        Self {
            status: TaskStatus::Succeeded,
            exit_code: Some(0),
        }
    }

    pub fn failed(exit_code: i32) -> Self {
        Self {
            status: TaskStatus::Failed,
            exit_code: Some(exit_code),
        }
    }

    pub fn canceled() -> Self {
        Self {
            status: TaskStatus::Canceled,
            exit_code: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub skip_reason: Option<SkipReason>,
}

#[derive(Debug, Clone)]
pub struct ScheduleReport {
    pub tasks: HashMap<TaskId, TaskRecord>,
    // Track commits created during execution for squash task
    pub commit_hashes: Vec<(String, String, Option<String>)>, // (task_id, commit_hash, title)
}

pub trait TaskRunner: Send + Sync + 'static {
    type Future: Future<Output = TaskResult> + Send + 'static;

    fn spawn(&self, task: TaskSpec) -> Self::Future;
}

pub struct Scheduler<R> {
    tasks: HashMap<TaskId, TaskSpec>,
    options: SchedulerOptions,
    runner: R,
    initial_states: Option<HashMap<TaskId, TaskRecord>>,
    // Git manager for commit handling; wrapped in a mutex to serialize git operations
    git_manager: Option<std::sync::Arc<std::sync::Mutex<crate::git::GitManager>>>,
}

impl<R> Scheduler<R>
where
    R: TaskRunner,
{
    pub fn new(tasks: Vec<TaskSpec>, options: SchedulerOptions, runner: R) -> Self {
        let mut map = HashMap::with_capacity(tasks.len());
        for task in tasks {
            map.insert(task.id.clone(), task);
        }
        Self {
            tasks: map,
            options,
            runner,
            initial_states: None,
            git_manager: None,
        }
    }

    pub fn with_git_manager(
        mut self,
        git_manager: std::sync::Arc<std::sync::Mutex<crate::git::GitManager>>,
    ) -> Self {
        self.git_manager = Some(git_manager);
        self
    }

    pub fn new_with_initial_state(
        tasks: Vec<TaskSpec>,
        options: SchedulerOptions,
        runner: R,
        initial_states: HashMap<TaskId, TaskRecord>,
    ) -> Self {
        let mut map = HashMap::with_capacity(tasks.len());
        for task in tasks {
            map.insert(task.id.clone(), task);
        }
        Self {
            tasks: map,
            options,
            runner,
            initial_states: Some(initial_states),
            git_manager: None,
        }
    }

    pub fn with_git_manager_for_state(
        mut self,
        git_manager: std::sync::Arc<std::sync::Mutex<crate::git::GitManager>>,
    ) -> Self {
        self.git_manager = Some(git_manager);
        self
    }

    pub async fn run(self, env_vars: &HashMap<String, String>) -> ScheduleReport {
        let max_concurrency = self.options.max_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let mode_semaphores: HashMap<TaskMode, Arc<Semaphore>> = self
            .options
            .mode_concurrency
            .iter()
            .map(|(&mode, &limit)| (mode, Arc::new(Semaphore::new(limit.max(1)))))
            .collect();
        let mode_semaphores = Arc::new(mode_semaphores);
        let lock_table = Arc::new(Mutex::new(init_lock_table(&self.tasks)));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let mut states = self.initial_states.unwrap_or_default();
        states.retain(|task_id, _| self.tasks.contains_key(task_id));
        for task_id in self.tasks.keys() {
            states.entry(task_id.clone()).or_insert(TaskRecord {
                status: TaskStatus::Pending,
                exit_code: None,
                skip_reason: None,
            });
        }

        let mut ready_queue = VecDeque::new();
        let mut running = 0usize;
        let mut fail_fast_triggered = self.options.fail_fast
            && states
                .values()
                .any(|record| record.status == TaskStatus::Failed);

        if fail_fast_triggered {
            apply_fail_fast(&mut states, &mut ready_queue);
        }

        let mut commit_hashes = Vec::new();

        loop {
            refresh_ready(
                &self.tasks,
                &mut states,
                &mut ready_queue,
                fail_fast_triggered,
                env_vars,
            );

            let mut rotations = 0usize;
            while !ready_queue.is_empty() && semaphore.available_permits() > 0 {
                if rotations >= ready_queue.len() {
                    break;
                }
                let Some(task_id) = ready_queue.pop_front() else {
                    break;
                };
                if fail_fast_triggered {
                    if let Some(record) = states.get_mut(&task_id) {
                        record.status = TaskStatus::Skipped;
                        record.exit_code = None;
                        record.skip_reason = Some(SkipReason::FailFast);
                    }
                    continue;
                }
                let task = match self.tasks.get(&task_id) {
                    Some(task) => task,
                    None => continue,
                };

                if !try_acquire_locks(&lock_table, &task_id, &task.locks) {
                    ready_queue.push_back(task_id);
                    rotations += 1;
                    continue;
                }

                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        release_locks(&lock_table, &task_id, &task.locks);
                        ready_queue.push_front(task_id);
                        break;
                    }
                };

                let mode_permit = if let Some(mode_sem) = mode_semaphores.get(&task.mode) {
                    match mode_sem.clone().try_acquire_owned() {
                        Ok(permit) => Some(permit),
                        Err(_) => {
                            release_locks(&lock_table, &task_id, &task.locks);
                            drop(permit);
                            ready_queue.push_back(task_id);
                            rotations += 1;
                            continue;
                        }
                    }
                } else {
                    None
                };

                if let Some(record) = states.get_mut(&task_id) {
                    record.status = TaskStatus::Running;
                    record.exit_code = None;
                }
                running += 1;
                rotations = 0;

                let task_clone = task.clone();
                let task_spec_for_event = task.clone();
                let task_id_clone = task_id.clone();
                let locks = task.locks.clone();
                let lock_table = Arc::clone(&lock_table);
                let mode_permit = mode_permit;
                let tx = tx.clone();
                let future = self.runner.spawn(task_clone);

                tokio::spawn(async move {
                    let result = future.await;
                    release_locks(&lock_table, &task_id_clone, &locks);
                    drop(mode_permit);
                    drop(permit);
                    let task_spec_for_event = task_spec_for_event.clone();
                    let _ = tx.send(SchedulerEvent::TaskFinished {
                        task_id: task_id_clone,
                        result,
                        task_spec: task_spec_for_event,
                    });
                });
            }

            if all_done(&states) {
                break;
            }

            if running == 0 && ready_queue.is_empty() && mark_stuck_tasks_skipped(&mut states) {
                if all_done(&states) {
                    break;
                }
                continue;
            }

            let event = match rx.recv().await {
                Some(event) => event,
                None => break,
            };
            handle_event(
                event,
                &mut states,
                &mut running,
                &mut fail_fast_triggered,
                self.options.fail_fast,
                &mut ready_queue,
                &mut commit_hashes,
                &self.git_manager,
            );
        }

        ScheduleReport {
            tasks: states,
            commit_hashes,
        }
    }
}

#[derive(Debug)]
enum SchedulerEvent {
    TaskFinished {
        task_id: TaskId,
        result: TaskResult,
        task_spec: TaskSpec,
    },
}

fn init_lock_table(tasks: &HashMap<TaskId, TaskSpec>) -> LockTable {
    let mut table = HashMap::new();
    for task in tasks.values() {
        for lock in &task.locks {
            table.entry(lock.clone()).or_insert(None);
        }
    }
    table
}

/// Attempts to acquire all required locks for a task.
///
/// # Arguments
/// * `lock_table` - Shared lock table mapping lock names to holding tasks
/// * `task_id` - The task requesting the locks
/// * `locks` - List of lock names to acquire
///
/// # Returns
/// `true` if all locks were acquired, `false` if any lock is held by another task
///
/// # Behavior
/// - If the task requires no locks, returns true immediately
/// - Checks if all locks are available (not held by another task)
/// - If any lock is held, returns false without acquiring any locks
/// - If all locks are available, acquires all of them atomically
fn try_acquire_locks(
    lock_table: &Arc<Mutex<LockTable>>,
    task_id: &TaskId,
    locks: &[String],
) -> bool {
    if locks.is_empty() {
        return true;
    }
    let mut table = match lock_table.lock() {
        Ok(guard) => guard,
        Err(_) => {
            eprintln!(
                "Error: lock table mutex poisoned, refusing to acquire locks (cannot safely proceed)"
            );
            return false;
        }
    };
    for lock in locks {
        if let Some(Some(_)) = table.get(lock) {
            return false;
        }
    }
    for lock in locks {
        table.insert(lock.clone(), Some(task_id.clone()));
    }
    true
}

/// Releases all locks held by a task.
///
/// # Arguments
/// * `lock_table` - Shared lock table mapping lock names to holding tasks
/// * `task_id` - The task releasing the locks
/// * `locks` - List of lock names to release
///
/// # Behavior
/// - If the task has no locks, returns immediately
/// - For each lock, checks if it's held by this task
/// - Only releases locks that are currently held by this task
/// - Ignores locks held by other tasks or already released locks
fn release_locks(lock_table: &Arc<Mutex<LockTable>>, task_id: &TaskId, locks: &[String]) {
    if locks.is_empty() {
        return;
    }
    let mut table = match lock_table.lock() {
        Ok(guard) => guard,
        Err(_) => {
            eprintln!(
                "Error: lock table mutex poisoned, skipping lock release (locks may remain held)"
            );
            return;
        }
    };
    for lock in locks {
        #[allow(clippy::collapsible_if)]
        if let Some(holder) = table.get(lock) {
            if holder.as_ref() == Some(task_id) {
                table.insert(lock.clone(), None);
            }
        }
    }
}

/// Refreshes the ready queue by checking which pending tasks have satisfied dependencies.
///
/// # Arguments
/// * `tasks` - All defined tasks in the plan
/// * `states` - Current execution state of all tasks
/// * `ready_queue` - Queue of tasks ready to execute (modified in place)
/// * `fail_fast_triggered` - Whether fail-fast mode is active
/// * `env_vars` - Environment variables for condition evaluation
///
/// # Behavior
/// - If fail_fast is active, returns immediately without queuing tasks
/// - For each pending task, checks if all dependencies succeeded (or were condition-skipped)
/// - If a dependency truly failed, marks the task as Skipped with DependencyFailed reason
/// - If condition is not met, marks the task as Skipped with ConditionNotMet reason
/// - If all dependencies are satisfied and condition is met, marks task as Ready and adds to queue
fn refresh_ready(
    tasks: &HashMap<TaskId, TaskSpec>,
    states: &mut HashMap<TaskId, TaskRecord>,
    ready_queue: &mut VecDeque<TaskId>,
    fail_fast_triggered: bool,
    env_vars: &HashMap<String, String>,
) {
    if fail_fast_triggered {
        return;
    }
    // Loop until no more state changes occur (handles dependency chains)
    loop {
        let mut changed = false;
        for task in tasks.values() {
            let Some(record) = states.get(&task.id) else {
                continue;
            };
            if record.status != TaskStatus::Pending {
                continue;
            }
            // Check if any dependency truly failed (not condition-skipped)
            if deps_failed(task, states) {
                if let Some(record) = states.get_mut(&task.id) {
                    record.status = TaskStatus::Skipped;
                    record.exit_code = None;
                    record.skip_reason = Some(SkipReason::DependencyFailed);
                    changed = true;
                }
                continue;
            }
            // Check if all dependencies are satisfied (succeeded or condition-skipped)
            if !deps_satisfied(task, states) {
                continue;
            }
            // Check condition if present
            if let Some(condition) = &task.condition {
                let (satisfied, desc) = evaluate_condition(condition, states, env_vars);
                if !satisfied {
                    if let Some(record) = states.get_mut(&task.id) {
                        record.status = TaskStatus::Skipped;
                        record.exit_code = None;
                        record.skip_reason = Some(SkipReason::ConditionNotMet { condition: desc });
                        changed = true;
                    }
                    continue;
                }
            }
            // All checks passed - mark as ready
            if let Some(record) = states.get_mut(&task.id) {
                record.status = TaskStatus::Ready;
                record.exit_code = None;
                changed = true;
            }
            ready_queue.push_back(task.id.clone());
        }
        if !changed {
            break;
        }
    }
}

/// Evaluates a task condition.
///
/// Returns a tuple of (satisfied, description) where:
/// - satisfied: whether the condition is met
/// - description: human-readable description of the condition evaluation
fn evaluate_condition(
    condition: &TaskCondition,
    states: &HashMap<TaskId, TaskRecord>,
    env_vars: &HashMap<String, String>,
) -> (bool, String) {
    use crate::plan::{ConditionStatus, EnvCondition, TaskResultCondition};

    match condition {
        TaskCondition::Env(EnvCondition {
            env,
            equals,
            not_equals,
            exists,
        }) => {
            let value = env_vars.get(env);
            // Priority order if multiple fields are set: equals > not_equals > exists
            // If none are set, defaults to checking if env var exists
            let result = match (equals, not_equals, exists) {
                (Some(expected), _, _) => value.map(|v| v == expected).unwrap_or(false),
                (_, Some(not_expected), _) => value.map(|v| v != not_expected).unwrap_or(true),
                (_, _, Some(should_exist)) => value.is_some() == *should_exist,
                _ => value.is_some(),
            };
            let desc = format!("env {env} = {value:?}");
            (result, desc)
        }
        TaskCondition::TaskResult(TaskResultCondition { task, status }) => {
            let actual_status = states.get(task).map(|r| r.status);
            let expected = match status {
                ConditionStatus::Succeeded => TaskStatus::Succeeded,
                ConditionStatus::Failed => TaskStatus::Failed,
            };
            let result = actual_status == Some(expected);
            let desc = format!("task {task} status = {actual_status:?}, expected {expected:?}");
            (result, desc)
        }
    }
}

/// Checks if all dependencies are satisfied.
/// A dependency is considered satisfied if it:
/// - Succeeded, OR
/// - Was skipped due to condition not met (condition-skip is treated as success), OR
/// - Failed but the task's condition expects that failure
fn deps_satisfied(task: &TaskSpec, states: &HashMap<TaskId, TaskRecord>) -> bool {
    // Get the task ID that is referenced in a TaskResultCondition with Failed status
    let condition_expects_failure: Option<&str> = task.condition.as_ref().and_then(|cond| {
        if let TaskCondition::TaskResult(TaskResultCondition {
            task: task_id,
            status,
        }) = cond
        {
            if *status == crate::plan::ConditionStatus::Failed {
                return Some(task_id.as_str());
            }
        }
        None
    });

    task.deps.iter().all(|dep| {
        states.get(dep).is_some_and(|record| {
            record.status == TaskStatus::Succeeded
                || (record.status == TaskStatus::Skipped
                    && matches!(record.skip_reason, Some(SkipReason::ConditionNotMet { .. })))
                || (record.status == TaskStatus::Failed
                    && condition_expects_failure == Some(dep.as_str()))
        })
    })
}

/// Checks if any dependency truly failed (not condition-skipped or expected by condition).
/// Returns true if a dependency:
/// - Failed (unless the task has a condition expecting that failure), OR
/// - Was canceled, OR
/// - Was skipped due to dependency failure or fail-fast (but NOT condition-skip)
fn deps_failed(task: &TaskSpec, states: &HashMap<TaskId, TaskRecord>) -> bool {
    // Get the task ID that is referenced in a TaskResultCondition with Failed status
    let condition_expects_failure: Option<&str> = task.condition.as_ref().and_then(|cond| {
        if let TaskCondition::TaskResult(TaskResultCondition {
            task: task_id,
            status,
        }) = cond
        {
            if *status == crate::plan::ConditionStatus::Failed {
                return Some(task_id.as_str());
            }
        }
        None
    });

    task.deps.iter().any(|dep| {
        states.get(dep).is_some_and(|record| {
            match record.status {
                TaskStatus::Failed => {
                    // If this task's condition expects this dep to fail, don't treat it as dep failure
                    condition_expects_failure != Some(dep.as_str())
                }
                TaskStatus::Canceled => true,
                TaskStatus::Skipped => {
                    // Condition-skip is treated as success, other skips are failure
                    !matches!(record.skip_reason, Some(SkipReason::ConditionNotMet { .. }))
                }
                _ => false,
            }
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_event(
    event: SchedulerEvent,
    states: &mut HashMap<TaskId, TaskRecord>,
    running: &mut usize,
    fail_fast_triggered: &mut bool,
    fail_fast_enabled: bool,
    ready_queue: &mut VecDeque<TaskId>,
    commit_hashes: &mut Vec<(String, String, Option<String>)>, // (task_id, commit_hash, title)
    git_manager: &Option<std::sync::Arc<std::sync::Mutex<crate::git::GitManager>>>,
) {
    match event {
        SchedulerEvent::TaskFinished {
            task_id,
            result,
            task_spec,
        } => {
            // Handle git commit if task succeeded and auto_commit is enabled
            if result.status == TaskStatus::Succeeded
                && task_spec.auto_commit
                && !task_spec.commit_before_gates
                && matches!(
                    task_spec.mode,
                    crate::plan::TaskMode::Implement | crate::plan::TaskMode::Verify
                )
            {
                if let Some(gm) = git_manager {
                    // Lock the git manager for thread-safe operations
                    if let Ok(gm_locked) = gm.lock() {
                        // Try to create commit
                        match gm_locked.ensure_on_branch() {
                            Ok(_) => {
                                let title = task_spec
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| task_spec.id.clone());
                                let message = crate::git::generate_commit_message(
                                    &title,
                                    &task_spec.id,
                                    &format!("{:?}", task_spec.mode),
                                );

                                match gm_locked.create_commit(&message) {
                                    Ok(commit_hash) => {
                                        if !commit_hash.is_empty() {
                                            commit_hashes.push((
                                                task_spec.id.clone(),
                                                commit_hash,
                                                task_spec.title,
                                            ));
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "Warning: auto-commit create_commit failed for task {}: {:?}",
                                            task_spec.id, err
                                        );
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!(
                                    "Warning: auto-commit ensure_on_branch failed for task {}: {:?}",
                                    task_spec.id, err
                                );
                            }
                        }
                    } else {
                        eprintln!(
                            "Warning: could not acquire git manager lock for task {}",
                            task_spec.id
                        );
                    }
                }
            }

            if let Some(record) = states.get_mut(&task_id) {
                record.status = result.status;
                record.exit_code = result.exit_code;
            }
            *running = running.saturating_sub(1);
            if result.status == TaskStatus::Failed && fail_fast_enabled && !*fail_fast_triggered {
                *fail_fast_triggered = true;
                apply_fail_fast(states, ready_queue);
            }
        }
    }
}

fn apply_fail_fast(states: &mut HashMap<TaskId, TaskRecord>, ready_queue: &mut VecDeque<TaskId>) {
    ready_queue.clear();
    for record in states.values_mut() {
        if matches!(record.status, TaskStatus::Pending | TaskStatus::Ready) {
            record.status = TaskStatus::Skipped;
            record.exit_code = None;
            record.skip_reason = Some(SkipReason::FailFast);
        }
    }
}

fn mark_stuck_tasks_skipped(states: &mut HashMap<TaskId, TaskRecord>) -> bool {
    let mut changed = false;
    for record in states.values_mut() {
        if matches!(record.status, TaskStatus::Pending | TaskStatus::Ready) {
            record.status = TaskStatus::Skipped;
            record.exit_code = None;
            changed = true;
        }
    }
    changed
}

fn all_done(states: &HashMap<TaskId, TaskRecord>) -> bool {
    states.values().all(|record| {
        matches!(
            record.status,
            TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Canceled | TaskStatus::Skipped
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        ConditionStatus, EnvCondition, TaskCondition, TaskMode, TaskResultCondition,
    };

    fn make_task_spec(id: &str, deps: Vec<&str>, condition: Option<TaskCondition>) -> TaskSpec {
        TaskSpec {
            id: id.to_string(),
            deps: deps.into_iter().map(String::from).collect(),
            locks: vec![],
            output_files: None,
            condition,
            title: None,
            mode: TaskMode::Implement,
            auto_commit: false,
            commit_before_gates: false,
            squash: false,
        }
    }

    fn make_record(status: TaskStatus, skip_reason: Option<SkipReason>) -> TaskRecord {
        TaskRecord {
            status,
            exit_code: None,
            skip_reason,
        }
    }

    // ==================== deps_satisfied tests ====================

    #[test]
    fn test_deps_satisfied_no_deps() {
        let task = make_task_spec("task_a", vec![], None);
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        assert!(deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_all_succeeded() {
        let task = make_task_spec("task_c", vec!["task_a", "task_b"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(TaskStatus::Succeeded, None),
        );
        states.insert(
            "task_b".to_string(),
            make_record(TaskStatus::Succeeded, None),
        );
        assert!(deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_one_pending() {
        let task = make_task_spec("task_c", vec!["task_a", "task_b"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(TaskStatus::Succeeded, None),
        );
        states.insert("task_b".to_string(), make_record(TaskStatus::Pending, None));
        assert!(!deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_one_running() {
        let task = make_task_spec("task_c", vec!["task_a", "task_b"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(TaskStatus::Succeeded, None),
        );
        states.insert("task_b".to_string(), make_record(TaskStatus::Running, None));
        assert!(!deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_condition_skipped() {
        // Condition-skipped dependencies should be treated as success
        let task = make_task_spec("task_b", vec!["task_a"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(
                TaskStatus::Skipped,
                Some(SkipReason::ConditionNotMet {
                    condition: "env TEST".to_string(),
                }),
            ),
        );
        assert!(deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_fail_fast_skipped_not_satisfied() {
        // FailFast-skipped dependencies are NOT treated as satisfied
        let task = make_task_spec("task_b", vec!["task_a"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(TaskStatus::Skipped, Some(SkipReason::FailFast)),
        );
        assert!(!deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_expected_failure() {
        // When task condition expects a dependency to fail, that failure is treated as satisfied
        let condition = TaskCondition::TaskResult(TaskResultCondition {
            task: "task_a".to_string(),
            status: ConditionStatus::Failed,
        });
        let task = make_task_spec("task_b", vec!["task_a"], Some(condition));
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert("task_a".to_string(), make_record(TaskStatus::Failed, None));
        assert!(deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_unexpected_failure() {
        // When task does not expect failure, a failed dependency is not satisfied
        let task = make_task_spec("task_b", vec!["task_a"], None);
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert("task_a".to_string(), make_record(TaskStatus::Failed, None));
        assert!(!deps_satisfied(&task, &states));
    }

    #[test]
    fn test_deps_satisfied_missing_dep() {
        // If dependency is not in states map, it's not satisfied
        let task = make_task_spec("task_b", vec!["task_a"], None);
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        assert!(!deps_satisfied(&task, &states));
    }

    // ==================== evaluate_condition tests ====================

    #[test]
    fn test_evaluate_condition_env_equals_match() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "expected_value".to_string());

        let (result, desc) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
        assert!(desc.contains("MY_VAR"));
    }

    #[test]
    fn test_evaluate_condition_env_equals_no_match() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "different_value".to_string());

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(!result);
    }

    #[test]
    fn test_evaluate_condition_env_equals_missing() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(!result);
    }

    #[test]
    fn test_evaluate_condition_env_not_equals_match() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: Some("forbidden_value".to_string()),
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "allowed_value".to_string());

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_env_not_equals_forbidden() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: Some("forbidden_value".to_string()),
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "forbidden_value".to_string());

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(!result);
    }

    #[test]
    fn test_evaluate_condition_env_not_equals_missing() {
        // When env var is missing and not_equals is set, it should be true
        // (a missing value can't equal the forbidden value)
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: Some("forbidden_value".to_string()),
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_env_exists_true() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: None,
            exists: Some(true),
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "any_value".to_string());

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_env_exists_false() {
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: None,
            exists: Some(false),
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_env_default_exists() {
        // When no condition field is set, defaults to checking existence
        let condition = TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: None,
            exists: None,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("MY_VAR".to_string(), "any_value".to_string());

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_task_result_succeeded() {
        let condition = TaskCondition::TaskResult(TaskResultCondition {
            task: "task_a".to_string(),
            status: ConditionStatus::Succeeded,
        });
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert(
            "task_a".to_string(),
            make_record(TaskStatus::Succeeded, None),
        );
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, desc) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
        assert!(desc.contains("task_a"));
        assert!(desc.contains("Succeeded"));
    }

    #[test]
    fn test_evaluate_condition_task_result_failed() {
        let condition = TaskCondition::TaskResult(TaskResultCondition {
            task: "task_a".to_string(),
            status: ConditionStatus::Failed,
        });
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert("task_a".to_string(), make_record(TaskStatus::Failed, None));
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(result);
    }

    #[test]
    fn test_evaluate_condition_task_result_mismatch() {
        let condition = TaskCondition::TaskResult(TaskResultCondition {
            task: "task_a".to_string(),
            status: ConditionStatus::Succeeded,
        });
        let mut states: HashMap<TaskId, TaskRecord> = HashMap::new();
        states.insert("task_a".to_string(), make_record(TaskStatus::Failed, None));
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(!result);
    }

    #[test]
    fn test_evaluate_condition_task_result_missing() {
        let condition = TaskCondition::TaskResult(TaskResultCondition {
            task: "task_a".to_string(),
            status: ConditionStatus::Succeeded,
        });
        let states: HashMap<TaskId, TaskRecord> = HashMap::new();
        let env_vars: HashMap<String, String> = HashMap::new();

        let (result, _) = evaluate_condition(&condition, &states, &env_vars);
        assert!(!result);
    }

    // ==================== Lock tests ====================

    #[test]
    fn test_init_lock_table() {
        let mut tasks: HashMap<TaskId, TaskSpec> = HashMap::new();
        tasks.insert(
            "task_a".to_string(),
            TaskSpec {
                id: "task_a".to_string(),
                deps: vec![],
                locks: vec!["lock1".to_string(), "lock2".to_string()],
                output_files: None,
                condition: None,
                title: None,
                mode: TaskMode::Implement,
                auto_commit: false,
                commit_before_gates: false,
                squash: false,
            },
        );
        tasks.insert(
            "task_b".to_string(),
            TaskSpec {
                id: "task_b".to_string(),
                deps: vec![],
                locks: vec!["lock2".to_string(), "lock3".to_string()],
                output_files: None,
                condition: None,
                title: None,
                mode: TaskMode::Implement,
                auto_commit: false,
                commit_before_gates: false,
                squash: false,
            },
        );

        let lock_table = init_lock_table(&tasks);
        assert_eq!(lock_table.len(), 3);
        assert_eq!(lock_table.get("lock1"), Some(&None));
        assert_eq!(lock_table.get("lock2"), Some(&None));
        assert_eq!(lock_table.get("lock3"), Some(&None));
    }

    #[test]
    fn test_try_acquire_locks_empty() {
        let lock_table = Arc::new(Mutex::new(HashMap::new()));
        let result = try_acquire_locks(&lock_table, &"task_a".to_string(), &[]);
        assert!(result);
    }

    #[test]
    fn test_try_acquire_locks_success() {
        let mut initial: LockTable = HashMap::new();
        initial.insert("lock1".to_string(), None);
        initial.insert("lock2".to_string(), None);
        let lock_table = Arc::new(Mutex::new(initial));

        let result = try_acquire_locks(
            &lock_table,
            &"task_a".to_string(),
            &["lock1".to_string(), "lock2".to_string()],
        );
        assert!(result);

        // Verify locks are held by task_a
        let table = lock_table.lock().unwrap();
        assert_eq!(table.get("lock1"), Some(&Some("task_a".to_string())));
        assert_eq!(table.get("lock2"), Some(&Some("task_a".to_string())));
    }

    #[test]
    fn test_try_acquire_locks_conflict() {
        let mut initial: LockTable = HashMap::new();
        initial.insert("lock1".to_string(), None);
        initial.insert("lock2".to_string(), Some("task_b".to_string())); // Already held
        let lock_table = Arc::new(Mutex::new(initial));

        let result = try_acquire_locks(
            &lock_table,
            &"task_a".to_string(),
            &["lock1".to_string(), "lock2".to_string()],
        );
        assert!(!result);

        // Verify lock1 was NOT acquired (atomic failure)
        let table = lock_table.lock().unwrap();
        assert_eq!(table.get("lock1"), Some(&None));
        assert_eq!(table.get("lock2"), Some(&Some("task_b".to_string())));
    }

    #[test]
    fn test_release_locks() {
        let mut initial: LockTable = HashMap::new();
        initial.insert("lock1".to_string(), Some("task_a".to_string()));
        initial.insert("lock2".to_string(), Some("task_a".to_string()));
        initial.insert("lock3".to_string(), Some("task_b".to_string())); // Held by different task
        let lock_table = Arc::new(Mutex::new(initial));

        release_locks(
            &lock_table,
            &"task_a".to_string(),
            &[
                "lock1".to_string(),
                "lock2".to_string(),
                "lock3".to_string(),
            ],
        );

        // Verify only task_a's locks were released
        let table = lock_table.lock().unwrap();
        assert_eq!(table.get("lock1"), Some(&None));
        assert_eq!(table.get("lock2"), Some(&None));
        assert_eq!(table.get("lock3"), Some(&Some("task_b".to_string()))); // Not released
    }

    #[test]
    fn test_release_locks_empty() {
        let lock_table = Arc::new(Mutex::new(HashMap::new()));
        // Should not panic
        release_locks(&lock_table, &"task_a".to_string(), &[]);
    }
}
