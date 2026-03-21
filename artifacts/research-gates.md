# Research: Completion Gates 実装調査

調査日: 2026-03-14

## 1. タスク完了フローの現在の構造

### PlanTaskRunner::spawn のフロー（main.rs）

```
1. 初期化・前処理
   - タスク取得、プロファイル解決
   - キャンセル確認 → TaskResult::canceled()
   - worktree 取得（no_worktree フラグで制御）
   - context.inject による上流コンテキスト注入（prompt 先頭 prepend）

2. リトライループ（attempt < max_attempts）
   ├─ attempt > 1 の場合:
   │   - retry_strategy.calculate_delay() で backoff + jitter 遅延
   │   - inject_error_context が有効なら stderr を prompt に prepend
   │   - escalate_model が設定されていればモデルをエスカレーション
   ├─ ランナー選択（codex / claude_code / opencode）
   ├─ 子プロセス spawn → キャンセルハンドルに登録
   ├─ task_started イベント記録（PID 付き）
   ├─ wait（spawn_blocking）
   ├─ map_exit_status() で exit code → TaskResult へマッピング  ← ★現在の判定地点
   └─ failure かつ retry 残存 → continue / それ以外 → break

3. break 後の後処理
   - output_files 処理（存在確認・保存）
   - context.publish 処理（成功時のみ）
   - worktree release（成功/失敗で分岐 → release_success 内で auto_commit）
   - notifier 通知
   - TaskResult 返却
```

### map_exit_status（現在）

```rust
fn map_exit_status(status: ExitStatus, cancel: &CancelHandle) -> TaskResult {
    if cancel.is_canceled() { return TaskResult::canceled(); }
    if let Some(sig) = status.signal() {
        if sig == 2 || sig == 15 { return TaskResult::canceled(); }
    }
    if status.success() {
        TaskResult::succeeded()        // exit 0 → Succeeded
    } else {
        let code = status.code().unwrap_or(-1);
        TaskResult::failed(code)       // exit != 0 → Failed
    }
}
```

**現在は exit 0 が即 `TaskResult::succeeded()` になる。**
Completion Gates の挿入ポイントは `exit 0 判定後、break result の前`。

### auto_commit のタイミング

worktree の `manager.release_success()` 内部で auto_commit が実行される。
→ spawn() 内でゲート全通過後に Succeeded を返す設計にすれば、Scheduler 側の変更は不要。

### 現在の Event enum（store/mod.rs）

```rust
pub enum Event {
    RunStarted  { run_id, timestamp },
    TaskStarted { task_id, pid, timestamp },
    TaskExited  { task_id, exit_code, timestamp },
    TaskCanceled{ task_id, timestamp },
}
```

### 現在の TaskState struct（store/mod.rs）

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
}
```

### 現在の Task struct（plan.rs）

```rust
pub struct Task {
    pub id: String,
    pub title: Option<String>,
    pub mode: TaskMode,               // Research / Implement / Verify
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
    pub auto_commit: bool,            // default: true
    pub squash: bool,
}
```

### 現在の RunConfig struct（plan.rs）

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
}
```

### Plan::validate()（plan.rs: 608-892）

主要バリデーション:
- version != 1 でエラー
- tasks 空チェック
- cwd は絶対パスであること
- 全タスク ID の一意性・使用可能文字チェック
- runner config（codex/claude_code/opencode）は 1 つのみ
- output_files・context.publish.source の相対パスチェック
- dependency 存在チェック・cycle 検出（petgraph）
- group / profile 参照チェック

---

## 2. 変更方針

### 2-1. `CompletionGate` struct の追加（plan.rs）

```rust
/// タスク完了後に実行する検証コマンド定義
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompletionGate {
    /// ゲートの識別名（同一タスク内でユニーク）
    pub name: String,
    /// 実行するコマンド（シェル経由: sh -c で実行）
    pub command: String,
    /// タイムアウト秒数（未設定時は 300 秒）
    #[serde(default)]
    pub timeout_sec: Option<u64>,
}
```

**追加場所**: plan.rs の既存 config struct 群（`RetryStrategy` の近傍）

### 2-2. `RunConfig` への追加（plan.rs）

```rust
pub struct RunConfig {
    // ... 既存フィールド ...
    /// Implement/Verify モードのタスクに自動適用するデフォルトゲート
    /// Research モードには適用されない
    #[serde(default)]
    pub default_gates: Option<Vec<CompletionGate>>,
}
```

**挿入位置**: `system_prompt` フィールドの直後

### 2-3. `Task` への追加（plan.rs）

```rust
pub struct Task {
    // ... 既存フィールド（squash の直後） ...

    /// このタスク固有の完了ゲート
    /// 設定時は default_gates を上書き（優先: タスク固有 > デフォルト）
    #[serde(default)]
    pub completion_gates: Option<Vec<CompletionGate>>,

    /// true の場合、すべてのゲートをスキップ（default_gates も含む）
    #[serde(default)]
    pub skip_gates: bool,
}
```

### 2-4. ゲート実行フロー（main.rs の spawn 内）

リトライループ内で `map_exit_status` が Succeeded を返した後に挿入:

```
map_exit_status() → Succeeded の場合:
  ↓
resolve_gates(task, run_config) でゲートリスト決定
  ↓
ゲートが空なら → 従来通り Succeeded で break
  ↓
ゲートを順次実行（短絡評価）:
  for gate in gates:
    Event::GateStarted を記録
    sh -c "<command>" を spawn（cwd = タスクの workdir、env = タスクと同一）
    timeout(gate.timeout_sec.unwrap_or(300)) でタイムアウト制御
    stdout/stderr をゲートログに保存
    Event::GateFinished を記録
    exit code != 0 の場合:
      → GateResult を収集
      → TaskResult::failed(exit_code) に差し替えて break（以降のゲートはスキップ）
全ゲート pass → TaskResult::succeeded() で break

Succeeded 以外はリトライ判定に進む（既存ロジックそのまま）
```

**ゲート解決ロジック**（`resolve_gates` 関数として追加）:

```rust
fn resolve_gates(task: &Task, run_config: &RunConfig) -> Vec<CompletionGate> {
    if task.skip_gates {
        return vec![];
    }
    if let Some(gates) = &task.completion_gates {
        if !gates.is_empty() {
            return gates.clone();
        }
    }
    // Research モードにはデフォルトゲートを適用しない
    if task.mode == TaskMode::Research {
        return vec![];
    }
    run_config.default_gates.clone().unwrap_or_default()
}
```

### 2-5. `Event` enum の拡張（store/mod.rs）

```rust
pub enum Event {
    // ... 既存 ...

    GateStarted {
        task_id: String,
        gate_name: String,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
    GateFinished {
        task_id: String,
        gate_name: String,
        exit_code: i32,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
}
```

### 2-6. `GateResult` struct と `TaskState` 拡張（store/mod.rs）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub name: String,
    pub command: String,
    pub exit_code: i32,
    /// stderr の末尾（最大 50 行）
    pub stderr_tail: Option<String>,
    pub duration_sec: f64,
}

// TaskState に追加
pub struct TaskState {
    // ... 既存フィールド ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_results: Option<Vec<GateResult>>,
}
```

### 2-7. `Plan::validate()` への追加（plan.rs）

各タスクのバリデーションループ内に追加:

```rust
// completion_gates のバリデーション
if let Some(gates) = task.completion_gates.as_ref() {
    let mut gate_names = HashSet::new();
    for gate in gates {
        if gate.name.trim().is_empty() {
            bail!("task {} completion_gates contains gate with empty name", task.id);
        }
        if gate.command.trim().is_empty() {
            bail!("task {} gate '{}' has empty command", task.id, gate.name);
        }
        if !gate_names.insert(gate.name.clone()) {
            bail!("task {} has duplicate gate name '{}'", task.id, gate.name);
        }
    }
}
```

RunConfig バリデーション部分に追加:

```rust
if let Some(gates) = &self.run.default_gates {
    let mut gate_names = HashSet::new();
    for gate in gates {
        if gate.name.trim().is_empty() {
            bail!("run.default_gates contains gate with empty name");
        }
        if gate.command.trim().is_empty() {
            bail!("run.default_gates gate '{}' has empty command", gate.name);
        }
        if !gate_names.insert(gate.name.clone()) {
            bail!("run.default_gates has duplicate gate name '{}'", gate.name);
        }
    }
}
```

---

## 3. 注意点

### 3-1. worktree での実行

- ゲートコマンドは**タスクと同じ cwd**（worktree ディレクトリ）で実行する
- worktree 未使用時（`no_worktree: true` または worktree manager 未設定）は通常の cwd を使用
- ゲート実行中も **semaphore スロットを占有し続ける**（scheduler 変更不要）
- ゲートは worktree release 前に実行するため、ゲート失敗時は `manager.release_failure()`

```rust
let gate_child = tokio::process::Command::new("sh")
    .arg("-c")
    .arg(&gate.command)
    .current_dir(&workdir)    // タスクと同じ workdir
    .envs(&env_vars)          // タスクと同じ環境変数
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
```

### 3-2. auto_commit タイミング

- `auto_commit` は**ゲートがすべて pass した後**（真の Succeeded 確定後）に実行
- spawn() 内でゲート実行後に Succeeded を返すため、Scheduler 側の変更は不要
- ゲート失敗で `TaskResult::failed()` に差し替えた場合は auto_commit は実行されない

### 3-3. retry 連携

- ゲート失敗 → `TaskResult::failed(exit_code)` → リトライループの failure 判定に入る
- `retry_strategy.inject_error_context` が有効な場合、ゲートの stderr をエラーコンテキストに含める
  - 注入メッセージ例: 「前回の試行ではタスクは正常終了しましたが、検証ゲート '{gate_name}' が失敗しました（exit code: {code}）: {stderr_tail}」
- ゲートの stdout/stderr は別途ゲートログに保存、retry エラーコンテキスト用は `GateResult.stderr_tail` を参照
- `max_stderr_lines` のデフォルト 50 行で stderr をトリム
- `FailureType::classify()` による永続的失敗判定はゲート失敗には**適用しない**（常にリトライ対象）

### 3-4. ゲートログの保存先

```
.quedex/runs/<run_id>/tasks/<task_id>/gates/<gate_name>/stdout.log
.quedex/runs/<run_id>/tasks/<task_id>/gates/<gate_name>/stderr.log
```

または既存タスクログ末尾に区切り付きで追記する方式でも可（実装コスト低）。
`Store::log_path()` ヘルパーにゲート用バリアントを追加するか、main.rs 側でパスを直接組み立てるかは実装時に判断。

### 3-5. タイムアウトのデフォルト値

- `CompletionGate.timeout_sec` 未設定時のデフォルト: **300 秒（5 分）**
- タイムアウト時はゲート失敗扱い（exit_code = -1 相当）
- エラーメッセージに「gate timeout」を明記

### 3-6. `args` フィールドについて

proposal 原案に `args: Vec<String>` があるが、`sh -c command` で実行する場合は不要。
シンプルさを優先するなら `command` フィールドのみ（引数は command 文字列に含める形）とし、`args` フィールドは削除してよい。

---

## 4. ファイル別変更サマリ

| ファイル | 変更内容 |
|---|---|
| `src/plan.rs` | `CompletionGate` struct 追加、`RunConfig::default_gates` 追加、`Task::completion_gates` / `Task::skip_gates` 追加、`validate()` 拡張 |
| `src/main.rs` | `PlanTaskRunner::spawn` にゲート実行ブロック挿入、`resolve_gates()` 関数追加、retry エラーコンテキスト拡張 |
| `src/store/mod.rs` | `Event::GateStarted` / `Event::GateFinished` 追加、`GateResult` struct 追加、`TaskState::gate_results` 追加 |
| `src/tui/` or `src/status.rs` | タスク詳細ビューにゲート結果表示 |
| `tests/` | ゲート成功/失敗/タイムアウト/default_gates/skip_gates のテスト追加 |

## 5. コンポーネント実装順（依存関係）

1. **Component 1**: スキーマ拡張（src/plan.rs）← 他すべてが依存
2. **Component 2**: 状態管理拡張（src/store/mod.rs）← Component 3 が依存
3. **Component 3**: ゲート実行エンジン（src/main.rs）← Component 4, 5 が依存
4. **Component 4**: Retry 連携（src/main.rs）
5. **Component 5**: CLI/TUI 表示（src/main.rs, src/tui/ui.rs）
6. **Component 6**: テスト（tests/）
