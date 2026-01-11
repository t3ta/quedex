use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use quedex::scheduler::{ScheduleReport, Scheduler, SchedulerOptions, TaskId, TaskResult, TaskRunner, TaskSpec};
use quedex::store::TaskStatus;

type BoxFuture = Pin<Box<dyn Future<Output = TaskResult> + Send>>;

#[derive(Clone)]
struct TaskBehavior {
    delay: Duration,
    result: TaskResult,
}

#[derive(Clone)]
struct TestRunner {
    behaviors: Arc<HashMap<TaskId, TaskBehavior>>,
    started: Option<mpsc::UnboundedSender<TaskId>>,
    running: Option<Arc<AtomicUsize>>,
    max_running: Option<Arc<AtomicUsize>>,
    lock_state: Option<Arc<Mutex<HashSet<String>>>>,
    lock_violation: Option<Arc<AtomicBool>>,
}

impl TestRunner {
    fn behavior_for(&self, task_id: &TaskId) -> TaskBehavior {
        self.behaviors.get(task_id).cloned().unwrap_or(TaskBehavior {
            delay: Duration::from_millis(0),
            result: TaskResult::succeeded(),
        })
    }
}

impl TaskRunner for TestRunner {
    type Future = BoxFuture;

    fn spawn(&self, task: TaskSpec) -> Self::Future {
        let behavior = self.behavior_for(&task.id);
        let started = self.started.clone();
        let running = self.running.clone();
        let max_running = self.max_running.clone();
        let lock_state = self.lock_state.clone();
        let lock_violation = self.lock_violation.clone();
        let task_id = task.id.clone();
        let locks = task.locks.clone();

        Box::pin(async move {
            if let Some(sender) = started {
                let _ = sender.send(task_id.clone());
            }

            if let Some(running) = &running {
                let current = running.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(max_running) = &max_running {
                    update_max(max_running, current);
                }
            }

            if let Some(lock_state) = &lock_state {
                if !locks.is_empty() {
                    let mut state = lock_state.lock().expect("lock state mutex poisoned");
                    for lock in &locks {
                        if state.contains(lock) {
                            if let Some(flag) = &lock_violation {
                                flag.store(true, Ordering::SeqCst);
                            }
                        }
                        state.insert(lock.clone());
                    }
                }
            }

            if !behavior.delay.is_zero() {
                tokio::time::sleep(behavior.delay).await;
            }

            if let Some(lock_state) = &lock_state {
                if !locks.is_empty() {
                    let mut state = lock_state.lock().expect("lock state mutex poisoned");
                    for lock in &locks {
                        state.remove(lock);
                    }
                }
            }

            if let Some(running) = &running {
                running.fetch_sub(1, Ordering::SeqCst);
            }

            behavior.result
        })
    }
}

fn update_max(max_running: &AtomicUsize, value: usize) {
    let mut prev = max_running.load(Ordering::SeqCst);
    while value > prev {
        match max_running.compare_exchange(prev, value, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => prev = next,
        }
    }
}

fn build_scheduler(tasks: Vec<TaskSpec>, runner: TestRunner, max_concurrency: usize, fail_fast: bool) -> Scheduler<TestRunner> {
    Scheduler::new(
        tasks,
        SchedulerOptions {
            max_concurrency,
            fail_fast,
        },
        runner,
    )
}

fn drain_started(rx: &mut mpsc::UnboundedReceiver<TaskId>) -> Vec<TaskId> {
    let mut ids = Vec::new();
    while let Ok(id) = rx.try_recv() {
        ids.push(id);
    }
    ids
}

fn assert_status(report: &ScheduleReport, task_id: &str, expected: TaskStatus) {
    let record = report
        .tasks
        .get(task_id)
        .unwrap_or_else(|| panic!("missing task {}", task_id));
    assert_eq!(record.status, expected);
}

#[tokio::test]
async fn scheduler_resolves_deps_in_order() {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let behaviors = HashMap::from([
        (
            "A".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(10),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "B".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(10),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "C".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(10),
                result: TaskResult::succeeded(),
            },
        ),
    ]);

    let runner = TestRunner {
        behaviors: Arc::new(behaviors),
        started: Some(tx),
        running: None,
        max_running: None,
        lock_state: None,
        lock_violation: None,
    };

    let tasks = vec![
        TaskSpec {
            id: "A".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "B".to_string(),
            deps: vec!["A".to_string()],
            locks: vec![],
        },
        TaskSpec {
            id: "C".to_string(),
            deps: vec!["B".to_string()],
            locks: vec![],
        },
    ];

    let scheduler = build_scheduler(tasks, runner, 4, false);
    let report = scheduler.run().await;

    let order = drain_started(&mut rx);
    assert_eq!(order, vec!["A", "B", "C"]);
    assert_status(&report, "A", TaskStatus::Succeeded);
    assert_status(&report, "B", TaskStatus::Succeeded);
    assert_status(&report, "C", TaskStatus::Succeeded);
}

#[tokio::test]
async fn scheduler_enforces_lock_exclusion() {
    let behaviors = HashMap::from([
        (
            "A".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "B".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "C".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(20),
                result: TaskResult::succeeded(),
            },
        ),
    ]);

    let lock_state = Arc::new(Mutex::new(HashSet::new()));
    let lock_violation = Arc::new(AtomicBool::new(false));

    let runner = TestRunner {
        behaviors: Arc::new(behaviors),
        started: None,
        running: None,
        max_running: None,
        lock_state: Some(lock_state),
        lock_violation: Some(lock_violation.clone()),
    };

    let tasks = vec![
        TaskSpec {
            id: "A".to_string(),
            deps: vec![],
            locks: vec!["db".to_string()],
        },
        TaskSpec {
            id: "B".to_string(),
            deps: vec![],
            locks: vec!["db".to_string()],
        },
        TaskSpec {
            id: "C".to_string(),
            deps: vec![],
            locks: vec![],
        },
    ];

    let scheduler = build_scheduler(tasks, runner, 2, false);
    let report = scheduler.run().await;

    assert!(!lock_violation.load(Ordering::SeqCst));
    assert_status(&report, "A", TaskStatus::Succeeded);
    assert_status(&report, "B", TaskStatus::Succeeded);
    assert_status(&report, "C", TaskStatus::Succeeded);
}

#[tokio::test]
async fn scheduler_skips_pending_tasks_on_fail_fast() {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let behaviors = HashMap::from([
        (
            "A".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(10),
                result: TaskResult::failed(1),
            },
        ),
        (
            "B".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(10),
                result: TaskResult::failed(2),
            },
        ),
        (
            "C".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(200),
                result: TaskResult::succeeded(),
            },
        ),
    ]);

    let runner = TestRunner {
        behaviors: Arc::new(behaviors),
        started: Some(tx),
        running: None,
        max_running: None,
        lock_state: None,
        lock_violation: None,
    };

    let tasks = vec![
        TaskSpec {
            id: "A".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "B".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "C".to_string(),
            deps: vec![],
            locks: vec![],
        },
    ];

    let scheduler = build_scheduler(tasks, runner, 2, true);
    let report = scheduler.run().await;
    let started = drain_started(&mut rx);

    let skipped: Vec<_> = report
        .tasks
        .iter()
        .filter(|(_, record)| record.status == TaskStatus::Skipped)
        .map(|(id, _)| id.clone())
        .collect();

    assert_eq!(skipped.len(), 1);
    assert_eq!(started.len(), 2);
    assert!(!started.contains(&skipped[0]));
}

#[tokio::test]
async fn scheduler_respects_max_concurrency() {
    let behaviors = HashMap::from([
        (
            "A".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "B".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "C".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
        (
            "D".to_string(),
            TaskBehavior {
                delay: Duration::from_millis(50),
                result: TaskResult::succeeded(),
            },
        ),
    ]);

    let running = Arc::new(AtomicUsize::new(0));
    let max_running = Arc::new(AtomicUsize::new(0));

    let runner = TestRunner {
        behaviors: Arc::new(behaviors),
        started: None,
        running: Some(running),
        max_running: Some(max_running.clone()),
        lock_state: None,
        lock_violation: None,
    };

    let tasks = vec![
        TaskSpec {
            id: "A".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "B".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "C".to_string(),
            deps: vec![],
            locks: vec![],
        },
        TaskSpec {
            id: "D".to_string(),
            deps: vec![],
            locks: vec![],
        },
    ];

    let scheduler = build_scheduler(tasks, runner, 2, false);
    let _ = scheduler.run().await;

    assert!(max_running.load(Ordering::SeqCst) <= 2);
}
