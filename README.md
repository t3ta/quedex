# quedex

LLM が生成した DAG 形式の plan を依存解決・並列実行・状態永続化まで含めて安全に回すための Rust 製 CLI です。LLM は「計画の生成」まで、実行・監視・状態管理は quedex が担います。

## プロジェクト概要

quedex は機械可読な plan (DAG) を受け取り、依存関係と排他制御を守りながらタスクを実行します。タスクは research / implement / verify を明示し、Codex CLI、Claude Code、Opencode をバックエンドとして動かせます。

## 特徴

- DAG ベースの依存解決とスケジューリング
- **複数ランナー対応**: Codex CLI / Claude Code / Opencode
- 並列実行とグローバルな同時実行数制御
- locks による排他制御（workspace / db-migrate など）
- 状態とログの永続化（落ちても復元できる設計）
- **リアルタイム TUI 監視**（タスク状態・ログ表示）
- **失敗タスクの再実行**（retry コマンド、自動リトライ対応）
- **状態復元**（プロセス死亡検出と途中再開）
- **タスクグループ機能**（論理的なグループ化と一括操作）
- **条件付き実行**（環境変数や前タスクの結果に基づく実行制御）
- **Web ダッシュボード**（リアルタイム監視）
- **出力ファイルキャプチャ**（タスク成果物の自動収集）

## インストール方法

### cargo install

```bash
cargo install --path .
```

### cargo build

```bash
cargo build --release
```

## クイックスタート

### 1. plan ファイルを生成

```bash
quedex init -o plan.yaml
```

### 2. plan.yaml を編集

```yaml
version: 1
run:
  name: "demo"
  max_concurrency: 2

tasks:
  - id: research
    title: "調査: 既存の構成を把握"
    mode: research
    codex:
      prompt: "このリポジトリの構成を調査して要点をまとめて"
      output_last_message: "artifacts/research.md"

  - id: implement
    title: "実装: 改善"
    mode: implement
    deps: [research]
    locks: [workspace]
    claude_code:
      prompt: "調査結果を踏まえてREADMEを改善して"
      model: sonnet
```

### 3. 実行

```bash
# 前面実行
quedex run plan.yaml

# バックグラウンド実行
quedex start plan.yaml
# → run_id: abc123...

# TUI で監視
quedex tui abc123
```

### ローカルビルドで試す場合

```bash
cargo run -- run plan.yaml
```

## コマンド一覧

### 基本コマンド

- **`quedex init [-o <path>] [--force]`**: plan テンプレートを生成（JSON/YAML）
- **`quedex run <plan.json|->` / `quedex start <plan.json|->`**: plan を実行
  - `--resume`: 途中から再開（Running タスクのプロセス状態を確認）
  - `--clean-start`: 状態をクリアして最初から実行
  - `--dry-run`: 実行せずに計画を表示
- **`quedex status [run_id] [--json] [--group <name>]`**: 実行状態を表示
- **`quedex logs <run_id> <task_id> [-f] [--stderr]`**: ログ閲覧
- **`quedex outputs <run_id> [--task <id>]`**: タスクの出力ファイルを表示
- **`quedex cancel <run_id> [task_id] [--group <name>]`**: キャンセル
- **`quedex clean [run_id] [--all] [--fix-orphans]`**: クリーンアップ
- **`quedex graph <plan|run_id> [--mermaid|--ascii]`**: DAG 表示

### 監視・分析コマンド

- **`quedex tui [run_id]`**: リアルタイム TUI で実行状態を監視
  - タスク一覧（状態・経過時間・依存関係・グループ）
  - 選択タスクのログ表示（stdout/stderr 切替）
  - 全体統計（完了数・実行中・失敗数・locks 状態）
  - キーバインド:
    - `↑↓`: タスク選択
    - `Enter`: ログフォーカス
    - `t`: stdout/stderr 切替
    - `r`: retry（失敗タスクを再実行）
    - `c`: cancel task
    - `C`: cancel run
    - `g`: graph 表示 / グループ折りたたみ切替
    - `q`: quit
- **`quedex serve [run_id] [-p <port>]`**: Web ダッシュボードを起動
- **`quedex history [-n <limit>] [--all] [--json]`**: 実行履歴を表示
- **`quedex stats [--since <duration>] [--json]`**: 実行統計を表示

### 再実行・分析コマンド

- **`quedex retry <run_id> [task_id] [--group <name>] [--reload-plan]`**: 失敗タスクを再実行
  - 依存関係が満たされている場合のみ実行可能
  - `--group`: グループ内の失敗タスクを一括再実行
  - `--reload-plan`: plan ファイルを再読み込みして再実行
- **`quedex dry-run <plan> [--show-order] [--check-locks] [--mermaid]`**: 実行計画を分析
- **`quedex schema [-o <path>]`**: plan の JSON Schema を出力

## Plan スキーマ（JSON / YAML）

quedex は JSON と YAML の両方の plan ファイルをサポートしています。

### JSON 形式（最小構成）

```json
{
  "version": 1,
  "run": { "name": "demo", "cwd": ".", "max_concurrency": 2 },
  "tasks": [
    {
      "id": "A",
      "title": "調査: 既存実装の把握",
      "mode": "research",
      "codex": { "prompt": "調査して要点をまとめて" }
    }
  ]
}
```

### YAML 形式

YAML は可読性が高く、コメントも書けるため推奨フォーマットです。

```yaml
version: 1

variables:
  target_module: "src/auth"
  test_command: "npm test"

# タスクグループの定義
groups:
  backend: [research, implement]
  verify: [test]

run:
  name: "auth-feature"
  max_concurrency: 2
  fail_fast: true

tasks:
  - id: research
    title: "調査: 既存認証の把握"
    mode: research
    kind: codex
    codex:
      prompt: "${target_module} の既存実装を調査して要点をまとめて"
      output_last_message: "artifacts/research.md"

  - id: implement
    title: "実装: 認証API"
    mode: implement
    deps: [research]
    locks: [workspace]
    kind: claude_code
    claude_code:
      prompt: "認証APIを実装し、${test_command} でテストを実行して"
      model: opus

  - id: test
    title: "検証: 条件付きテスト"
    mode: verify
    deps: [implement]
    condition:
      task: implement
      status: succeeded
    codex:
      prompt: "テストを実行して"
```

### 主要フィールド

**ルートレベル:**
- `version`: スキーマバージョン（v1）
- `variables`: テンプレート変数の定義
- `groups`: タスクグループの定義（グループ名 → タスクIDリスト）

**run（実行設定）:**
- `name`: run 名
- `cwd`: 実行基準ディレクトリ
- `env`: 追加環境変数
- `max_concurrency`: 同時実行数
- `fail_fast`: 失敗時の停止方針
- `default_timeout_sec`: デフォルトタイムアウト
- `worktree`: Git worktree 設定（`enabled`, `base_dir`, `shallow_depth`）
- `notifications`: Webhook 通知設定

**tasks（タスク配列）:**
- `id`: タスク ID（ユニーク、英数字・`_`・`-`のみ）
- `title`: 説明
- `mode`: `research | implement | verify`
- `group`: 所属グループ名（オプション）
- `deps`: 依存タスク ID
- `locks`: 排他リソース名
- `timeout_sec`: タイムアウト（秒数または `"auto"` / `"2x_average"`）
- `retry_count`: 失敗時の自動リトライ回数
- `retry_delay_sec`: リトライ間隔（秒）
- `output_files`: 出力として収集するファイルパス（相対パス）
- `condition`: 条件付き実行（環境変数または前タスクの結果）
- `no_worktree`: worktree を使用しない（デフォルト: false）

**ランナー設定（いずれか一つ）:**

- `codex`: Codex CLI 実行設定
  - `prompt`: 実行プロンプト（必須）
  - `output_last_message`: 最終メッセージを保存するパス（research モードのみ）
  - `verify_after`: 実装後に build→lint→test を実行（デフォルト: true）
  - `sandbox`: サンドボックスモード（research モードのみ）
  - `json`: JSONL 出力（デフォルト: true）
  - **注意**: implement/verify モードでは `--dangerously-bypass-approvals-and-sandbox` が自動使用

- `claude_code`: Claude Code 実行設定
  - `prompt`: 実行プロンプト（必須）
  - `model`: モデル指定（`sonnet`, `opus` など）
  - `json`: JSONL 出力（デフォルト: true）

- `opencode`: Opencode 実行設定
  - `prompt`: 実行プロンプト（必須）
  - `model`: モデル指定
  - `json`: JSONL 出力（デフォルト: true）

### 条件付き実行

タスクの実行を条件で制御できます:

```yaml
# 環境変数による条件
condition:
  env: "CI"
  equals: "true"

# 前タスクの結果による条件
condition:
  task: "build"
  status: succeeded  # または failed
```

### バリデーション

- `id` の一意性、`deps` の存在、DAG（循環なし）
- ランナー設定（`codex` / `claude_code` / `opencode`）は必須で排他
- `output_last_message` は `mode=research` のみ許可
- `output_files` は相対パスのみ（`..` や絶対パス禁止）
- グループ内のタスクID存在チェック、重複チェック

## プロンプトテンプレート

プロンプト内で変数展開が使えます。

### 変数の定義

plan ファイルの `variables` セクションで定義:

```yaml
variables:
  project_name: "my-app"
  target_dir: "src/components"
```

### 変数の使用

`${variable}` 形式でプロンプト内から参照:

```yaml
tasks:
  - id: analyze
    title: "コード分析"
    mode: research
    kind: codex
    codex:
      prompt: "${target_dir} 内の ${project_name} コンポーネントを分析して"
```

### 環境変数の参照

`${env.VAR}` で環境変数を参照:

```yaml
tasks:
  - id: deploy
    title: "デプロイ"
    mode: implement
    kind: codex
    codex:
      prompt: "${env.DEPLOY_TARGET} 環境にデプロイして"
```

## quedex.toml 設定ファイル

プロジェクトルートに `quedex.toml` を配置すると、デフォルト設定を指定できます。
CLI オプションが優先され、指定がない場合に設定ファイルの値が使われます。

```toml
# 同時実行タスク数（デフォルト: plan の指定値）
max_concurrency = 4

# 失敗時に即停止するか（デフォルト: true）
fail_fast = false

# 状態保存ディレクトリ（デフォルト: .quedex）
store = ".quedex"

# デフォルトタイムアウト（秒）
default_timeout_sec = 3600
```

## Webhook 通知

Slack や Discord に実行状況を通知できます。

### 設定例

```yaml
run:
  name: "daily-task"
  notifications:
    url: "https://hooks.slack.com/services/XXX/YYY/ZZZ"
    events: ["on_start", "on_complete", "on_failure"]
    username: "quedex-bot"
```

### 通知イベント

| イベント | 説明 |
|---------|------|
| `on_start` | run 開始時 |
| `on_task_complete` | 各タスク完了時 |
| `on_complete` | 全タスク成功時 |
| `on_failure` | run 失敗時 |

`events` を省略すると全イベントが通知されます。

### Discord での使用

Discord の Webhook URL もそのまま使用可能:

```yaml
notifications:
  url: "https://discord.com/api/webhooks/XXX/YYY"
  events: ["on_complete", "on_failure"]
```

## 使用例

### 基本的な使い方

- 複数タスクの調査 → 実装 → 検証を DAG で分解して並列化
- DB マイグレーションは `locks: ["db-migrate"]` で排他実行
- Codex / Claude Code / Opencode を混在させて実行
- 長時間タスクを `start` でバックグラウンド実行し、`status`/`logs` で追跡

### タスクグループの活用

```yaml
groups:
  backend: [api-research, api-impl]
  frontend: [ui-research, ui-impl]

tasks:
  - id: api-research
    mode: research
    # ...
```

```bash
# グループ単位で状態確認
quedex status abc123 --group backend

# グループ内の失敗タスクを一括再実行
quedex retry abc123 --group backend

# グループ内のタスクを一括キャンセル
quedex cancel abc123 --group frontend
```

### 出力ファイルのキャプチャ

タスクが生成するファイルを収集できます:

```yaml
tasks:
  - id: generate-report
    mode: implement
    output_files:
      - "reports/summary.md"
      - "reports/details.json"
    codex:
      prompt: "レポートを生成して reports/ に保存して"
```

```bash
# 出力ファイルを確認
quedex outputs abc123 --task generate-report
```

### TUI でリアルタイム監視

```bash
# planを実行
quedex start plan.yaml
# → run_id: abc123...

# TUIで監視
quedex tui abc123
```

### 失敗タスクの再実行

```bash
# 失敗したタスクを確認
quedex status abc123

# 特定タスクを再実行
quedex retry abc123 task-id

# plan を更新して再実行
quedex retry abc123 task-id --reload-plan
```

### プロセス死亡からの復元

```bash
# quedex が途中で落ちた場合、Running タスクのプロセスを確認して復元
quedex run plan.yaml --resume

# または、状態をクリアして最初から
quedex run plan.yaml --clean-start

# 孤立した run を修復
quedex clean --fix-orphans
```

### Web ダッシュボードで監視

```bash
# Web サーバーを起動
quedex serve abc123 -p 8080

# ブラウザで http://localhost:8080 にアクセス
```

### 実行統計の確認

```bash
# 過去7日間の統計
quedex stats --since 7d

# 実行履歴
quedex history -n 20
```

## ライセンス

現時点ではライセンスは未設定です。公開時に追記予定です。
