# Proposal: 停滞検出 (Stall Detection)

## 背景と課題

### 現状の問題

quedexは現在、タスクのプロセス終了を無期限に待機する（`src/main.rs` の `spawn_blocking(move || child.wait())`）。LLMエージェント（Codex CLI, Claude Code, Opencode）は以下の理由でハングする可能性がある:

- **無限ループ**: エージェントが自己参照的な思考ループに入る
- **レートリミット**: API側のレートリミットにより応答が停止
- **ネットワーク障害**: 接続タイムアウトがエージェント内部で適切に処理されない
- **リソース枯渇**: メモリリークやディスクフル等でプロセスが応答不能になる

この場合、該当タスクが永久にブロックし、依存するすべての下流タスクも実行されない。`fail_fast` モードでも、プロセスが終了しない限り検出できない。

### 先行事例

OpenAI Symphonyは同様の問題に対して「300秒間エージェントからのイベントがなければ強制killしてリトライ」というstall detectionを実装している。本proposalはこのアプローチをquedexに適用する。

---

## 設計

### 概要

各タスクのstdout/stderr書き込みアクティビティを監視し、`stall_timeout` で指定された時間内に出力がなければ、プロセスを強制killしてFailedとして扱う（retry対象）。

### スキーマ変更

#### RunConfig（run-levelデフォルト）

```rust
// src/plan.rs
pub struct RunConfig {
    // ... 既存フィールド
    /// stdout/stderrの無出力タイムアウト（秒）。0で無効化。
    #[serde(default = "default_stall_timeout")]
    pub stall_timeout: u64,
}

fn default_stall_timeout() -> u64 {
    300 // Symphony準拠
}
```

#### Task（task-levelオーバーライド）

```rust
// src/plan.rs
pub struct Task {
    // ... 既存フィールド
    /// タスク固有のstall timeout（秒）。run.stall_timeoutをオーバーライド。0で無効化。
    #[serde(default)]
    pub stall_timeout: Option<u64>,
}
```

#### plan.json / plan.yaml での指定例

```json
{
  "version": 1,
  "run": {
    "stall_timeout": 300
  },
  "tasks": [
    {
      "id": "fast-task",
      "stall_timeout": 60,
      "claude_code": { "prompt": "..." }
    },
    {
      "id": "long-compile",
      "stall_timeout": 0,
      "claude_code": { "prompt": "..." }
    }
  ]
}
```

タスクレベルの `stall_timeout` がrunレベルのデフォルトを上書きする。`0` を指定すると無効化。

### 実装方針

#### 1. stdout/stderrファイル監視

現在のrunner実装では、子プロセスのstdout/stderrはファイルにリダイレクトされている（`ChildHandle` の `stdout_path` / `stderr_path`）。これらのファイルのメタデータ（更新日時またはファイルサイズ）を定期的にポーリングし、変化がなければstallと判定する。

```rust
// 疑似コード: src/main.rs のタスク実行ループ内
let stall_timeout = task.stall_timeout
    .unwrap_or(run_config.stall_timeout);

if stall_timeout > 0 {
    let timeout_duration = Duration::from_secs(stall_timeout);
    let stdout_path = child.stdout_path.clone();
    let stderr_path = child.stderr_path.clone();

    let stall_monitor = tokio::spawn(async move {
        let mut last_stdout_size = 0u64;
        let mut last_stderr_size = 0u64;
        let mut last_activity = Instant::now();
        let poll_interval = Duration::from_secs(5);

        loop {
            tokio::time::sleep(poll_interval).await;

            let stdout_size = fs::metadata(&stdout_path)
                .map(|m| m.len()).unwrap_or(0);
            let stderr_size = fs::metadata(&stderr_path)
                .map(|m| m.len()).unwrap_or(0);

            if stdout_size != last_stdout_size || stderr_size != last_stderr_size {
                last_stdout_size = stdout_size;
                last_stderr_size = stderr_size;
                last_activity = Instant::now();
            }

            if last_activity.elapsed() >= timeout_duration {
                return true; // stall detected
            }
        }
    });

    tokio::select! {
        result = wait_future => { /* 通常の終了処理 */ }
        stalled = stall_monitor => {
            if stalled {
                child.kill();
                // Failedとして処理（retryループで再試行対象に）
            }
        }
    }
}
```

#### 2. プロセスkillとエラーハンドリング

stall検出時の処理フロー:

1. `child.kill()` でプロセスをSIGKILLする
2. ステータスを `TaskStatus::Failed` に設定し、exit_codeは特殊値（例: `-1` または `124`、timeoutの慣例）
3. stderrログの末尾に `[quedex] stall detected: no output for {stall_timeout}s, process killed` を追記
4. 既存のretryループがそのまま再試行を処理する

#### 3. SchedulerEvent / Store拡張

`Event` enumに新しいバリアントを追加してstall killを記録する:

```rust
// src/store/mod.rs
pub enum Event {
    // ... 既存バリアント
    TaskStalled {
        task_id: String,
        stall_timeout_sec: u64,
        #[serde(rename = "ts")]
        timestamp: DateTime<Utc>,
    },
}
```

これによりTUIやWeb UIでstallが発生したことを表示できる。

#### 4. TaskStateへの情報付加

```rust
// src/store/mod.rs
pub struct TaskState {
    // ... 既存フィールド
    /// stall detectionによって強制killされた場合true
    #[serde(default)]
    pub stalled: bool,
}
```

### retryメカニズムとの統合

stall killは通常のFailedと同様に扱われるため、既存のretry機構がそのまま適用される:

- `retry_count` > 0 であれば自動リトライ
- `retry_delay_sec` 分の待機後に再実行
- `retry_strategy.inject_error_context` が有効なら、stall時のstderr末尾（`[quedex] stall detected...` メッセージを含む）がリトライプロンプトに注入される
- `retry_strategy.escalate_model` によるモデルエスカレーションも通常通り動作

### adaptive retryとの相乗効果

stall検出とadaptive retryを組み合わせることで、以下のような自動復旧が可能:

```json
{
  "id": "complex-task",
  "stall_timeout": 180,
  "retry_count": 2,
  "retry_strategy": {
    "inject_error_context": true,
    "escalate_model": "opus"
  }
}
```

1回目: sonnetモデルで実行 → 180秒間出力なし → stall kill
2回目: stallのコンテキストを注入してsonnetで再試行
3回目: opusにエスカレーションして再試行

---

## エッジケースと対策

### 出力を長時間生成しないタスク

一部のタスクは正当な理由で長時間出力を生成しない場合がある:

- **長時間のコンパイル**: `cargo build` 等がエージェント内部で実行される場合
- **大規模ファイルのダウンロード**: ネットワーク操作中はエージェント出力がない

対策: タスクレベルで `stall_timeout: 0` を指定して無効化、または十分に大きな値を設定する。

### プロセスグループの処理

LLMエージェントは内部で子プロセス（コンパイラ、テストランナー等）を起動する。`child.kill()` は直接のプロセスのみをkillするため、孫プロセスが残る可能性がある。

対策: プロセスグループ単位でのkill（`libc::killpg`）を検討する。ただし初期実装では `child.kill()` のみで対応し、必要に応じて拡張する。

### stall判定の誤検知

エージェントが内部的に処理中（ツール呼び出しの応答待ち等）でも、stdout/stderrに出力がなければstallと判定される。

対策:
- デフォルト300秒は十分に保守的な値
- 誤検知が発生してもretryで自動復旧可能
- タスク単位で `stall_timeout` を調整可能

### cancel操作との競合

ユーザーが `quedex cancel` を実行した場合とstall killが同時に発生する可能性がある。

対策: `cancel.is_canceled()` チェックを優先し、stall killの場合のみ `TaskStatus::Failed` を設定する。cancelの場合は `TaskStatus::Canceled` が優先される。

---

## 影響範囲

### 変更ファイル

| ファイル | 変更内容 |
|---|---|
| `src/plan.rs` | `RunConfig` に `stall_timeout` フィールド追加、`Task` に `stall_timeout` フィールド追加 |
| `src/main.rs` | タスク実行ループにstall監視ロジック追加 |
| `src/store/mod.rs` | `Event::TaskStalled` バリアント追加、`TaskState.stalled` フィールド追加 |
| `src/runner/mod.rs` | 変更なし（既存の `ChildHandle::kill()` をそのまま利用） |
| `src/scheduler.rs` | 変更なし（stall検出はrunner層で処理） |
| `src/tui/ui.rs` | stall表示の追加（stalledタスクにインジケータ表示） |
| JSON Schema | `stall_timeout` フィールドの追加 |

### 後方互換性

- `stall_timeout` はデフォルト値300秒を持つため、既存のplan.jsonはそのまま動作する
- stall detectionを望まない場合は `stall_timeout: 0` で無効化できる
- 既存の `timeout_sec` フィールドは削除済み（rejectされる）であり、`stall_timeout` は異なるセマンティクス（プロセス全体のタイムアウトではなく、無出力時間の監視）を持つ

### 削除済み timeout_sec との違い

以前存在した `timeout_sec` はプロセス全体の実行時間制限であり、「正常に動作しているが時間がかかるタスク」も強制終了してしまう問題があった。`stall_timeout` は出力アクティビティを監視するため、活発に動作中のタスクは影響を受けない。

---

## 実装順序

1. **スキーマ変更**: `RunConfig` と `Task` に `stall_timeout` フィールドを追加
2. **Store拡張**: `Event::TaskStalled` と `TaskState.stalled` を追加
3. **コア実装**: `src/main.rs` のタスク実行ループにstall監視を追加
4. **TUI対応**: stalledタスクの表示インジケータ
5. **テスト**: stall検出の単体テストと統合テスト
6. **ドキュメント**: JSON SchemaとREADMEの更新
