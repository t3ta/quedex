use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, Semaphore};

use crate::store::TaskStatus;

pub type TaskId = String;
pub type LockTable = HashMap<String, Option<TaskId>>;

#[derive(Debug, Clone)]
pub struct TaskSpec {
    pub id: TaskId,
    pub deps: Vec<TaskId>,
    pub locks: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
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
}

#[derive(Debug, Clone)]
pub struct ScheduleReport {
    pub tasks: HashMap<TaskId, TaskRecord>,
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
        }
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
        }
    }

    pub async fn run(self) -> ScheduleReport {
        let max_concurrency = self.options.max_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let lock_table = Arc::new(Mutex::new(init_lock_table(&self.tasks)));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let mut states = self.initial_states.unwrap_or_default();
        states.retain(|task_id, _| self.tasks.contains_key(task_id));
        for task_id in self.tasks.keys() {
            states.entry(task_id.clone()).or_insert(TaskRecord {
                status: TaskStatus::Pending,
                exit_code: None,
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

        loop {
            refresh_ready(
                &self.tasks,
                &mut states,
                &mut ready_queue,
                fail_fast_triggered,
            );

            let mut rotations = 0usize;
            while !ready_queue.is_empty() && semaphore.available_permits() > 0 {
                if rotations >= ready_queue.len() {
                    break;
                }
                let task_id = ready_queue
                    .pop_front()
                    .expect("ready queue empty after check");
                if fail_fast_triggered {
                    if let Some(record) = states.get_mut(&task_id) {
                        record.status = TaskStatus::Skipped;
                        record.exit_code = None;
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

                if let Some(record) = states.get_mut(&task_id) {
                    record.status = TaskStatus::Running;
                    record.exit_code = None;
                }
                running += 1;
                rotations = 0;

                let task_clone = task.clone();
                let task_id_clone = task_id.clone();
                let locks = task.locks.clone();
                let lock_table = Arc::clone(&lock_table);
                let tx = tx.clone();
                let future = self.runner.spawn(task_clone);

                tokio::spawn(async move {
                    let result = future.await;
                    release_locks(&lock_table, &task_id_clone, &locks);
                    drop(permit);
                    let _ = tx.send(SchedulerEvent::TaskFinished {
                        task_id: task_id_clone,
                        result,
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
            );

        }

        ScheduleReport { tasks: states }
    }
}

#[derive(Debug)]
enum SchedulerEvent {
    TaskFinished { task_id: TaskId, result: TaskResult },
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

fn try_acquire_locks(
    lock_table: &Arc<Mutex<LockTable>>,
    task_id: &TaskId,
    locks: &[String],
) -> bool {
    if locks.is_empty() {
        return true;
    }
    let mut table = lock_table
        .lock()
        .expect("lock table mutex poisoned");
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

fn release_locks(lock_table: &Arc<Mutex<LockTable>>, task_id: &TaskId, locks: &[String]) {
    if locks.is_empty() {
        return;
    }
    let mut table = lock_table
        .lock()
        .expect("lock table mutex poisoned");
    for lock in locks {
        #[allow(clippy::collapsible_if)]
        if let Some(holder) = table.get(lock) {
            if holder.as_ref() == Some(task_id) {
                table.insert(lock.clone(), None);
            }
        }
    }
}

fn refresh_ready(
    tasks: &HashMap<TaskId, TaskSpec>,
    states: &mut HashMap<TaskId, TaskRecord>,
    ready_queue: &mut VecDeque<TaskId>,
    fail_fast_triggered: bool,
) {
    if fail_fast_triggered {
        return;
    }
    for task in tasks.values() {
        let Some(record) = states.get(&task.id) else {
            continue;
        };
        if record.status != TaskStatus::Pending {
            continue;
        }
        if deps_failed(task, states) {
            if let Some(record) = states.get_mut(&task.id) {
                record.status = TaskStatus::Skipped;
                record.exit_code = None;
            }
            continue;
        }
        if deps_satisfied(task, states) {
            if let Some(record) = states.get_mut(&task.id) {
                record.status = TaskStatus::Ready;
                record.exit_code = None;
            }
            ready_queue.push_back(task.id.clone());
        }
    }
}

fn deps_satisfied(task: &TaskSpec, states: &HashMap<TaskId, TaskRecord>) -> bool {
    task.deps.iter().all(|dep| {
        states
            .get(dep)
            .is_some_and(|record| record.status == TaskStatus::Succeeded)
    })
}

fn deps_failed(task: &TaskSpec, states: &HashMap<TaskId, TaskRecord>) -> bool {
    task.deps.iter().any(|dep| {
        states.get(dep).is_none_or(|record| {
            matches!(
                record.status,
                TaskStatus::Failed | TaskStatus::Canceled | TaskStatus::Skipped
            )
        })
    })
}

fn handle_event(
    event: SchedulerEvent,
    states: &mut HashMap<TaskId, TaskRecord>,
    running: &mut usize,
    fail_fast_triggered: &mut bool,
    fail_fast_enabled: bool,
    ready_queue: &mut VecDeque<TaskId>,
) {
    match event {
        SchedulerEvent::TaskFinished { task_id, result } => {
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
            TaskStatus::Succeeded
                | TaskStatus::Failed
                | TaskStatus::Canceled
                | TaskStatus::Skipped
        )
    })
}
