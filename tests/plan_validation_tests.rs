use quedex::plan::{CodexConfig, Plan, RunConfig, ShellConfig, Task, TaskMode};

fn shell_task(id: &str, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        timeout_sec: None,
        kind: None,
        codex: None,
        shell: Some(ShellConfig {
            command: "echo ok".to_string(),
        }),
    }
}

fn plan_with_tasks(tasks: Vec<Task>) -> Plan {
    Plan {
        version: 1,
        run: RunConfig::default(),
        tasks,
    }
}

fn assert_validation_error(plan: Plan, expected: &str) {
    let err = plan.validate().expect_err("expected validation error");
    let msg = err.to_string();
    assert!(msg.contains(expected), "unexpected error: {msg}");
}

#[test]
fn plan_rejects_duplicate_ids() {
    let tasks = vec![shell_task("dup", vec![]), shell_task("dup", vec![])];
    let plan = plan_with_tasks(tasks);
    assert_validation_error(plan, "duplicate task id");
}

#[test]
fn plan_rejects_missing_deps() {
    let tasks = vec![shell_task("A", vec!["B"])];
    let plan = plan_with_tasks(tasks);
    assert_validation_error(plan, "missing dep");
}

#[test]
fn plan_rejects_dependency_cycles() {
    let tasks = vec![shell_task("A", vec!["B"]), shell_task("B", vec!["A"])];
    let plan = plan_with_tasks(tasks);
    assert_validation_error(plan, "dependency cycle");
}

#[test]
fn plan_rejects_empty_codex_prompt() {
    let task = Task {
        id: "codex".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        kind: None,
        codex: Some(CodexConfig {
            prompt: " ".to_string(),
            output_last_message: None,
            verify_after: false,
            sandbox: None,
            ask_for_approval: None,
        }),
        shell: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "codex.prompt is empty");
}
