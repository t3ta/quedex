use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Worktree 設定
#[derive(Debug, Clone, Default)]
pub struct WorktreeConfig {
    pub enabled: bool,
    pub base_dir: Option<PathBuf>,
    pub shallow_depth: Option<u32>,
}

/// Worktree インスタンス（RAII パターン）
pub struct Worktree {
    path: PathBuf,
    source_repo: PathBuf,
    task_id: String,
    auto_cleanup: bool,
}

impl Worktree {
    /// 新しい worktree を作成
    pub fn create(source_repo: &Path, task_id: &str, config: &WorktreeConfig) -> Result<Self> {
        let base_dir = config
            .base_dir
            .clone()
            .unwrap_or_else(|| source_repo.join(".quedex/worktrees"));
        std::fs::create_dir_all(&base_dir)?;

        let worktree_path = base_dir.join(format!("task-{task_id}"));

        // git worktree add
        let status = Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&worktree_path)
            .arg("HEAD")
            .current_dir(source_repo)
            .status()
            .context("Failed to execute git worktree add")?;

        if !status.success() {
            anyhow::bail!("git worktree add failed with status: {status}");
        }

        Ok(Self {
            path: worktree_path,
            source_repo: source_repo.to_path_buf(),
            task_id: task_id.to_string(),
            auto_cleanup: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// 明示的にクリーンアップ
    pub fn cleanup(mut self) -> Result<()> {
        self.auto_cleanup = false;
        self.do_cleanup()
    }

    /// クリーンアップをスキップ（デバッグ用）
    pub fn detach(mut self) {
        self.auto_cleanup = false;
    }

    /// タスク成功時に変更を patch として保存
    pub fn save_patch(&self, patches_dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(patches_dir)?;
        let patch_path = patches_dir.join(format!("{}.patch", self.task_id));

        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&self.path)
            .output()
            .context("Failed to generate diff")?;

        std::fs::write(&patch_path, &output.stdout)?;
        Ok(patch_path)
    }

    fn do_cleanup(&self) -> Result<()> {
        let status = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.source_repo)
            .status()
            .context("Failed to execute git worktree remove")?;

        if !status.success() {
            anyhow::bail!("git worktree remove failed");
        }
        Ok(())
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if self.auto_cleanup {
            let _ = self.do_cleanup();
        }
    }
}

pub mod manager;
