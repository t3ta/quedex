use anyhow::{Context, Result, anyhow};
use git2::{Repository, StatusOptions};

/// Manager for git operations
pub struct GitManager {
    repo: Repository,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    pub summary: String,
}

impl GitManager {
    /// Open git repository at current working directory
    pub fn open() -> Result<Self> {
        let repo = Repository::discover(".")
            .context("failed to open git repository. Ensure you are inside a git repository.")?;
        Ok(Self { repo })
    }

    /// Open git repository at the given path (useful for worktree directories)
    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        let repo = Repository::discover(path)
            .with_context(|| format!("failed to open git repository at {}", path.display()))?;
        Ok(Self { repo })
    }

    /// Create a GitManager from an existing Repository (for testing)
    pub fn from_repo(repo: Repository) -> Self {
        Self { repo }
    }

    /// Ensure we are on a valid branch (not detached HEAD)
    pub fn ensure_on_branch(&self) -> Result<()> {
        let head = self.repo.head()?;
        if head.shorthand().is_none() {
            // If no shorthand like "main", "develop", it's detached head (pointing to a commit)
            anyhow::bail!(
                "Git HEAD is detached. Please switch to a branch first (e.g., git checkout -b feature-branch)"
            );
        }
        Ok(())
    }

    /// Create a commit with the given message
    /// Returns the commit hash
    pub fn create_commit(&self, message: &str) -> Result<String> {
        // Add all changes to staging
        let mut index = self.repo.index()?;

        // Get current status and stage all modified files
        let mut status_opts = StatusOptions::new();
        status_opts.include_untracked(true);
        let statuses = self.repo.statuses(Some(&mut status_opts))?;

        for status in statuses.iter() {
            if let Some(path) = status.path() {
                // Add the file to index
                let s = status.status();
                if s.intersects(git2::Status::WT_MODIFIED)
                    || s.intersects(git2::Status::WT_NEW)
                    || s.intersects(git2::Status::WT_RENAMED)
                {
                    if let Err(e) = index.add_path(std::path::Path::new(path)) {
                        eprintln!("Warning: Could not stage {:?}: {}", path, e);
                    }
                } else if s.intersects(git2::Status::WT_DELETED) {
                    if let Err(e) = index.remove_path(std::path::Path::new(path)) {
                        eprintln!("Warning: Could not unstage {:?}: {}", path, e);
                    }
                }
            }
        }

        // Write tree for current index state
        let tree_id = index.write_tree()?;

        // Get HEAD commit and commit parents
        let head = self.repo.head()?;
        let parent_commit = head.peel_to_commit()?;

        // Check if there are any changes to commit by comparing index tree with HEAD tree
        if tree_id == parent_commit.tree_id() {
            return Ok(String::new()); // No changes, return empty
        }

        // Load tree for committing
        let tree = self.repo.find_tree(tree_id)?;

        // Get author signature (use git config if available)
        let signature = self.repo.signature()?;

        // Create commit
        let commit_id = self.repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent_commit],
        )?;

        Ok(commit_id.to_string())
    }

    /// List recent commits
    pub fn list_commits(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let mut revwalk = self.repo.revwalk()?;
        revwalk.push_head()?;

        let mut commits = Vec::new();
        for (i, commit_id) in revwalk.enumerate() {
            if i >= limit {
                break;
            }

            let commit_id = commit_id?;
            let commit = self.repo.find_commit(commit_id)?;

            let message = commit.message().unwrap_or("").to_string();
            let summary = message.lines().next().unwrap_or("").to_string();

            commits.push(CommitInfo {
                id: commit_id.to_string(),
                message,
                summary,
            });
        }

        Ok(commits)
    }

    /// Squash multiple commits into one
    /// Takes the last `count` commits and squashes them into a single commit
    pub fn squash_commits(&self, count: usize, message: &str) -> Result<String> {
        if count == 0 {
            anyhow::bail!("count must be greater than 0");
        }

        if count <= 1 {
            // No need to squash if only 1 commit
            let mut revwalk = self.repo.revwalk()?;
            revwalk.push_head()?;
            let first_commit_id = revwalk
                .next()
                .ok_or_else(|| anyhow!("no commits in repository"))??;
            Ok(first_commit_id.to_string())
        } else {
            // Get HEAD commit
            let mut revwalk = self.repo.revwalk()?;
            revwalk.push_head()?;
            let head_commit_id = revwalk.next().ok_or_else(|| anyhow!("no commits"))??;
            let head_commit = self.repo.find_commit(head_commit_id)?;

            // Skip count commits to find the parent of the squash range
            // If we want to squash 3 commits (HEAD, HEAD~1, HEAD~2), the new parent is HEAD~3
            let mut skip_count = count;
            let mut squash_parent_id = head_commit_id;
            while skip_count > 0 {
                squash_parent_id = revwalk
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("not enough commits to squash"))??;
                skip_count -= 1;
            }
            let squash_parent = self.repo.find_commit(squash_parent_id)?;

            // Use the same tree as HEAD
            let tree = head_commit.tree()?;

            let signature = self.repo.signature()?;

            // Create the squashed commit without updating HEAD first
            let commit_id = self.repo.commit(
                None, // Don't update any reference yet
                &signature,
                &signature,
                message,
                &tree,
                &[&squash_parent],
            )?;

            // Now update HEAD to point to the new commit
            let commit = self.repo.find_commit(commit_id)?;
            self.repo
                .head()?
                .set_target(commit_id, &format!("squash: {}", message))?;

            // Reset the working tree and index to match the new HEAD
            self.repo
                .reset(commit.as_object(), git2::ResetType::Hard, None)?;

            Ok(commit_id.to_string())
        }
    }
}

/// Generate commit message for a task
///
/// Format: `{mode}: {title} [{id}]\n\n{description}`
pub fn generate_commit_message(title: &str, task_id: &str, mode: &str) -> String {
    let mode_str = match mode {
        "implement" | "Implement" => "feat",
        "verify" | "Verify" => "chore",
        _ => "chore",
    };

    format!(
        "{}: {} [{}]\n\nTask ID: {}\nExecuted by quedex",
        mode_str, title, task_id, task_id
    )
}

/// Generate squash commit message from multiple task commits
pub fn generate_squash_message(
    task_summaries: &[(String, String)],
    integration_task_title: &str,
) -> String {
    let mut message = format!("feat/integration: {}\n\n", integration_task_title);
    message.push_str("Squashed commits from:\n");

    for (task_id, title) in task_summaries {
        message.push_str(&format!("- {}: {}\n", task_id, title));
    }

    message.push_str("\nThis commit consolidates all changes made by the individual tasks above.");
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_message_generation() {
        let msg = generate_commit_message("Implement auth feature", "task-1", "implement");
        assert!(msg.contains("feat: Implement auth feature [task-1]"));
        assert!(msg.contains("Task ID: task-1"));
    }

    #[test]
    fn test_git_manager_creation() {
        // This test requires being in a git repository
        if let Ok(manager) = GitManager::open() {
            assert!(manager.ensure_on_branch().is_ok());
        }
    }
}
