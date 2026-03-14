# Proposal: Issue Tracker連携 (Issue Tracker Polling Integration)

## 背景と動機

### 現状の課題
quedexは現在、タスク実行にplan.yaml（またはplan.json）ファイルの手動作成を必要とする。開発者はIssue Trackerでタスクを管理しつつ、別途plan.yamlを書くという二重管理が発生している。

### インスピレーション
OpenAI Symphonyは、LinearのIssueを自動的にポーリングし、エージェントの実行をトリガーする仕組みを持つ。この設計思想をquedexに取り入れ、Issue TrackerをDAGタスク実行のフロントエンドとして利用可能にする。

### ゴール
Issue Trackerの新規・更新Issueを検知し、LLMによるplan.yaml自動生成とquedex実行を一連のパイプラインとして自動化する。

---

## Phase 0: Discovered Information

### Project Overview
- **Project**: quedex - DAG-based task execution with LLM coding agent integration
- **Language**: Rust
- **Build**: Cargo
- **Supported Runners**: Codex CLI, Claude Code, Opencode

### Relevant Files
- `src/cli.rs` - CLIコマンド定義（`Commands` enum）
- `src/main.rs` - コマンドハンドラ
- `src/config.rs` - `quedex.toml` 設定読み込み（`Config` struct）
- `src/plan.rs` - Plan/Taskスキーマ定義
- `src/scheduler.rs` - タスクスケジューリング

### Existing Structures
```rust
// CLI commands (src/cli.rs)
pub enum Commands {
    Init { ... },
    Run { ... },
    Status { ... },
    // ... 既存コマンド群
}

// Config (src/config.rs)
pub struct Config {
    pub max_concurrency: Option<usize>,
    pub fail_fast: Option<bool>,
    pub store: Option<PathBuf>,
    pub system_prompt: Option<String>,
}
```

---

## Phase 1: Project Overview

### 目的
Issue TrackerをポーリングしてIssueからplan.yamlを自動生成・実行する`quedex watch`コマンドを追加する。

### 成功基準
- **最重要**: `quedex watch`でLinear Issueからplan.yamlを自動生成し、実行が完了すること
- Issue進捗に応じたステータス自動更新（Pending → In Progress → Done）
- Issue間の依存関係（blocking/blocked by）を尊重したディスパッチ
- GitHub Issues対応（第2ターゲット）

### スコープ
- CLI: `quedex watch` コマンド追加
- Config: `quedex.toml` にトラッカー設定セクション追加
- Plan生成: LLMによるIssue → plan.yaml変換
- ステータス同期: quedexタスク進捗 → Issue Trackerステータス更新
- スコープ外: Issue作成機能、PR自動作成（将来の拡張として残す）

---

## Phase 2: Features

### Feature 1: `quedex watch` CLIコマンド (Must)
**User Story**: 開発者として、Issue Trackerを監視して自動的にタスクを実行したい。

**コマンド仕様**:
```
quedex watch --tracker linear --project <project-slug> [--interval <seconds>] [--max-issues <n>] [--dry-run]
```

| オプション | デフォルト | 説明 |
|---|---|---|
| `--tracker` | (必須) | トラッカー種別: `linear`, `github` |
| `--project` | (必須) | プロジェクトslug/identifier |
| `--interval` | `30` | ポーリング間隔（秒） |
| `--max-issues` | `3` | 同時処理する最大Issue数 |
| `--dry-run` | `false` | plan.yaml生成のみ、実行しない |
| `--states` | (config参照) | 対象ステータスのカンマ区切り |

**Acceptance Criteria**:
- [ ] `quedex watch --tracker linear --project my-project` でポーリングが開始される
- [ ] Ctrl-Cでgraceful shutdownする
- [ ] ポーリング間隔は設定可能
- [ ] `--dry-run`で生成されたplan.yamlを確認できる
- [ ] 既に処理中のIssueは重複処理されない

### Feature 2: `quedex.toml` トラッカー設定 (Must)
**User Story**: 開発者として、トラッカー接続設定をプロジェクト設定ファイルに記述したい。

**設定スキーマ**:
```toml
[tracker]
type = "linear"            # "linear" | "github"
project = "my-project"     # プロジェクトslug

[tracker.polling]
interval_sec = 30          # ポーリング間隔
max_concurrent_issues = 3  # 同時処理最大Issue数

[tracker.states]
active = ["Todo", "Backlog"]        # 取得対象のステータス
in_progress = "In Progress"         # 実行中に設定するステータス
done = "Done"                       # 完了時に設定するステータス
failed = "Todo"                     # 失敗時に戻すステータス

[tracker.plan_generation]
runner = "claude_code"              # plan生成に使うrunner
model = "sonnet"                    # plan生成に使うmodel
system_prompt = """                 # plan生成用のsystem prompt（省略可）
このプロジェクトはRust製です。
"""
```

**API Key管理**:
- Linear: 環境変数 `LINEAR_API_KEY`
- GitHub: 環境変数 `GITHUB_TOKEN`（gh CLIの認証も利用可能）
- **設定ファイルにAPIキーを記載することは禁止**（パース時にエラーにする）

**Acceptance Criteria**:
- [ ] `quedex.toml`に`[tracker]`セクションを追加可能
- [ ] 環境変数未設定時に明確なエラーメッセージを表示
- [ ] CLIオプションが設定ファイルの値をオーバーライド可能
- [ ] APIキーフィールドが設定ファイルに含まれている場合はパースエラー

### Feature 3: Plan自動生成 (Must)
**User Story**: 開発者として、Issueの内容からplan.yamlを自動生成したい。

**生成フロー**:
1. Issue Trackerから対象Issueを取得（タイトル、本文、ラベル、コメント）
2. LLM（Claude Code or Codex CLI）にplan.yaml生成を依頼
3. 生成されたplan.yamlをバリデーション（既存の`Plan::validate()`を使用）
4. バリデーション成功 → `quedex run`で実行
5. バリデーション失敗 → LLMにエラーメッセージを渡してリトライ（最大3回）

**LLMへのprompt構成**:
```
以下のIssueからquedex plan.yamlを生成してください。

## Issue情報
- タイトル: {issue.title}
- 本文: {issue.body}
- ラベル: {issue.labels}

## プロジェクト情報
- リポジトリ構造: {tree出力の要約}
- 既存のplan.yamlスキーマ: {JSON Schema}

## 制約
- version: 1
- 各taskにはclaude_code or codex configが必須
- depsで依存関係を定義
- 出力はYAML形式のみ
```

**生成先**: `{store_dir}/watch/{issue_id}/plan.yaml`

**Acceptance Criteria**:
- [ ] IssueからLLMを使ってplan.yamlを生成できる
- [ ] 生成されたplan.yamlが`Plan::validate()`を通過する
- [ ] バリデーション失敗時にリトライする
- [ ] 3回リトライしても失敗する場合、Issueにコメントを付けてスキップ

### Feature 4: Blocker-Aware Dispatching (Should)
**User Story**: 開発者として、Issue間の依存関係を尊重した実行順序で処理したい。

**動作仕様**:
- Linear: `blocking` / `blockedBy` リレーションを取得
- GitHub: Issue本文中の `Blocked by #123` パターンを解析
- 依存先Issueが完了するまで、依存元Issueの処理を保留
- 循環依存を検出した場合はログに警告を出力し、すべてのIssueを独立として扱う

**Acceptance Criteria**:
- [ ] Linear blocking関係を取得してディスパッチ順序に反映
- [ ] GitHub Issues依存関係テキスト解析
- [ ] 循環依存の検出と警告
- [ ] 依存先完了後に自動的に依存元を処理開始

### Feature 5: ステータス同期 (Should)
**User Story**: 開発者として、quedexの実行状況がIssue Trackerに反映されてほしい。

**ステータスマッピング**:
| quedex状態 | Issue Tracker操作 |
|---|---|
| Plan生成中 | ステータスを`in_progress`に変更 |
| タスク実行中 | コメントで進捗を通知（オプション） |
| 全タスク完了 | ステータスを`done`に変更 |
| タスク失敗 | ステータスを`failed`に変更 + エラーコメント |

**コメント例**:
```
🤖 quedex: Plan実行完了
- 合計タスク: 5
- 成功: 4
- 失敗: 1
- 失敗タスク: `verify-tests` (exit code 1)
- Run ID: abc123
```

**Acceptance Criteria**:
- [ ] quedex実行開始時にIssueステータスを`in_progress`に更新
- [ ] 実行完了時にステータスを`done`に更新
- [ ] 失敗時にエラー詳細をコメントとして投稿
- [ ] ステータス更新失敗時にリトライ（最大3回）

---

## Phase 3: Technical Hints

### アーキテクチャ概要

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐     ┌───────────┐
│ Issue Tracker│────▶│  Watcher     │────▶│ Plan Generator│────▶│ Scheduler │
│ (Linear/GH) │◀────│  (Poller)    │     │ (LLM)        │     │ (既存)    │
└─────────────┘     └──────────────┘     └──────────────┘     └───────────┘
       ▲                   │                                        │
       │                   │          ステータス同期                 │
       └───────────────────┴────────────────────────────────────────┘
```

### モジュール構成
```
src/
├── tracker/
│   ├── mod.rs          # TrackerClient trait定義
│   ├── linear.rs       # Linear API client
│   ├── github.rs       # GitHub Issues client
│   └── types.rs        # Issue, IssueState等の共通型
├── watcher.rs          # watchループ、ディスパッチロジック
├── plan_generator.rs   # LLMによるplan.yaml生成
├── cli.rs              # Watchコマンド追加
└── config.rs           # TrackerConfig追加
```

### TrackerClient trait
```rust
#[async_trait]
pub trait TrackerClient: Send + Sync {
    /// アクティブなIssueの一覧を取得
    async fn fetch_active_issues(&self) -> Result<Vec<TrackerIssue>>;
    /// Issueのステータスを更新
    async fn update_status(&self, issue_id: &str, status: &str) -> Result<()>;
    /// Issueにコメントを追加
    async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()>;
    /// Issue間の依存関係を取得
    async fn fetch_dependencies(&self, issue_id: &str) -> Result<Vec<IssueDependency>>;
}
```

### 依存crate
- `reqwest` - HTTP client（既存依存の可能性あり）
- `graphql_client` - Linear GraphQL API用
- `octocrab` or `gh` CLI wrapper - GitHub Issues用

### 設計方針
- `TrackerClient`をtrait化し、Linear/GitHub/将来のトラッカーを統一的に扱う
- Watcherは既存のスケジューラ（`src/scheduler.rs`）をそのまま使用する
- Plan生成はsubprocess呼び出し（Claude CodeまたはCodex CLI）で行い、quedex自体にLLM統合は持たない
- 既存の`quedex run`コマンドのロジックを内部的に再利用する

### セキュリティ考慮
- APIキーは環境変数からのみ取得: `LINEAR_API_KEY`, `GITHUB_TOKEN`
- `quedex.toml`にAPIキー関連フィールドが存在する場合はパースエラーにする
- ポーリングで取得したIssue内容はplan.yaml生成にのみ使用し、ログにはIssue IDとタイトルのみ記録
- LLMに渡すpromptにはAPIキーを含めない

---

## Phase 4: Components

### Component 1: TrackerClient trait + 共通型
- **Files**: `src/tracker/mod.rs`, `src/tracker/types.rs`
- **Lock**: tracker

### Component 2: Linear Client
- **Files**: `src/tracker/linear.rs`
- **Lock**: tracker
- **Depends on**: Component 1

### Component 3: GitHub Client
- **Files**: `src/tracker/github.rs`
- **Lock**: tracker
- **Depends on**: Component 1

### Component 4: Config拡張
- **Files**: `src/config.rs`
- **Lock**: config.rs

### Component 5: Plan Generator
- **Files**: `src/plan_generator.rs`
- **Lock**: plan_generator.rs
- **Depends on**: Component 1 (TrackerIssue型)

### Component 6: Watcher
- **Files**: `src/watcher.rs`
- **Lock**: watcher.rs
- **Depends on**: Component 1, 4, 5

### Component 7: CLI拡張
- **Files**: `src/cli.rs`, `src/main.rs`
- **Lock**: cli.rs, main.rs
- **Depends on**: Component 4, 6

### Component 8: テスト・検証
- **Files**: `tests/`
- **No Lock**
- **Depends on**: All components

---

## Phase 5: Implementation Phases

### Phase A: MVP - Linear Polling + Plan生成 (Must)
1. TrackerClient trait定義 + Linear Client実装
2. Config拡張（`[tracker]`セクション）
3. Plan Generator（Claude Code subprocess呼び出し）
4. `quedex watch`コマンド（基本ポーリングループ）
5. ステータス同期（開始/完了/失敗のみ）

### Phase B: GitHub対応 + Blocker (Should)
1. GitHub Issues Client実装
2. Blocker-aware dispatching（Linear blocking + GitHub テキスト解析）
3. 進捗コメント投稿

### Phase C: UX改善 (Nice to have)
1. TUIにwatch状態の表示を追加
2. `quedex watch --status`で現在の監視状態を表示
3. Webhook受信モード（ポーリングの代替）

---

## Phase 6: Runner Selection

- **Default Runner**: claude_code
- **Model**: sonnet（plan生成）/ opus（実装タスク）
- **全タスク共通設定**

---

## 補足: 利用例

### 基本的なワークフロー

```bash
# 1. 環境変数を設定
export LINEAR_API_KEY="lin_api_xxxxx"

# 2. quedex.toml にトラッカー設定を追加
cat >> quedex.toml << 'EOF'
[tracker]
type = "linear"
project = "my-project"

[tracker.states]
active = ["Todo"]
in_progress = "In Progress"
done = "Done"
EOF

# 3. watchを開始
quedex watch --tracker linear --project my-project

# 4. LinearでIssueを「Todo」に移動すると自動的に:
#    - plan.yamlが生成される
#    - quedex runが実行される
#    - 完了後にステータスが「Done」に更新される
```

### dry-runで確認

```bash
# plan.yaml生成のみ（実行しない）
quedex watch --tracker linear --project my-project --dry-run

# 生成されたplanを確認
cat .quedex/watch/ISSUE-123/plan.yaml
```
