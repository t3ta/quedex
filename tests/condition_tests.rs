use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use quedex::plan::{ConditionStatus, EnvCondition, TaskCondition, TaskResultCondition};
use quedex::scheduler::{
    ScheduleReport, Scheduler, SchedulerOptions, TaskId, TaskResult, TaskRunner, TaskSpec,
};
use quedex::store::{SkipReason, TaskStatus};

type BoxFuture = Pin<Box<dyn Future<Output = TaskResult> + Send>>;

#[derive(Clone)]
struct TaskBehavior {
    delay: Duration,
    result: TaskResult,
}

#[derive(Clone)]
struct TestRunner {
    behaviors: HashMap<TaskId, TaskBehavior>,
}

impl TestRunner {
    fn new() -> Self {
        Self {
            behaviors: HashMap::new(),
        }
    }

    fn with_behavior(mut self, task_id: &str, result: TaskResult) -> Self {
        self.behaviors.insert(
            task_id.to_string(),
            TaskBehavior {
                delay: Duration::from_millis(0),
                result,
            },
        );
        self
    }
}

impl TaskRunner for TestRunner {
    type Future = BoxFuture;

    fn spawn(&self, task: TaskSpec) -> Self::Future {
        let behavior = self.behaviors.get(&task.id).cloned().unwrap_or(TaskBehavior {
            delay: Duration::from_millis(0),
            result: TaskResult::succeeded(),
        });

        Box::pin(async move {
            if !behavior.delay.is_zero() {
                tokio::time::sleep(behavior.delay).await;
            }
            behavior.result
        })
    }
}

fn build_scheduler(tasks: Vec<TaskSpec>, runner: TestRunner) -> Scheduler<TestRunner> {
    Scheduler::new(
        tasks,
        SchedulerOptions {
            max_concurrency: 4,
            fail_fast: false,
        },
        runner,
    )
}

fn assert_status(report: &ScheduleReport, task_id: &str, expected: TaskStatus) {
    let record = report
        .tasks
        .get(task_id)
        .unwrap_or_else(|| panic!("missing task {task_id}"));
    assert_eq!(record.status, expected);
}

fn assert_skip_reason_condition_not_met(report: &ScheduleReport, task_id: &str) {
    let record = report
        .tasks
        .get(task_id)
        .unwrap_or_else(|| panic!("missing task {task_id}"));
    assert!(
        matches!(record.skip_reason, Some(SkipReason::ConditionNotMet { .. })),
        "expected ConditionNotMet, got {:?}",
        record.skip_reason
    );
}

// === Environment Variable Condition Tests ===

#[tokio::test]
async fn env_condition_equals_match() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "expected_value".to_string());

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Succeeded);
}

#[tokio::test]
async fn env_condition_equals_no_match() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "different_value".to_string());

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Skipped);
    assert_skip_reason_condition_not_met(&report, "task1");
}

#[tokio::test]
async fn env_condition_equals_missing_var() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: Some("expected_value".to_string()),
            not_equals: None,
            exists: None,
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let env_vars = HashMap::new();

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Skipped);
    assert_skip_reason_condition_not_met(&report, "task1");
}

#[tokio::test]
async fn env_condition_not_equals_match() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: Some("forbidden_value".to_string()),
            exists: None,
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "other_value".to_string());

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Succeeded);
}

#[tokio::test]
async fn env_condition_not_equals_no_match() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: Some("forbidden_value".to_string()),
            exists: None,
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "forbidden_value".to_string());

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Skipped);
}

#[tokio::test]
async fn env_condition_exists_true() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: None,
            exists: Some(true),
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "any_value".to_string());

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Succeeded);
}

#[tokio::test]
async fn env_condition_exists_false() {
    let runner = TestRunner::new();
    let tasks = vec![TaskSpec {
        id: "task1".to_string(),
        deps: vec![],
        locks: vec![],
        output_files: None,
        condition: Some(TaskCondition::Env(EnvCondition {
            env: "MY_VAR".to_string(),
            equals: None,
            not_equals: None,
            exists: Some(false),
        })),
        title: None,
        mode: quedex::plan::TaskMode::default(),
        auto_commit: true,
        squash: false,
    }];

    let scheduler = build_scheduler(tasks, runner);
    let env_vars = HashMap::new();

    let report = scheduler.run(&env_vars).await;
    assert_status(&report, "task1", TaskStatus::Succeeded);
}

// === Task Result Condition Tests ===

#[tokio::test]
async fn task_condition_succeeded_match() {
    let runner = TestRunner::new()
        .with_behavior("check", TaskResult::succeeded());

    let tasks = vec![
        TaskSpec {
            id: "check".to_string(),
            deps: vec![],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
        TaskSpec {
            id: "main".to_string(),
            deps: vec!["check".to_string()],
            locks: vec![],
            output_files: None,
            condition: Some(TaskCondition::TaskResult(TaskResultCondition {
                task: "check".to_string(),
                status: ConditionStatus::Succeeded,
            })),
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
    ];

    let scheduler = build_scheduler(tasks, runner);
    let report = scheduler.run(&HashMap::new()).await;

    assert_status(&report, "check", TaskStatus::Succeeded);
    assert_status(&report, "main", TaskStatus::Succeeded);
}

#[tokio::test]
async fn task_condition_failed_match() {
    let runner = TestRunner::new()
        .with_behavior("check", TaskResult::failed(1));

    let tasks = vec![
        TaskSpec {
            id: "check".to_string(),
            deps: vec![],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
        TaskSpec {
            id: "fix".to_string(),
            deps: vec!["check".to_string()],
            locks: vec![],
            output_files: None,
            condition: Some(TaskCondition::TaskResult(TaskResultCondition {
                task: "check".to_string(),
                status: ConditionStatus::Failed,
            })),
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
    ];

    let scheduler = build_scheduler(tasks, runner);
    let report = scheduler.run(&HashMap::new()).await;

    assert_status(&report, "check", TaskStatus::Failed);
    assert_status(&report, "fix", TaskStatus::Succeeded);
}

#[tokio::test]
async fn task_condition_failed_no_match_when_succeeded() {
    let runner = TestRunner::new()
        .with_behavior("check", TaskResult::succeeded());

    let tasks = vec![
        TaskSpec {
            id: "check".to_string(),
            deps: vec![],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
        TaskSpec {
            id: "fix".to_string(),
            deps: vec!["check".to_string()],
            locks: vec![],
            output_files: None,
            condition: Some(TaskCondition::TaskResult(TaskResultCondition {
                task: "check".to_string(),
                status: ConditionStatus::Failed,
            })),
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
    ];

    let scheduler = build_scheduler(tasks, runner);
    let report = scheduler.run(&HashMap::new()).await;

    assert_status(&report, "check", TaskStatus::Succeeded);
    assert_status(&report, "fix", TaskStatus::Skipped);
    assert_skip_reason_condition_not_met(&report, "fix");
}

// === Condition Skip Propagation Tests ===

#[tokio::test]
async fn condition_skip_does_not_propagate_as_failure() {
    let runner = TestRunner::new();

    let tasks = vec![
        TaskSpec {
            id: "conditional".to_string(),
            deps: vec![],
            locks: vec![],
            output_files: None,
            condition: Some(TaskCondition::Env(EnvCondition {
                env: "SKIP_ME".to_string(),
                equals: Some("false".to_string()),
                not_equals: None,
                exists: None,
            })),
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
        TaskSpec {
            id: "dependent".to_string(),
            deps: vec!["conditional".to_string()],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
    ];

    let scheduler = build_scheduler(tasks, runner);
    let mut env_vars = HashMap::new();
    env_vars.insert("SKIP_ME".to_string(), "true".to_string());

    let report = scheduler.run(&env_vars).await;

    // conditional is skipped due to condition
    assert_status(&report, "conditional", TaskStatus::Skipped);
    assert_skip_reason_condition_not_met(&report, "conditional");

    // dependent should still run because condition-skip is treated as success
    assert_status(&report, "dependent", TaskStatus::Succeeded);
}

#[tokio::test]
async fn dependency_failure_skip_does_propagate() {
    let runner = TestRunner::new()
        .with_behavior("failing", TaskResult::failed(1));

    let tasks = vec![
        TaskSpec {
            id: "failing".to_string(),
            deps: vec![],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
        TaskSpec {
            id: "dependent".to_string(),
            deps: vec!["failing".to_string()],
            locks: vec![],
            output_files: None,
            condition: None,
            title: None,
            mode: quedex::plan::TaskMode::default(),
            auto_commit: true,
            squash: false,
        },
    ];

    let scheduler = build_scheduler(tasks, runner);
    let report = scheduler.run(&HashMap::new()).await;

    assert_status(&report, "failing", TaskStatus::Failed);
    // dependent should be skipped due to dependency failure
    assert_status(&report, "dependent", TaskStatus::Skipped);
    let record = report.tasks.get("dependent").unwrap();
    assert!(
        matches!(record.skip_reason, Some(SkipReason::DependencyFailed)),
        "expected DependencyFailed, got {:?}",
        record.skip_reason
    );
}

// === Plan Validation Tests ===

#[test]
fn plan_rejects_condition_referencing_nonexistent_task() {
    use quedex::plan::{CodexConfig, Plan, RunConfig, Task, TaskMode};

    let task = Task {
        id: "main".to_string(),
        title: None,
        mode: TaskMode::Implement,
        group: None,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: Some(CodexConfig {
            prompt: "test".to_string(),
            output_last_message: None,
            verify_after: false,
            sandbox: None,
            ask_for_approval: None,
            json: true,
        }),
        claude_code: None,
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        auto_commit: true,
        squash: false,
        condition: Some(TaskCondition::TaskResult(TaskResultCondition {
            task: "nonexistent".to_string(),
            status: ConditionStatus::Failed,
        })),
    };

    let plan = Plan {
        version: 1,
        run: RunConfig::default(),
        variables: HashMap::new(),
        groups: HashMap::new(),
        tasks: vec![task],
    };

    let err = plan.validate().expect_err("expected validation error");
    let msg = err.to_string();
    assert!(
        msg.contains("condition references non-existent task"),
        "unexpected error: {msg}"
    );
}

#[test]
fn plan_rejects_condition_referencing_self() {
    use quedex::plan::{CodexConfig, Plan, RunConfig, Task, TaskMode};

    let task = Task {
        id: "main".to_string(),
        title: None,
        mode: TaskMode::Implement,
        group: None,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: Some(CodexConfig {
            prompt: "test".to_string(),
            output_last_message: None,
            verify_after: false,
            sandbox: None,
            ask_for_approval: None,
            json: true,
        }),
        claude_code: None,
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        auto_commit: true,
        squash: false,
        condition: Some(TaskCondition::TaskResult(TaskResultCondition {
            task: "main".to_string(),
            status: ConditionStatus::Failed,
        })),
    };

    let plan = Plan {
        version: 1,
        run: RunConfig::default(),
        variables: HashMap::new(),
        groups: HashMap::new(),
        tasks: vec![task],
    };

    let err = plan.validate().expect_err("expected validation error");
    let msg = err.to_string();
    assert!(
        msg.contains("condition cannot reference itself"),
        "unexpected error: {msg}"
    );
}
