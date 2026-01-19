use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;

use super::{Worktree, WorktreeConfig};

/// worktree の作業ディレクトリ
pub enum TaskWorkdir {
    /// 通常モード: 共有 cwd
    Shared(PathBuf),
    /// Worktree モード: 独立した worktree
    Isolated(Worktree),
}

impl TaskWorkdir {
    pub fn path(&self) -> &Path {
        match self {
            TaskWorkdir::Shared(p) => p,
            TaskWorkdir::Isolated(w) => w.path(),
        }
    }
}

/// Worktree のライフサイクル管理
pub struct WorktreeManager {
    config: WorktreeConfig,
    source_repo: PathBuf,
    patches_dir: PathBuf,
    active: Mutex<HashMap<String, PathBuf>>,
}

impl WorktreeManager {
    pub fn new(source_repo: PathBuf, config: WorktreeConfig) -> Self {
        let patches_dir = source_repo.join(".quedex/patches");
        Self {
            config,
            source_repo,
            patches_dir,
            active: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// タスク用の worktree を取得または作成
    pub fn acquire(&self, task_id: &str, no_worktree: bool) -> Result<TaskWorkdir> {
        if !self.config.enabled || no_worktree {
            return Ok(TaskWorkdir::Shared(self.source_repo.clone()));
        }

        let worktree = Worktree::create(&self.source_repo, task_id, &self.config)?;

        {
            let mut active = self.active.lock().unwrap();
            active.insert(task_id.to_string(), worktree.path().to_path_buf());
        }

        Ok(TaskWorkdir::Isolated(worktree))
    }

    /// タスク完了後のクリーンアップ（成功時）
    pub fn release_success(&self, task_id: &str, workdir: TaskWorkdir) -> Result<()> {
        if let TaskWorkdir::Isolated(worktree) = workdir {
            // patch を保存
            worktree.save_patch(&self.patches_dir)?;
            // クリーンアップ
            worktree.cleanup()?;
        }

        let mut active = self.active.lock().unwrap();
        active.remove(task_id);
        Ok(())
    }

    /// タスク失敗時（worktree を保持）
    pub fn release_failure(&self, _task_id: &str, workdir: TaskWorkdir) {
        if let TaskWorkdir::Isolated(worktree) = workdir {
            // デバッグ用に保持
            worktree.detach();
        }
        // active からは削除しない（調査用）
    }

    /// 全 worktree のクリーンアップ
    pub fn cleanup_all(&self) -> Result<()> {
        let active = self.active.lock().unwrap();
        for (_task_id, path) in active.iter() {
            let _ = std::process::Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(path)
                .current_dir(&self.source_repo)
                .status();
        }
        Ok(())
    }

    pub fn patches_dir(&self) -> &Path {
        &self.patches_dir
    }
}
