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

// Integration tests for GitManager operations
// These tests create temporary git repositories to test actual git operations

#[cfg(test)]
mod git_integration_tests {
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use git2::Repository;

    fn create_temp_git_repo() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let repo_path = temp_dir.path().to_path_buf();
        
        // Initialize git repository
        let repo = Repository::init(&repo_path).expect("Failed to init git repo");
        
        // Configure git user for commits
        let mut config = repo.config().expect("Failed to get config");
        config.set_str("user.name", "Test User").expect("Failed to set user.name");
        config.set_str("user.email", "test@example.com").expect("Failed to set user.email");
        
        // Create initial commit
        let sig = repo.signature().expect("Failed to create signature");
        let tree_id = {
            let mut index = repo.index().expect("Failed to get index");
            index.write_tree().expect("Failed to write tree")
        };
        let tree = repo.find_tree(tree_id).expect("Failed to find tree");
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Initial commit",
            &tree,
            &[],
        ).expect("Failed to create initial commit");
        
        (temp_dir, repo_path)
    }

    #[test]
    fn test_git_manager_create_commit() {
        let (_temp_dir, repo_path) = create_temp_git_repo();
        
        // Create a test file
        fs::write(repo_path.join("test.txt"), "test content").expect("Failed to write test file");
        
        // Open GitManager from the specific path
        let repo = Repository::open(&repo_path).expect("Failed to open repo");
        let manager = quedex::git::GitManager::from_repo(repo);
        let commit_hash = manager.create_commit("Test commit message").expect("Failed to create commit");
        
        assert!(!commit_hash.is_empty(), "Commit hash should not be empty");
    }

    #[test]
    fn test_git_manager_create_commit_no_changes() {
        let (_temp_dir, repo_path) = create_temp_git_repo();
        
        // Open GitManager with no new changes
        let repo = Repository::open(&repo_path).expect("Failed to open repo");
        let manager = quedex::git::GitManager::from_repo(repo);
        let commit_hash = manager.create_commit("Empty commit").expect("Should handle no changes");
        
        assert!(commit_hash.is_empty(), "Should return empty string when no changes");
    }

    #[test]
    fn test_git_manager_list_commits() {
        let (_temp_dir, repo_path) = create_temp_git_repo();
        
        let repo = Repository::open(&repo_path).expect("Failed to open repo");
        let manager = quedex::git::GitManager::from_repo(repo);
        let commits = manager.list_commits(5).expect("Failed to list commits");
        
        assert!(!commits.is_empty(), "Should have at least the initial commit");
        assert_eq!(commits[0].summary, "Initial commit");
    }

    #[test]
    fn test_git_manager_squash_commits() {
        let (_temp_dir, repo_path) = create_temp_git_repo();
        
        // Create multiple commits
        for i in 1..=3 {
            fs::write(repo_path.join(format!("file{}.txt", i)), format!("content {}", i))
                .expect("Failed to write file");
            
            // Re-open manager for each commit to ensure clean state
            let repo_inner = Repository::open(&repo_path).expect("Failed to open repo");
            let manager_inner = quedex::git::GitManager::from_repo(repo_inner);
            manager_inner.create_commit(&format!("Commit {}", i)).expect("Failed to create commit");
        }
        
        // Re-open for squash operation
        let repo_final = Repository::open(&repo_path).expect("Failed to open repo");
        let manager_final = quedex::git::GitManager::from_repo(repo_final);
        
        // Squash the last 3 commits
        let squash_hash = manager_final.squash_commits(3, "Squashed commit").expect("Failed to squash commits");
        assert!(!squash_hash.is_empty(), "Squash should return a commit hash");
        
        // Verify there are now 2 commits total (initial + squashed)
        let commits = manager_final.list_commits(10).expect("Failed to list commits");
        assert_eq!(commits.len(), 2, "Should have 2 commits after squash");
        assert_eq!(commits[0].summary, "Squashed commit");
    }
}
