# Proposal: ライフサイクルフック & プロンプトテンプレート

## 背景

OpenAI Symphonyの`WORKFLOW.md`では、フックやLiquidテンプレートによりタスク実行の前後に柔軟な処理を挿入できる。一方、現在のquedexでは`quedex.toml`は`max_concurrency`、`fail_fast`、`store`、`system_prompt`といった基本設定のみをサポートしている。

タスク実行のライフサイクル各ポイントでシェルコマンドを実行したい（環境セットアップ、通知、クリーンアップ等）ニーズや、リトライ回数に応じてプロンプトを動的に変更したいニーズに対応するため、ライフサイクルフックとプロンプトテンプレート機能を提案する。

---

## Phase 0: Discovered Information

### Project Overview
- **Project**: quedex - DAG-based task orchestrator for LLM coding agents
- **Language**: Rust
- **Build**: Cargo

### Relevant Files
- `src/config.rs` - `Config`構造体（quedex.toml読み込み）
- `src/scheduler.rs` - `Scheduler`、`SchedulerEvent`、タスク実行ループ
- `src/plan.rs` - `Plan`、`Task`、`RunConfig`スキーマ定義
- `src/main.rs` - コマンドハンドラ、タスク起動処理

### Existing Structures
```rust
// quedex.toml (src/config.rs)
pub struct Config {
    pub max_concurrency: Option<usize>,
    pub fail_fast: Option<bool>,
    pub store: Option<PathBuf>,
    pub system_prompt: Option<String>,
}

// スケジューライベント (src/scheduler.rs)
enum SchedulerEvent {
    TaskFinished { task_id: TaskId, result: TaskResult, task_spec: TaskSpec },
}

// Plan (src/plan.rs) - 既存のvariablesフィールドはない（以前削除済み）
// RunConfig にはenv: Option<HashMap<String, String>>がある
```

---

## Phase 1: Project Overview

### 目的
タスク実行ライフサイクルの各ポイントにフックを設定し、プロンプトをテンプレートエンジンで動的に生成できるようにする。

### 成功基準
- `quedex.toml`の`[hooks]`セクションでライフサイクルフックを定義できる
- フックのシェルコマンドが適切なタイミングで実行される
- フックコマンドに`QUEDEX_*`環境変数が渡される
- プロンプト内でTeraテンプレート構文（`{{ attempt }}`等）が展開される
- 既存のplan.yamlとquedex.tomlに後方互換性がある

### スコープ
- `quedex.toml`への`[hooks]`セクション追加
- plan.yamlでのタスクレベルフック定義
- Teraテンプレートエンジンによるプロンプト展開
- テンプレート変数: `attempt`、`task.*`、`run.*`、`env.*`

---

## Phase 2: Features

### Feature 1: quedex.toml フック設定 (Must)
**User Story**: 開発者として、全タスク共通のライフサイクルフックをquedex.tomlで定義したい。

**設定例**:
```toml
[hooks]
before_run = "echo 'Starting run' && mkdir -p /tmp/quedex-workspace"
after_run = "echo 'Run completed' && ./scripts/notify.sh"
before_task = "echo 'Starting task: $QUEDEX_TASK_ID'"
after_task = "echo 'Task $QUEDEX_TASK_ID finished with status: $QUEDEX_STATUS'"
on_failure = "echo 'Task $QUEDEX_TASK_ID failed (attempt $QUEDEX_ATTEMPT)'"
```

**Acceptance Criteria**:
- [ ] `Config`構造体に`hooks: Option<HooksConfig>`を追加
- [ ] `HooksConfig`に`before_run`、`after_run`、`before_task`、`after_task`、`on_failure`フィールドを追加
- [ ] 各フィールドは`Option<String>`（シェルコマンド文字列）
- [ ] フック未設定時は何も実行しない（後方互換性）

### Feature 2: フック実行エンジン (Must)
**User Story**: 開発者として、フックが適切なタイミングで実行され、環境変数でコンテキストを受け取りたい。

**環境変数一覧**:

| 変数名 | 説明 | 利用可能フック |
|---|---|---|
| `QUEDEX_RUN_ID` | 実行ID | 全フック |
| `QUEDEX_RUN_NAME` | Run名（run.nameから） | 全フック |
| `QUEDEX_TASK_ID` | タスクID | before_task, after_task, on_failure |
| `QUEDEX_TASK_TITLE` | タスクタイトル | before_task, after_task, on_failure |
| `QUEDEX_STATUS` | タスクステータス（succeeded/failed） | after_task, on_failure |
| `QUEDEX_ATTEMPT` | 現在の試行回数（1-indexed） | before_task, after_task, on_failure |
| `QUEDEX_EXIT_CODE` | 終了コード | after_task, on_failure |

**Acceptance Criteria**:
- [ ] `before_run`はスケジューラ開始前に実行される
- [ ] `after_run`はスケジューラ完了後に実行される（成功・失敗問わず）
- [ ] `before_task`はタスク開始直前（`TaskStatus::Running`設定後）に実行される
- [ ] `after_task`はタスク完了後（成功・失敗問わず）に実行される
- [ ] `on_failure`はタスク失敗時に`after_task`の前に実行される
- [ ] フックコマンドの失敗はwarning出力のみで、タスク実行を中断しない
- [ ] フックコマンドのタイムアウト（デフォルト30秒、設定可能）

### Feature 3: タスクレベルフック (Should)
**User Story**: 開発者として、特定タスクに固有のフックを定義したい。グローバルフックとマージして実行される。

**plan.yaml設定例**:
```yaml
tasks:
  - id: build-backend
    claude_code:
      prompt: "Build the backend"
    hooks:
      before_task: "cd backend && npm install"
      after_task: "npm test"
```

**Acceptance Criteria**:
- [ ] `Task`構造体に`hooks: Option<TaskHooksConfig>`を追加
- [ ] タスクレベルフックはグローバルフック（quedex.toml）の後に実行される
- [ ] タスクレベルでは`before_task`、`after_task`、`on_failure`のみサポート

### Feature 4: Teraプロンプトテンプレート (Must)
**User Story**: 開発者として、リトライ時にプロンプトを動的に変更し、失敗コンテキストを含めたい。

**plan.yaml設定例**:
```yaml
tasks:
  - id: implement-feature
    retry_count: 2
    claude_code:
      prompt: |
        Implement the login feature.
        {% if attempt > 1 %}
        NOTE: This is retry attempt {{ attempt }}.
        The previous attempt failed. Please review your approach carefully.
        {% endif %}
```

**利用可能なテンプレート変数**:

| 変数名 | 型 | 説明 |
|---|---|---|
| `attempt` | integer | 現在の試行回数（1-indexed） |
| `task.id` | string | タスクID |
| `task.title` | string | タスクタイトル |
| `task.mode` | string | タスクモード（research/implement/verify） |
| `run.name` | string | Run名 |
| `env` | object | 環境変数マップ（`{{ env.HOME }}`等） |

**Acceptance Criteria**:
- [ ] Teraテンプレートエンジンをdependencyに追加
- [ ] タスク実行前にプロンプト文字列をTera展開する
- [ ] テンプレート構文エラー時は展開前のプロンプトをそのまま使用し、warning出力
- [ ] テンプレート構文を含まないプロンプトは変更なし（後方互換性）
- [ ] `run.env`で定義された環境変数も`env.*`経由でアクセス可能

### Feature 5: quedex.toml テンプレート設定 (Could)
**User Story**: 開発者として、テンプレートのデリミタやカスタム変数をquedex.tomlで設定したい。

**設定例**:
```toml
[templates]
enabled = true  # デフォルト: true

[templates.variables]
project_name = "my-project"
coding_style = "functional"
```

**Acceptance Criteria**:
- [ ] `Config`構造体に`templates: Option<TemplatesConfig>`を追加
- [ ] `templates.variables`で定義した変数がプロンプトテンプレートから参照可能
- [ ] `templates.enabled = false`でテンプレート展開を無効化できる

---

## Phase 3: Technical Hints

### 実行フロー（フック統合後）

```
before_run (quedex.toml)
│
├── Task A
│   ├── before_task (quedex.toml → plan.yaml)
│   ├── [テンプレート展開 → プロンプト生成]
│   ├── [タスク実行]
│   ├── on_failure? (quedex.toml → plan.yaml)  ← 失敗時のみ
│   └── after_task (quedex.toml → plan.yaml)
│
├── Task B (並行実行)
│   └── ...
│
after_run (quedex.toml)
```

### 参考パターン
- **タスク実行ポイント**: `src/scheduler.rs`の`handle_event()` - タスク完了時の処理（git commit等）が既に存在し、フック呼び出しを同様に統合可能
- **環境変数管理**: `src/main.rs`の環境変数マージ処理 - `run.env`と`std::env`のマージパターンを再利用
- **設定読み込み**: `src/config.rs`の`Config::load()` - TOMLパース処理を拡張
- **リトライ処理**: `src/main.rs`のリトライループ - `attempt`カウンタが既に存在

### 設計方針
- フックは`tokio::process::Command`で非同期実行し、タイムアウトを設ける
- フックの失敗はタスク実行をブロックしない（warning出力のみ）
- テンプレートエンジンにはTeraを採用（Rustエコシステムで広く使われ、Jinjaライクな構文）
- テンプレート展開はタスク実行直前に1回だけ行う
- `before_run`/`after_run`フック失敗時のrun自体の中断はオプション（`hooks.fail_on_error`で制御）

### Tera依存追加
```toml
# Cargo.toml
[dependencies]
tera = "1"
```

### 構造体設計案
```rust
// src/config.rs に追加
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_task: Option<String>,
    pub after_task: Option<String>,
    pub on_failure: Option<String>,
    /// フックコマンドのタイムアウト（秒）。デフォルト: 30
    pub timeout_sec: Option<u64>,
    /// before_run/after_run失敗時にrunを中断するか。デフォルト: false
    pub fail_on_error: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TemplatesConfig {
    pub enabled: Option<bool>,
    pub variables: Option<HashMap<String, String>>,
}

// Config に追加
pub struct Config {
    // ... existing fields ...
    pub hooks: Option<HooksConfig>,
    pub templates: Option<TemplatesConfig>,
}
```

```rust
// src/plan.rs Task に追加
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskHooksConfig {
    pub before_task: Option<String>,
    pub after_task: Option<String>,
    pub on_failure: Option<String>,
}

pub struct Task {
    // ... existing fields ...
    pub hooks: Option<TaskHooksConfig>,
}
```

---

## Phase 4: Components

### Component 1: Config拡張（HooksConfig / TemplatesConfig）
- **Files**: `src/config.rs`
- **Lock**: config.rs

### Component 2: Plan拡張（TaskHooksConfig）
- **Files**: `src/plan.rs`
- **Lock**: plan.rs

### Component 3: フック実行エンジン
- **Files**: `src/hooks.rs`（新規）
- **Lock**: hooks.rs
- **Depends on**: Component 1, Component 2

### Component 4: テンプレートエンジン統合
- **Files**: `src/template.rs`（新規）
- **Lock**: template.rs
- **Depends on**: Component 1

### Component 5: スケジューラ統合
- **Files**: `src/scheduler.rs`, `src/main.rs`
- **Lock**: scheduler.rs, main.rs
- **Depends on**: Component 3, Component 4

### Component 6: テスト・検証
- **Files**: `tests/`
- **No Lock**
- **Depends on**: All components

---

## Phase 5: Runner Selection

- **Default Runner**: claude_code
- **Model**: opus
- **全タスク共通設定**

---

## 付録: 完全なquedex.toml設定例

```toml
max_concurrency = 4
fail_fast = true
store = ".quedex"
system_prompt = "You are a senior Rust developer."

[hooks]
before_run = "./scripts/setup.sh"
after_run = "./scripts/cleanup.sh"
before_task = "echo 'Starting: $QUEDEX_TASK_ID'"
after_task = "echo 'Finished: $QUEDEX_TASK_ID ($QUEDEX_STATUS)'"
on_failure = "./scripts/on-failure.sh"
timeout_sec = 60
fail_on_error = false

[templates]
enabled = true

[templates.variables]
project_name = "my-project"
lang = "rust"
```

## 付録: 完全なplan.yaml設定例

```yaml
version: 1
run:
  name: feature-implementation
  env:
    RUST_LOG: debug

tasks:
  - id: setup-db
    mode: implement
    hooks:
      before_task: "docker compose up -d postgres"
      after_task: "docker compose down"
    claude_code:
      prompt: |
        Set up the database schema.
        Project: {{ env.PROJECT_NAME }}

  - id: implement-api
    mode: implement
    deps: [setup-db]
    retry_count: 2
    claude_code:
      prompt: |
        Implement the REST API endpoints.
        {% if attempt > 1 %}
        This is retry attempt {{ attempt }}. Review the previous error and fix the issue.
        {% endif %}

  - id: verify-api
    mode: verify
    deps: [implement-api]
    claude_code:
      prompt: "Run tests for task {{ task.id }} (depends on: implement-api)"
```
