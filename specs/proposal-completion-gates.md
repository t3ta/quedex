# Proposal: Completion Gates / 完了ゲート (Proof of Work)

## 背景と動機

現在のquedexでは、タスクのrunnerプロセスが exit code 0 で終了すれば「成功」とみなしている。しかし、LLMコーディングエージェントが「完了した」と報告しても、実際にはコンパイルエラーやテスト失敗を残していることが少なくない。

OpenAI Symphonyの「Proof of Work」コンセプトに触発され、タスク完了後に検証コマンド（ゲート）を順次実行し、すべてパスして初めて真の成功とする仕組みを提案する。

---

## Phase 0: Discovered Information

### Project Overview
- **Project**: quedex - DAG-based task execution with LLM coding agent integration
- **Language**: Rust
- **Build**: Cargo

### Relevant Files
- `src/plan.rs` - Plan/Taskスキーマ定義（`Task` struct、バリデーション）
- `src/scheduler.rs` - スケジューラ（`TaskResult`, `TaskRunner`, `handle_event`）
- `src/store/mod.rs` - 状態管理（`TaskStatus`, `TaskState`, `Event`）
- `src/main.rs` - コマンドハンドラ（runner起動ロジック）

### Existing Structures
```rust
// Task構造 (src/plan.rs) - 関連フィールド抜粋
pub struct Task {
    pub id: String,
    pub mode: TaskMode,
    pub retry_count: u32,
    pub retry_delay_sec: u64,
    pub retry_strategy: Option<RetryStrategy>,
    // ... other fields
}

// TaskResult (src/scheduler.rs)
pub struct TaskResult {
    pub status: TaskStatus,  // Succeeded | Failed | Canceled
    pub exit_code: Option<i32>,
}

// TaskState (src/store/mod.rs)
pub struct TaskState {
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub stderr_tail: Option<String>,
    // ...
}
```

---

## Phase 1: Project Overview

### 目的
タスク完了後に検証コマンド（completion gates）を実行し、品質を担保する仕組みを導入する。ゲートがすべてパスしなければタスクは失敗扱いとなり、retry対象になる。

### 成功基準
- **最重要**: タスク成功後にゲートコマンドが順次実行され、全パスで初めてSucceededとなること
- ゲート失敗時にどのゲートが失敗したか・stderrが記録されること
- retryとの連携（ゲート失敗 → タスク全体のretry）
- run-levelのデフォルトゲート定義

### スコープ
- スキーマ: `Task.completion_gates` フィールド追加
- スキーマ: `RunConfig.default_gates` フィールド追加
- スキーマ: `Task.skip_gates` フィールド追加
- 実行フロー: runner成功後にゲート実行ロジック追加
- 状態管理: ゲート結果の記録

---

## Phase 2: Features

### Feature 1: スキーマ拡張 - CompletionGate (Must)
**User Story**: 開発者として、タスクごとに完了条件となるコマンドを定義したい。

#### スキーマ定義

```rust
/// 完了ゲート: タスク成功後に実行される検証コマンド
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompletionGate {
    /// ゲートの名前（ログ表示・エラー報告用）
    pub name: String,
    /// 実行するコマンド（シェル経由で実行）
    pub command: String,
    /// ゲート固有のタイムアウト秒数（省略時はデフォルト300秒）
    #[serde(default)]
    pub timeout_sec: Option<u64>,
}
```

```yaml
# Plan YAML での記述例
tasks:
  - id: implement-auth
    mode: implement
    claude_code:
      prompt: "認証モジュールを実装してください"
    completion_gates:
      - name: "type check"
        command: "cargo check"
      - name: "lint"
        command: "cargo clippy -- -D warnings"
      - name: "unit test"
        command: "cargo test --lib"
        timeout_sec: 120
```

**Acceptance Criteria**:
- [ ] `CompletionGate` structを `src/plan.rs` に追加
- [ ] `Task` structに `completion_gates: Vec<CompletionGate>` を追加（`#[serde(default)]`）
- [ ] `Task` structに `skip_gates: bool` を追加（`#[serde(default)]`、デフォルト `false`）
- [ ] バリデーション: `name` と `command` が空文字でないこと
- [ ] バリデーション: `name` の重複チェック（同一タスク内）
- [ ] JSONスキーマ更新

### Feature 2: Run-levelデフォルトゲート (Must)
**User Story**: 開発者として、全implement/verifyタスクに共通のゲートを一箇所で定義したい。

```yaml
# Plan YAML での記述例
run:
  name: "feature-x"
  default_gates:
    - name: "compile check"
      command: "cargo check"
    - name: "lint"
      command: "cargo clippy -- -D warnings"

tasks:
  - id: implement-core
    mode: implement
    claude_code:
      prompt: "コア機能を実装"
    # → default_gatesが自動適用される

  - id: research-api
    mode: research
    claude_code:
      prompt: "APIドキュメントを調査"
    # → researchモードなのでゲート適用なし

  - id: implement-hotfix
    mode: implement
    skip_gates: true
    claude_code:
      prompt: "緊急修正"
    # → skip_gates: trueなのでゲートスキップ

  - id: implement-ui
    mode: implement
    claude_code:
      prompt: "UI実装"
    completion_gates:
      - name: "frontend test"
        command: "npm test"
    # → タスク固有ゲートのみ実行（default_gatesは適用しない）
```

**Acceptance Criteria**:
- [ ] `RunConfig` structに `default_gates: Vec<CompletionGate>` を追加（`#[serde(default)]`）
- [ ] デフォルトゲートは `Implement` および `Verify` モードのタスクにのみ適用
- [ ] `Research` モードのタスクには適用しない
- [ ] タスクに `completion_gates` が明示的に定義されている場合、デフォルトゲートは**適用しない**（タスク固有ゲートが優先）
- [ ] `skip_gates: true` のタスクにはデフォルトゲートもタスク固有ゲートも適用しない

### Feature 3: ゲート実行エンジン (Must)
**User Story**: 開発者として、タスク成功後にゲートが自動実行され、結果に応じてタスクの最終ステータスが決定されてほしい。

#### 実行フロー

```
Runner実行
  │
  ├─ exit code != 0 → TaskStatus::Failed（従来通り）
  │
  └─ exit code == 0
       │
       ├─ skip_gates: true → TaskStatus::Succeeded
       │
       └─ ゲート解決（タスク固有 or デフォルト）
            │
            ├─ ゲートなし → TaskStatus::Succeeded
            │
            └─ ゲートあり → 順次実行
                 │
                 ├─ 全ゲートpass → TaskStatus::Succeeded
                 │
                 └─ いずれかfail → TaskStatus::Failed
                      └─ retry_count > 0 なら retry対象
```

**Acceptance Criteria**:
- [ ] ゲートはタスクのworking directory（cwdまたはworktree）で実行
- [ ] ゲートは定義順に**逐次**実行（並列実行しない）
- [ ] あるゲートが失敗した時点で残りのゲートはスキップ（short-circuit）
- [ ] ゲートのstdout/stderrはタスクのログディレクトリに記録
- [ ] ゲート実行中もTUIでRunning表示を維持

### Feature 4: ゲートタイムアウト (Should)
**User Story**: 開発者として、ゲートが無限に実行され続けることを防ぎたい。

**Acceptance Criteria**:
- [ ] `CompletionGate.timeout_sec` でゲート個別のタイムアウトを指定可能
- [ ] 未指定時のデフォルトは300秒（5分）
- [ ] タイムアウト時はゲート失敗扱い（タスク全体がFailed）
- [ ] タイムアウト発生時のエラーメッセージに「gate timeout」と明記

### Feature 5: ゲート失敗レポート (Must)
**User Story**: 開発者として、どのゲートが失敗したかを素早く特定したい。

#### 状態拡張

```rust
// TaskState (src/store/mod.rs) への追加フィールド
pub struct TaskState {
    // ... 既存フィールド
    /// ゲート実行結果（ゲートが実行された場合のみ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_results: Option<Vec<GateResult>>,
}

/// 個別ゲートの実行結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    /// ゲート名
    pub name: String,
    /// 実行したコマンド
    pub command: String,
    /// exit code
    pub exit_code: i32,
    /// stderrの末尾（最大50行）
    pub stderr_tail: Option<String>,
    /// 実行時間（秒）
    pub duration_sec: f64,
}
```

**Acceptance Criteria**:
- [ ] `GateResult` structを `src/store/mod.rs` に追加
- [ ] `TaskState` に `gate_results` フィールドを追加
- [ ] 失敗ゲートのstderrを最大50行キャプチャ
- [ ] `quedex status` でゲート結果を表示
- [ ] TUIのタスク詳細でゲート結果を表示

### Feature 6: Retry連携 (Must)
**User Story**: 開発者として、ゲート失敗時にタスク全体をretryしてほしい。

**Acceptance Criteria**:
- [ ] ゲート失敗はタスクのFailed扱いとなり、`retry_count` に基づいてretry対象
- [ ] retry時はメインタスクの最初から再実行（ゲートだけの再実行ではない）
- [ ] `retry_strategy.inject_error_context` が有効な場合、ゲートのstderrもエラーコンテキストに含める
- [ ] ゲート失敗情報をretryプロンプトに注入:「前回の試行ではタスクは完了しましたが、検証ゲート '{gate_name}' が失敗しました: {stderr_tail}」

---

## Phase 3: Technical Hints

### 実装方針

#### ゲート実行のタイミング
schedulerの `handle_event` 内でゲート実行を行うのは不適切（同期的なイベントハンドラ内で長時間ブロックするため）。代わりに、runner層でゲート実行を組み込む。

```
TaskRunner::spawn()
  → runner実行（codex/claude_code/opencode）
  → exit 0の場合、ゲートを順次実行
  → 最終結果をTaskResultとして返却
```

これにより、schedulerの変更を最小限に抑えつつ、ゲート実行中もsemaphoreスロットを占有し続ける（想定される動作）。

#### ゲートの解決ロジック
```rust
fn resolve_gates(task: &Task, run_config: &RunConfig) -> Vec<CompletionGate> {
    if task.skip_gates {
        return vec![];
    }
    if !task.completion_gates.is_empty() {
        return task.completion_gates.clone();
    }
    // researchモードにはデフォルトゲートを適用しない
    if task.mode == TaskMode::Research {
        return vec![];
    }
    run_config.default_gates.clone()
}
```

#### ゲートログの保存
ゲートのstdout/stderrは以下のパスに保存:
```
.quedex/runs/<run_id>/tasks/<task_id>/gates/<gate_name>/stdout.log
.quedex/runs/<run_id>/tasks/<task_id>/gates/<gate_name>/stderr.log
```

#### Eventの拡張
```rust
pub enum Event {
    // ... 既存
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

### 参考パターン
- **Runner実行**: `src/main.rs` のrunnerプロセス起動ロジック
- **stderr_tail取得**: 既存の `TaskState.stderr_tail` パターン
- **タイムアウト**: `tokio::time::timeout` の利用
- **Retry連携**: `src/plan.rs` の `RetryStrategy` と `retry_count`

### 設計上の注意点
- ゲートは**タスクのworking directory**で実行する（worktree使用時はworktree内）
- ゲートコマンドはシェル経由（`sh -c "command"`）で実行する
- ゲートの環境変数はタスクと同一のものを継承する
- `auto_commit` はゲートがすべてパスした後に実行する（ゲート失敗時はcommitしない）

---

## Phase 4: Components

### Component 1: スキーマ拡張
- **Files**: `src/plan.rs`
- **Lock**: plan.rs
- **内容**: `CompletionGate` struct追加、`Task.completion_gates`/`Task.skip_gates` 追加、`RunConfig.default_gates` 追加、バリデーション

### Component 2: 状態管理拡張
- **Files**: `src/store/mod.rs`
- **Lock**: store
- **内容**: `GateResult` struct追加、`TaskState.gate_results` 追加、`Event` variant追加、ゲートログディレクトリ対応

### Component 3: ゲート実行エンジン
- **Files**: `src/runner.rs`（新規）, `src/main.rs`
- **Lock**: main.rs
- **Depends on**: Component 1, Component 2
- **内容**: ゲート解決ロジック、ゲートコマンド実行、タイムアウト処理、結果集約

### Component 4: Retry連携
- **Files**: `src/main.rs`
- **Lock**: main.rs
- **Depends on**: Component 3
- **内容**: ゲート失敗時のエラーコンテキスト注入、retryプロンプト生成

### Component 5: CLI/TUI表示
- **Files**: `src/main.rs`, `src/tui/ui.rs`
- **Lock**: tui
- **Depends on**: Component 2
- **内容**: `quedex status` でのゲート結果表示、TUI詳細ビューでのゲート表示

### Component 6: テスト
- **Files**: `tests/`
- **No Lock**
- **Depends on**: All components
- **内容**: ゲート成功/失敗/タイムアウトのunit test、retry連携のintegration test

---

## Phase 5: Runner Selection

- **Default Runner**: claude_code
- **Model**: opus
- **全タスク共通設定**

---

## 補足: 将来の拡張候補

以下は本提案のスコープ外だが、将来的に検討する価値がある:

- **ゲートの並列実行オプション**: 独立したゲート（lint と test など）を並列実行する `parallel: true` オプション
- **ゲートのcontinue-on-error**: 全ゲートを実行し、最後にまとめて結果を報告する `continue_on_error: true` オプション
- **ゲートのcaching**: 前回成功したゲートの結果をキャッシュし、変更がない場合はスキップ
- **カスタムゲート結果パーサー**: JUnit XML等の構造化された出力をパースし、詳細なレポートを生成
