# Stall Detection 実装調査

調査日: 2026-03-14

---

## 1. タスク実行ループの現在の構造

### spawn_blocking + child.wait のフロー（src/main.rs）

```
PlanTaskRunner::spawn() の retry ループ内:

  1. runner に応じてプロセスを起動
     let child = codex.spawn(...) / claude_code.spawn(...) / opencode.spawn(...)

  2. キャンセル登録 & 開始イベント記録
     cancel.register(&task_id, child.clone());
     state.task_started(&task_id, child.pid);

  3. spawn_blocking で child.wait() を実行（← ここが無期限ブロック）
     let wait_future = tokio::task::spawn_blocking(move || child.wait());
     let wait_result = wait_future.await;   // ← タイムアウトなし

  4. キャンセル登録解除
     cancel.unregister(&task_id);

  5. 終了ステータスを判定
     let status = match wait_result { Ok(Ok(s)) => s, ... };
     let result = map_exit_status(status, cancel.is_canceled());

  6. retry ロジック
     if result.status == TaskStatus::Failed && attempt < max_attempts { continue; }

  7. 完了記録
     state.task_finished(&task_id, result.status, result.exit_code);
     break result;
```

**現在の問題点:**
- `child.wait()` はプロセスが終了するまで無期限に待機する
- LLM エージェントがハング（無限ループ・レートリミット・ネットワーク障害等）した場合、
  依存タスクも永久にブロックされる
- fail_fast モードでもプロセスが終了するまで stall を検知できない
- キャンセル操作は外部から明示的に呼ばれた場合のみ機能し、自動検知はない

### ChildHandle の構造（src/runner/mod.rs）

```rust
#[derive(Clone)]
pub struct ChildHandle {
    pub pid: u32,
    pub stdout_path: PathBuf,   // パブリックフィールド: ファイルサイズ監視に使用可能
    pub stderr_path: PathBuf,   // パブリックフィールド: ファイルサイズ監視に使用可能
    child: Arc<Mutex<Child>>,   // Arc<Mutex> でスレッドセーフ
}

// kill(): SIGKILL を送信
pub fn kill(&self) -> Result<()> {
    let mut child = self.child.lock()?;
    child.kill().context("kill child process")?;
    Ok(())
}

// wait(): ブロッキング待機
pub fn wait(&self) -> Result<std::process::ExitStatus> {
    let mut child = self.child.lock()?;
    let status = child.wait().context("wait child process")?;
    Ok(status)
}
```

`stdout_path` / `stderr_path` はパブリックフィールドのため、
`std::fs::metadata()` でファイルサイズを直接取得可能。

### Event enum の現状（src/store/mod.rs）

```rust
pub enum Event {
    RunStarted {
        run_id: String,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskStarted {
        task_id: String,
        pid: u32,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskExited {
        task_id: String,
        #[serde(rename = "code")]
        exit_code: i32,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    TaskCanceled {
        task_id: String,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    // ← ここに TaskStalled を追加予定
}
```

### TaskState の現状（src/store/mod.rs）

```rust
pub struct TaskState {
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub stderr_tail: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output_files: Option<Vec<String>>,
    pub pid: Option<u32>,
    pub skip_reason: Option<SkipReason>,
    // ← stalled: bool を追加予定
}
```

### RunConfig の現状（src/plan.rs）

```rust
pub struct RunConfig {
    pub name: Option<String>,
    pub cwd: Option<PathBuf>,
    pub worktree: Option<WorktreeRunConfig>,
    pub env: Option<HashMap<String, String>>,
    pub max_concurrency: Option<usize>,
    pub fail_fast: Option<bool>,
    pub notifications: Option<NotificationConfig>,
    pub system_prompt: Option<String>,
    // ← stall_timeout_sec: Option<u64> を追加予定
}
```

**注意:** `timeout_sec` は削除済み（reject 機構により拒否）。
`stall_timeout_sec` は「出力なし期間の検出」という異なる概念で新規追加する。

### Task の現状（src/plan.rs）

```rust
pub struct Task {
    pub id: String,
    pub title: Option<String>,
    pub mode: TaskMode,
    pub profile: Option<String>,
    pub group: Option<String>,
    pub deps: Vec<String>,
    pub locks: Vec<String>,
    pub no_worktree: bool,
    pub kind: Option<String>,
    pub output_files: Option<Vec<String>>,
    pub codex: Option<CodexConfig>,
    pub claude_code: Option<ClaudeCodeConfig>,
    pub opencode: Option<OpencodeConfig>,
    pub retry_count: u32,
    pub retry_delay_sec: u64,
    pub retry_strategy: Option<RetryStrategy>,
    pub context: Option<ContextConfig>,
    pub condition: Option<TaskCondition>,
    pub auto_commit: bool,
    pub squash: bool,
    // ← stall_timeout_sec: Option<u64> を追加予定
}
```

---

## 2. 変更方針

### 2.1 RunConfig に `stall_timeout_sec: Option<u64>` を追加（src/plan.rs）

```rust
/// Stall detection timeout in seconds (run-level default).
/// If a task produces no stdout/stderr output for this many seconds, the process is killed.
/// Set to 0 to disable stall detection. None uses the compiled-in default (300s).
#[serde(default)]
pub stall_timeout_sec: Option<u64>,
```

- `None` → デフォルト値 300 秒として動作
- `Some(0)` → 全タスクで stall 検出を無効化
- `Some(n)` → n 秒を RunConfig レベルのデフォルトとして使用

### 2.2 Task に `stall_timeout_sec: Option<u64>` を追加（src/plan.rs）

```rust
/// Per-task stall detection timeout in seconds.
/// Overrides RunConfig.stall_timeout_sec for this specific task.
/// Set to 0 to disable stall detection for this task.
/// None falls back to RunConfig.stall_timeout_sec.
#[serde(default)]
pub stall_timeout_sec: Option<u64>,
```

**フォールバック優先度:**
```
Task.stall_timeout_sec
  → RunConfig.stall_timeout_sec
    → ハードコードデフォルト (300)
```

`Some(0)` は「このタスクのみ無効化」を意味する。

### 2.3 `Event::TaskStalled` バリアントを追加（src/store/mod.rs）

```rust
TaskStalled {
    task_id: String,
    stall_timeout_sec: u64,
    #[serde(rename = "ts")]
    timestamp: DateTime<Utc>,
},
```

### 2.4 `TaskState.stalled: bool` フィールドを追加（src/store/mod.rs）

```rust
/// True if this task was killed by stall detection
#[serde(default)]
pub stalled: bool,
```

### 2.5 PlanTaskRunner::spawn() 内に stall monitor を追加（src/main.rs）

`child.wait()` の前後を以下の構造に置き換える:

```rust
// --- stall timeout の解決 ---
let effective_stall_timeout: u64 = task.stall_timeout_sec
    .or(run_config.stall_timeout_sec)
    .unwrap_or(300);
let stall_enabled = effective_stall_timeout > 0;

// --- child.wait() を spawn_blocking で起動 ---
let child_for_wait = child.clone();
let wait_future = tokio::task::spawn_blocking(move || child_for_wait.wait());

// --- stall monitor を tokio::spawn で起動 ---
let stalled_flag = Arc::new(AtomicBool::new(false));
let stall_handle = if stall_enabled {
    let child_for_stall = child.clone();
    let stalled = stalled_flag.clone();
    let task_id_s = task_id.clone();
    let timeout_secs = effective_stall_timeout;

    Some(tokio::spawn(async move {
        let poll = Duration::from_secs(1);
        let threshold = Duration::from_secs(timeout_secs);
        let mut last_size = get_output_size(&child_for_stall);
        let mut idle = Duration::ZERO;

        loop {
            tokio::time::sleep(poll).await;
            let cur = get_output_size(&child_for_stall);
            if cur != last_size {
                last_size = cur;
                idle = Duration::ZERO;
            } else {
                idle += poll;
                if idle >= threshold {
                    stalled.store(true, Ordering::SeqCst);
                    let _ = child_for_stall.kill();
                    tracing::warn!(
                        task_id = %task_id_s,
                        "stall detected: no output for {}s, process killed",
                        timeout_secs,
                    );
                    break;
                }
            }
        }
    }))
} else {
    None
};

// --- wait_future を await ---
let wait_result = wait_future.await;

// stall monitor を停止
if let Some(h) = stall_handle {
    h.abort();
}
cancel.unregister(&task_id);

// --- stall の場合は TaskStalled イベントを記録 ---
if stalled_flag.load(Ordering::SeqCst) {
    store.append_event(Event::TaskStalled {
        task_id: task_id.clone(),
        stall_timeout_sec: effective_stall_timeout,
        timestamp: Utc::now(),
    }).await?;
    // stderr 末尾にメッセージ追記
    append_to_stderr(&task_ctx, &format!(
        "\n[quedex] stall detected: no output for {}s, process killed\n",
        effective_stall_timeout
    ));
    // TaskState.stalled = true を設定
    state.set_stalled(&task_id).await;
}
```

**ファイルサイズ取得ヘルパー:**

```rust
fn get_output_size(child: &ChildHandle) -> u64 {
    let stdout = std::fs::metadata(&child.stdout_path).map(|m| m.len()).unwrap_or(0);
    let stderr = std::fs::metadata(&child.stderr_path).map(|m| m.len()).unwrap_or(0);
    stdout + stderr
}
```

### 2.6 タイムアウト時の処理フロー

1. stall monitor が timeout を検知 → `stalled_flag = true`, `child.kill()` 呼び出し
2. `child.wait()` が SIGKILL による終了ステータスを返す（非ゼロ）
3. `map_exit_status()` により `TaskStatus::Failed` に分類
4. `Event::TaskStalled` を store に記録
5. `TaskState.stalled = true` を設定
6. stderr 末尾に `[quedex] stall detected: no output for Ns, process killed` を追記
7. 既存の retry ループ（`retry_count`, `retry_strategy`）により retry 対象として扱われる

---

## 3. 注意点

### 3.1 cancel 操作との競合

**問題:** stall monitor の `child.kill()` と cancel 操作の `child.kill()` が同時に呼ばれる可能性がある。

**安全性:**
- `ChildHandle.kill()` は `Arc<Mutex<Child>>` で保護されているためスレッドセーフ
- 既に終了したプロセスへの kill() は無視されるため、二重 kill は問題なし

**イベント重複対策:**
- `Arc<AtomicBool> stalled_flag` で「stall kill 済み」を管理
- `map_exit_status(status, cancel.is_canceled())` の後で `stalled_flag` をチェックし、
  cancel と stall の両方が真の場合は cancel を優先（`TaskCanceled` を記録）

```
競合シナリオ:
  stall monitor → kill()  ─┐
  cancel 操作   → kill()  ─┘─→ wait() が非ゼロで一度だけ返る
                              → cancel.is_canceled() == true なら TaskCanceled
                              → stalled_flag == true のみなら TaskStalled
```

### 3.2 retry との統合

- stall で失敗したタスクは基本的に retry 対象
- SIGKILL による終了は signal 終了（exit_code = null/-1）として返る
  → `map_exit_status()` が既に signal 終了を `Failed` に変換する設計であることを確認
- `retry_strategy.skip_permanent_failures` との干渉:
  - stall は permanent failure ではないため `Transient` として扱う
  - `stalled_flag` を使って `FailureType::classify()` が正しく Transient を返すよう調整
- retry ループの各 attempt で stall monitor は新規に `tokio::spawn` されるため、
  タイマーは自動的にリセットされる（設計上問題なし）

### 3.3 stall monitor の abort 漏れ防止

- `wait_future` が正常終了した場合、`stall_handle.abort()` を確実に呼ぶこと
- tokio の `JoinHandle::abort()` は Drop 時に自動 abort しないため、明示的に呼ぶ必要がある
- またはラッパー構造体で Drop 時に abort する実装も選択肢

### 3.4 ファイルサイズ監視の限界と代替案

- stdout/stderr がカーネルバッファにあって OS のファイルに書き込まれていない場合、
  短時間の遅れが生じることがある
- LLM エージェント（Codex CLI 等）は通常 line-buffered で動くため実用上は問題ない
- `mtime`（更新日時）を使う方法も選択肢だが、サイズの方が信頼性が高い
- ポーリング間隔は 1 秒を推奨（`specs/proposal-stall-detection.md` では 5 秒を提案しているが、
  1 秒の方がタイムアウト精度が高く CPU 負荷も軽微）

### 3.5 後方互換性

- `stall_timeout_sec` は `#[serde(default)]` のため、既存の plan.yaml はそのまま動作する
- `TaskState.stalled` は `#[serde(default)]` により既存のイベントログも正しくデシリアライズされる
- デフォルト 300 秒は保守的な値であり、既存の短時間タスクに影響しない

---

## 4. 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `src/plan.rs` | `RunConfig.stall_timeout_sec`, `Task.stall_timeout_sec` 追加 |
| `src/store/mod.rs` | `Event::TaskStalled` バリアント追加、`TaskState.stalled: bool` 追加 |
| `src/main.rs` | stall monitor の `tokio::spawn` + `Arc<AtomicBool>` + `get_output_size()` 追加 |
| `src/runner/mod.rs` | **変更不要**（`kill()`, `stdout_path`, `stderr_path` は既存パブリックフィールド） |
| `src/tui/ui.rs` | stalled 状態の表示（`[STALLED]` ラベル等）（オプション） |

---

## 5. 参照

- `specs/proposal-stall-detection.md`: 設計提案元（ポーリング間隔5秒の提案あり）
- `src/runner/mod.rs`: `ChildHandle.kill()`, `.stdout_path`, `.stderr_path` が利用可能
- `src/store/mod.rs`: 既存バリアント構造に準拠して `TaskStalled` を追加
- `src/main.rs`: 既存の cancel 競合対策（`Arc<AtomicBool>`）を stall にも適用
