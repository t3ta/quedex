use std::collections::HashMap;
use quedex::plan::{ClaudeCodeConfig, CodexConfig, OpencodeConfig, Plan, RunConfig, Task, TaskMode};

fn codex_task(id: &str, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: Some(CodexConfig {
            prompt: "test prompt".to_string(),
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
        condition: None,
    }
}

#[allow(dead_code)]
fn opencode_task(id: &str, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: "test prompt".to_string(),
            model: None,
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    }
}

fn plan_with_tasks(tasks: Vec<Task>) -> Plan {
    Plan {
        version: 1,
        run: RunConfig::default(),
        variables: HashMap::new(),
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
    let tasks = vec![codex_task("dup", vec![]), codex_task("dup", vec![])];
    let plan = plan_with_tasks(tasks);
    assert_validation_error(plan, "duplicate task id");
}

#[test]
fn plan_rejects_missing_deps() {
    let tasks = vec![codex_task("A", vec!["B"])];
    let plan = plan_with_tasks(tasks);
    assert_validation_error(plan, "missing dep");
}

#[test]
fn plan_rejects_dependency_cycles() {
    let tasks = vec![codex_task("A", vec!["B"]), codex_task("B", vec!["A"])];
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
        no_worktree: false,
        kind: None,
        codex: Some(CodexConfig {
            prompt: " ".to_string(),
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
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "codex.prompt is empty");
}

#[test]
fn plan_rejects_empty_claude_code_prompt() {
    let task = Task {
        id: "claude".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: None,
        claude_code: Some(ClaudeCodeConfig {
            prompt: " ".to_string(),
            model: None,
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "claude_code.prompt is empty");
}

#[test]
fn plan_rejects_multiple_runner_configs() {
    let task = Task {
        id: "multi".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: Some(CodexConfig {
            prompt: "test".to_string(),
            output_last_message: None,
            verify_after: false,
            sandbox: None,
            ask_for_approval: None,
            json: true,
        }),
        claude_code: Some(ClaudeCodeConfig {
            prompt: "test".to_string(),
            model: None,
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "multiple runner configs");
}

#[test]
fn plan_accepts_valid_claude_code_task() {
    let task = Task {
        id: "claude".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: Some("claude_code".to_string()),
        codex: None,
        claude_code: Some(ClaudeCodeConfig {
            prompt: "implement feature".to_string(),
            model: Some("sonnet".to_string()),
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert!(plan.validate().is_ok());
}

#[test]
fn plan_rejects_kind_mismatch_claude_code() {
    let task = Task {
        id: "mismatch".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: Some("claude_code".to_string()),
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
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "kind=claude_code without claude_code config");
}

#[test]
fn plan_rejects_empty_opencode_prompt() {
    let task = Task {
        id: "opencode".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: " ".to_string(),
            model: None,
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "opencode.prompt is empty");
}

#[test]
fn plan_accepts_valid_opencode_task() {
    let task = Task {
        id: "opencode".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: Some("opencode".to_string()),
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: "implement feature".to_string(),
            model: Some("anthropic/claude-sonnet".to_string()),
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert!(plan.validate().is_ok());
}

#[test]
fn plan_rejects_kind_mismatch_opencode() {
    let task = Task {
        id: "mismatch".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: Some("opencode".to_string()),
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
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "kind=opencode without opencode config");
}

#[test]
fn plan_rejects_multiple_runner_configs_with_opencode() {
    let task = Task {
        id: "multi".to_string(),
        title: None,
        mode: TaskMode::Implement,
        deps: vec![],
        locks: vec![],
        timeout_sec: None,
        no_worktree: false,
        kind: None,
        codex: None,
        claude_code: Some(ClaudeCodeConfig {
            prompt: "test".to_string(),
            model: None,
            json: true,
        }),
        opencode: Some(OpencodeConfig {
            prompt: "test".to_string(),
            model: None,
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        condition: None,
    };

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "multiple runner configs");
}
