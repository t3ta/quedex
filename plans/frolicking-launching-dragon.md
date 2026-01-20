# quedex git worktree サポート実装計画

## 概要

git worktree を使用して各タスクに独立した作業ディレクトリを提供し、並列実行時のファイル競合を解消する。

## 現状の問題

- 全タスクが同じ `cwd` で実行される
- 並列タスクがファイルを変更すると競合が発生
- 特に git 操作（index/HEAD）が並行実行で危険

## 解決策

各タスク実行時に独立した git worktree を作成し、タスク完了後にクリーンアップする。

---

## 実装計画

### Phase 1: 基盤実装

#### 1.1 新規ファイル: `src/worktree/mod.rs`

```rust
pub struct WorktreeConfig {
    pub enabled: bool,
    pub base_dir: Option<PathBuf>,
    pub shallow_depth: Option<u32>,
}

pub struct Worktree {
    path: PathBuf,
    source_repo: PathBuf,
    task_id: String,
    auto_cleanup: bool,
}

impl Worktree {
    pub fn create(source_repo: &Path, task_id: &str, config: &WorktreeConfig) -> Result<Self>;
    pub fn path(&self) -> &Path;
    pub fn cleanup(self) -> Result<()>;
}

impl Drop for Worktree {
    // RAII パターンで自動クリーンアップ
}
```

#### 1.2 `src/plan.rs` 拡張

```rust
// RunConfig に追加
pub struct WorktreeRunConfig {
    pub enabled: bool,
    pub base_dir: Option<PathBuf>,
    pub shallow_depth: Option<u32>,
}

// Task に追加
pub no_worktree: bool,  // タスク単位で無効化
```

### Phase 2: 統合

#### 2.1 新規ファイル: `src/worktree/manager.rs`

```rust
pub struct WorktreeManager {
    config: WorktreeConfig,
    source_repo: PathBuf,
    active: Mutex<HashMap<String, PathBuf>>,
}

impl WorktreeManager {
    pub fn acquire(&self, task_id: &str) -> Result<TaskWorkdir>;
    pub fn release(&self, task_id: &str) -> Result<()>;
    pub fn cleanup_all(&self) -> Result<()>;
}
```

#### 2.2 `src/runner/mod.rs` 拡張

```rust
pub struct RunContext {
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub store: Arc<dyn Store>,
    pub worktree_manager: Option<Arc<WorktreeManager>>,  // 追加
}

pub enum TaskWorkdir {
    Shared(PathBuf),
    Worktree(Worktree),
}
```

#### 2.3 `src/main.rs` 更新

- `handle_run` で WorktreeManager を初期化
- `PlanTaskRunner` で worktree の acquire/release を管理

### Phase 3: テスト

- `tests/worktree_tests.rs` 作成
- 単体テスト: worktree 作成/削除
- 統合テスト: 並列タスクでのファイル競合なし確認

---

## plan.json 例

```json
{
  "version": 1,
  "run": {
    "name": "parallel-impl",
    "cwd": ".",
    "worktree": {
      "enabled": true,
      "base_dir": ".quedex/worktrees",
      "shallow_depth": 1
    },
    "max_concurrency": 4
  },
  "tasks": [
    { "id": "task-a", "codex": { "prompt": "Implement A" } },
    { "id": "task-b", "codex": { "prompt": "Implement B" } },
    {
      "id": "task-merge",
      "deps": ["task-a", "task-b"],
      "no_worktree": true,
      "codex": { "prompt": "Verify changes" }
    }
  ]
}
```

---

## 変更ファイル一覧

| ファイル | 変更内容 |
|----------|----------|
| `src/worktree/mod.rs` | 新規: Worktree, WorktreeConfig |
| `src/worktree/manager.rs` | 新規: WorktreeManager |
| `src/lib.rs` | `pub mod worktree` 追加 |
| `src/plan.rs` | WorktreeRunConfig, Task.no_worktree 追加 |
| `src/runner/mod.rs` | RunContext 拡張, TaskWorkdir 追加 |
| `src/main.rs` | PlanTaskRunner で worktree 管理 |
| `tests/worktree_tests.rs` | 新規: テスト |

---

## 検証方法

1. 単体テスト実行: `cargo test worktree`
2. 統合テスト: 2つのタスクが同時に同じファイルを編集するプランを実行
3. 手動確認: `.quedex/worktrees/` にワークツリーが作成・削除されることを確認

---

## 設計判断

- **マージ戦略**: Codex/Claude による自動マージ
  - 各タスク完了時、worktree の変更を patch ファイル（`.quedex/patches/{task_id}.patch`）として保存
  - 最終マージタスク（`no_worktree: true`）で、Codex/Claude が各 patch を適用し競合を解決
- **失敗時の処理**: worktree をデバッグ用に保持。手動で調査・削除可能
- **クリーンアップ**: `quedex clean` コマンドで孤立 worktree と patch を削除可能にする

---

## マージフロー詳細

```
task-a (worktree-a)  ──成功──> patches/task-a.patch 保存
task-b (worktree-b)  ──成功──> patches/task-b.patch 保存
         │
         ▼
task-merge (no_worktree: true, メインで実行)
  1. patches/task-a.patch を適用
  2. patches/task-b.patch を適用
  3. 競合があれば解決
  4. 結果をコミット
```

### 追加実装: Patch 保存

```rust
// src/worktree/mod.rs に追加
impl Worktree {
    /// タスク成功時に変更を patch として保存
    pub fn save_patch(&self, store: &dyn Store) -> Result<PathBuf> {
        // git diff HEAD > patches/{task_id}.patch
    }
}
```

### plan.json 例（マージタスク付き）

```json
{
  "run": {
    "worktree": { "enabled": true }
  },
  "tasks": [
    { "id": "impl-a", "codex": { "prompt": "Implement A" } },
    { "id": "impl-b", "codex": { "prompt": "Implement B" } },
    {
      "id": "merge-all",
      "deps": ["impl-a", "impl-b"],
      "no_worktree": true,
      "codex": {
        "prompt": "Apply patches from .quedex/patches/*.patch and resolve conflicts"
      }
    }
  ]
}
```
