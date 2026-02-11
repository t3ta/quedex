use std::collections::HashMap;
use quedex::plan::{ClaudeCodeConfig, CodexConfig, OpencodeConfig, Plan, RunConfig, Task, TaskMode};

fn codex_task(id: &str, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        profile: None,
        group: None,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
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
        retry_strategy: None,
        context: None,
        condition: None,
        auto_commit: true,
        squash: false,
}
}

#[allow(dead_code)]
fn opencode_task(id: &str, deps: Vec<&str>) -> Task {
    Task {
        id: id.to_string(),
        title: None,
        mode: TaskMode::Implement,
        profile: None,
        group: None,
        deps: deps.into_iter().map(|dep| dep.to_string()).collect(),
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: "test prompt".to_string(),
            model: None,
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
        auto_commit: true,
        squash: false,
}
}

fn plan_with_tasks(tasks: Vec<Task>) -> Plan {
    Plan {
        version: 1,
        run: RunConfig::default(),
        profiles: HashMap::new(),
        groups: HashMap::new(),
        tasks,
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
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
        retry_strategy: None,
        context: None,
        condition: None,
        auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: None,
        claude_code: Some(ClaudeCodeConfig {
            prompt: " ".to_string(),
            model: None,
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
        auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
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
        claude_code: Some(ClaudeCodeConfig {
            prompt: "test".to_string(),
            model: None,
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: Some("claude_code".to_string()),
        output_files: None,
        codex: None,
        claude_code: Some(ClaudeCodeConfig {
            prompt: "implement feature".to_string(),
            model: Some("sonnet".to_string()),
            json: true,
        }),
        opencode: None,
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: Some("claude_code".to_string()),
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
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: " ".to_string(),
            model: None,
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
};

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "opencode.prompt is empty");
}

#[test]
fn plan_rejects_empty_output_files() {
    let mut task = codex_task("output", vec![]);
    task.output_files = Some(vec![]);

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "output_files is empty");
}

#[test]
fn plan_rejects_blank_output_file_path() {
    let mut task = codex_task("output", vec![]);
    task.output_files = Some(vec!["  ".to_string()]);

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "output_files contains empty path");
}

#[test]
fn plan_accepts_valid_opencode_task() {
    let task = Task {
        id: "opencode".to_string(),
        title: None,
        mode: TaskMode::Implement,
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: Some("opencode".to_string()),
        output_files: None,
        codex: None,
        claude_code: None,
        opencode: Some(OpencodeConfig {
            prompt: "implement feature".to_string(),
            model: Some("anthropic/claude-sonnet".to_string()),
            json: true,
        }),
        retry_count: 0,
        retry_delay_sec: 0,
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: Some("opencode".to_string()),
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
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
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
        profile: None,
        group: None,
        deps: vec![],
        locks: vec![],
        _timeout_sec_rejected: None,
        _default_timeout_sec_rejected: None,
        no_worktree: false,
        kind: None,
        output_files: None,
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
        retry_strategy: None,
        context: None,
        condition: None,
            auto_commit: true,
        squash: false,
};

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "multiple runner configs");
}

#[test]
fn plan_rejects_absolute_path_in_output_files() {
    let mut task = codex_task("test", vec![]);
    task.output_files = Some(vec!["/tmp/result.txt".to_string()]);

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "contains absolute path");
}

#[test]
fn plan_rejects_parent_dir_in_output_files() {
    let mut task = codex_task("test", vec![]);
    task.output_files = Some(vec!["../out.txt".to_string()]);

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "contains '..'");
}

#[test]
fn plan_accepts_system_prompt_in_run_config() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
run:
  system_prompt: |
    This is a test project.
    Please follow coding conventions.
tasks:
  - id: test
    mode: implement
    codex:
      prompt: "implement feature"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());
    assert!(plan.run.system_prompt.is_some());
    let sys_prompt = plan.run.system_prompt.unwrap();
    assert!(sys_prompt.contains("test project"));
    assert!(sys_prompt.contains("coding conventions"));
}

#[test]
fn plan_accepts_empty_run_config_system_prompt() {
    use quedex::plan::PlanFormat;

    let json = r#"{
        "version": 1,
        "tasks": [
            {
                "id": "test",
                "mode": "implement",
                "codex": {
                    "prompt": "implement feature"
                }
            }
        ]
    }"#;
    let plan = Plan::parse_str(json, PlanFormat::Json).unwrap();
    assert!(plan.validate().is_ok());
    assert!(plan.run.system_prompt.is_none());
}

// --- Agent Role Profiles tests ---

#[test]
fn plan_accepts_valid_profile_reference() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
profiles:
  architect:
    system_prompt: "You are a software architect."
    model: "opus"
tasks:
  - id: design
    mode: research
    profile: architect
    codex:
      prompt: "Design the API"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());
    assert_eq!(plan.profiles.len(), 1);
    let profile = plan.profiles.get("architect").unwrap();
    assert_eq!(profile.system_prompt.as_deref(), Some("You are a software architect."));
    assert_eq!(profile.model.as_deref(), Some("opus"));
    assert_eq!(plan.tasks[0].profile.as_deref(), Some("architect"));
}

#[test]
fn plan_rejects_non_existent_profile_reference() {
    let mut task = codex_task("design", vec![]);
    task.profile = Some("non_existent".to_string());

    let plan = plan_with_tasks(vec![task]);
    assert_validation_error(plan, "references non-existent profile");
}

#[test]
fn plan_accepts_tasks_without_profile() {
    let task = codex_task("impl", vec![]);
    let plan = plan_with_tasks(vec![task]);
    assert!(plan.validate().is_ok());
}

#[test]
fn plan_accepts_profiles_section_without_references() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
profiles:
  architect:
    system_prompt: "You are a software architect."
tasks:
  - id: impl
    mode: implement
    codex:
      prompt: "Implement feature"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());
    assert_eq!(plan.profiles.len(), 1);
}

// --- Adaptive Retry tests ---

#[test]
fn plan_accepts_retry_strategy() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: impl
    mode: implement
    retry_count: 2
    retry_strategy:
      inject_error_context: true
      escalate_model: "opus"
      max_stderr_lines: 30
    claude_code:
      prompt: "Implement feature"
      model: "sonnet"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());
    let strategy = plan.tasks[0].retry_strategy.as_ref().unwrap();
    assert!(strategy.inject_error_context);
    assert_eq!(strategy.escalate_model.as_deref(), Some("opus"));
    assert_eq!(strategy.max_stderr_lines, 30);
}

#[test]
fn plan_accepts_retry_strategy_with_defaults() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: impl
    mode: implement
    retry_count: 1
    retry_strategy:
      inject_error_context: true
    claude_code:
      prompt: "Implement feature"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());
    let strategy = plan.tasks[0].retry_strategy.as_ref().unwrap();
    assert!(strategy.inject_error_context);
    assert!(strategy.escalate_model.is_none());
    assert_eq!(strategy.max_stderr_lines, 50); // default
}

// --- Shared Context Store tests ---

#[test]
fn plan_accepts_context_config() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: research
    mode: research
    context:
      publish:
        key: "auth_analysis"
        source: "artifacts/auth.md"
    codex:
      prompt: "Analyze authentication"
  - id: implement
    mode: implement
    deps: [research]
    context:
      inject:
        - from: "auth_analysis"
          as: "Authentication Analysis"
    codex:
      prompt: "Implement authentication"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    assert!(plan.validate().is_ok());

    let research = &plan.tasks[0];
    let ctx = research.context.as_ref().unwrap();
    let publish = ctx.publish.as_ref().unwrap();
    assert_eq!(publish.key, "auth_analysis");
    assert_eq!(publish.source, "artifacts/auth.md");

    let implement = &plan.tasks[1];
    let ctx = implement.context.as_ref().unwrap();
    let injections = ctx.inject.as_ref().unwrap();
    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].from, "auth_analysis");
    assert_eq!(injections[0].r#as.as_deref(), Some("Authentication Analysis"));
}

#[test]
fn plan_rejects_absolute_path_in_publish_source() {
    use quedex::plan::{PlanFormat};

    let yaml = r#"
version: 1
tasks:
  - id: research
    mode: research
    context:
      publish:
        key: "data"
        source: "/etc/passwd"
    codex:
      prompt: "analyze"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    let err = plan.validate().expect_err("expected validation error");
    assert!(err.to_string().contains("absolute path"), "unexpected: {err}");
}

#[test]
fn plan_rejects_parent_dir_in_publish_source() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: research
    mode: research
    context:
      publish:
        key: "data"
        source: "../../secret.txt"
    codex:
      prompt: "analyze"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    let err = plan.validate().expect_err("expected validation error");
    assert!(err.to_string().contains("'..'"), "unexpected: {err}");
}

#[test]
fn plan_rejects_invalid_publish_key() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: research
    mode: research
    context:
      publish:
        key: "../../escape"
        source: "output.md"
    codex:
      prompt: "analyze"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    let err = plan.validate().expect_err("expected validation error");
    assert!(err.to_string().contains("invalid characters"), "unexpected: {err}");
}

#[test]
fn plan_rejects_invalid_inject_from_key() {
    use quedex::plan::PlanFormat;

    let yaml = r#"
version: 1
tasks:
  - id: impl
    mode: implement
    context:
      inject:
        - from: "../../tasks/other/stderr.log"
    codex:
      prompt: "implement"
"#;
    let plan = Plan::parse_str(yaml, PlanFormat::Yaml).unwrap();
    let err = plan.validate().expect_err("expected validation error");
    assert!(err.to_string().contains("invalid characters"), "unexpected: {err}");
}
