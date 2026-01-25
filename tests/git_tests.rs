#[test]
fn test_auto_commit_enabled_by_default() {
    // Test that auto_commit defaults to true for implement mode
    let task_json = r#"{
        "id": "task-1",
        "mode": "implement",
        "codex": { "prompt": "test" }
    }"#;

    let task: quedex::plan::Task = serde_json::from_str(task_json).unwrap();
    assert!(task.auto_commit);
}

#[test]
fn test_auto_commit_can_be_disabled() {
    // Test that auto_commit can be set to false
    let task_json = r#"{
        "id": "task-1",
        "mode": "implement",
        "auto_commit": false,
        "codex": { "prompt": "test" }
    }"#;

    let task: quedex::plan::Task = serde_json::from_str(task_json).unwrap();
    assert!(!task.auto_commit);
}

#[test]
fn test_squash_field_parsing() {
    // Test that squash field can be parsed
    let task_json = r#"{
        "id": "squash-task",
        "mode": "verify",
        "squash": true,
        "codex": { "prompt": "final review" }
    }"#;

    let task: quedex::plan::Task = serde_json::from_str(task_json).unwrap();
    assert!(task.squash);
}

#[test]
fn test_commit_message_generation() {
    // Test commit message generation function
    let msg = quedex::git::generate_commit_message(
        "Implement authentication",
        "task-1",
        "implement"
    );

    assert!(msg.contains("feat: Implement authentication [task-1]"));
    assert!(msg.contains("Task ID: task-1"));
}

#[test]
fn test_squash_message_generation() {
    // Test squash message generation
    let summaries = vec![
        ("task-1".to_string(), "API実装".to_string()),
        ("task-2".to_string(), "テスト作成".to_string()),
    ];

    let msg = quedex::git::generate_squash_message(&summaries, "最終統合");

    assert!(msg.contains("feat/integration: 最終統合"));
    assert!(msg.contains("task-1: API実装"));
    assert!(msg.contains("task-2: テスト作成"));
}

#[test]
fn test_task_json_parsing() {
    // Test that task JSON can be parsed with git fields
    let task_json = r#"{
        "id": "task-1",
        "mode": "implement",
        "auto_commit": true,
        "squash": false,
        "codex": { "prompt": "test" }
    }"#;

    let task: quedex::plan::Task = serde_json::from_str(task_json).unwrap();
    assert_eq!(task.id, "task-1");
    assert!(task.auto_commit);
    assert!(!task.squash);
}

#[test]
fn test_auto_commit_true_by_default() {
    // Verify auto_commit defaults to true
    let task_json = r#"{
        "id": "task-1",
        "mode": "implement",
        "codex": { "prompt": "test" }
    }"#;

    let task: quedex::plan::Task = serde_json::from_str(task_json).unwrap();
    assert!(task.auto_commit, "auto_commit should default to true for implement mode");
}