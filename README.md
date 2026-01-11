# quedex

Codex CLI を中心に、LLM が生成した DAG 形式の plan を依存解決・並列実行・状態永続化まで含めて安全に回すための Rust 製 CLI です。LLM は「計画の生成」まで、実行・監視・状態管理は quedex が担います。

## プロジェクト概要

quedex は機械可読な plan (DAG) を受け取り、依存関係と排他制御を守りながらタスクを実行します。タスクは research / implement / verify を明示し、Codex CLI もしくは任意のシェルコマンドをバックエンドとして動かせます。

## 特徴

- DAG ベースの依存解決とスケジューリング
- Codex CLI との統合（非対話デフォルト）
- 並列実行とグローバルな同時実行数制御
- locks による排他制御（workspace / db-migrate など）
- 状態とログの永続化（落ちても復元できる設計）
- **リアルタイム TUI 監視**（タスク状態・ログ表示）
- **失敗タスクの再実行**（retry コマンド）
- **状態復元**（プロセス死亡検出と途中再開）

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

`plan.json` を作成して実行します。

```json
{
  "version": 1,
  "run": {
    "name": "demo",
    "cwd": ".",
    "max_concurrency": 2,
    "fail_fast": true,
    "default_timeout_sec": 3600
  },
  "tasks": [
    {
      "id": "A",
      "title": "調査: 既存の構成を把握",
      "mode": "research",
      "deps": [],
      "locks": [],
      "kind": "codex",
      "codex": {
        "prompt": "このリポジトリの構成を調査して要点をまとめて",
        "output_last_message": "artifacts/A_research.md",
        "sandbox": "workspace-write",
        "ask_for_approval": "never"
      }
    },
    {
      "id": "B",
      "title": "実装: 小さな改善",
      "mode": "implement",
      "deps": ["A"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "README にクイックスタートを追加して",
        "full_auto": true,
        "sandbox": "workspace-write",
        "ask_for_approval": "never"
      }
    }
  ]
}
```

実行:

```bash
quedex run plan.json
```

ローカルビルドで試す場合:

```bash
cargo run -- run plan.json
```

## コマンド一覧

### 基本コマンド

- `quedex run <plan.json|->`: 前面実行（終了コードで成功/失敗を返す）
  - `--resume`: 途中から再開（Running タスクのプロセス状態を確認）
  - `--clean-start`: 状態をクリアして最初から実行
- `quedex start <plan.json|->`: バックグラウンド実行（run_id を返す）
  - `--resume` / `--clean-start` オプションも使用可能
- `quedex status [run_id] [--json]`: 実行中/直近の run、または指定 run の状態
- `quedex logs <run_id> <task_id> [-f] [--stderr]`: ログ閲覧
- `quedex cancel <run_id> [task_id]`: run 全体または単一 task をキャンセル
- `quedex graph <plan|run_id> [--mermaid|--ascii]`: DAG 表示

### v1 新機能

- **`quedex tui [run_id]`**: リアルタイム TUI で実行状態を監視
  - タスク一覧（状態・経過時間・依存関係）
  - 選択タスクのログ表示（stdout/stderr 切替）
  - 全体統計（完了数・実行中・失敗数・locks 状態）
  - キーバインド:
    - `↑↓`: タスク選択
    - `Enter`: ログフォーカス
    - `t`: stdout/stderr 切替
    - `r`: retry（失敗タスクを再実行）
    - `c`: cancel task
    - `C`: cancel run
    - `g`: graph 表示
    - `q`: quit
- **`quedex retry <run_id> <task_id>`**: 失敗/キャンセルされたタスクを再実行
  - 依存関係が満たされている場合のみ実行可能

## Plan スキーマ（JSON）

最小構成:

```json
{
  "version": 1,
  "run": { "name": "demo", "cwd": ".", "max_concurrency": 2 },
  "tasks": [
    {
      "id": "A",
      "title": "調査: 既存実装の把握",
      "mode": "research",
      "deps": [],
      "locks": [],
      "kind": "codex",
      "codex": { "prompt": "調査して要点をまとめて" }
    }
  ]
}
```

主要フィールド:

- `version`: スキーマバージョン（v1）
- `run`: 実行全体の設定
  - `name`: run 名
  - `cwd`: 実行基準ディレクトリ
  - `env`: 追加環境変数
  - `max_concurrency`: 同時実行数
  - `fail_fast`: 失敗時の停止方針
  - `default_timeout_sec`: デフォルトタイムアウト
- `tasks`: タスク配列
  - `id`: タスク ID（ユニーク）
  - `title`: 説明
  - `mode`: `research | implement | verify`
  - `deps`: 依存タスク ID
  - `locks`: 排他リソース名
  - `timeout_sec`: タスク個別タイムアウト
  - `kind`: `codex | shell`
  - `codex`: Codex 実行設定
    - `prompt`: 実行プロンプト（必須）
    - `output_last_message`: 最終メッセージを保存するパス（research モードのみ）
    - `verify_after`: 実装後に build→lint→test を実行（implement モード）
    - `sandbox`: サンドボックスモード（`workspace-write`, `danger-full-access` など）
    - `ask_for_approval`: 承認モード（`never`, `on-request` など）
    - `json`: JSONL形式でイベントを出力（デフォルト: `true`、TUI で進捗を見る場合に推奨）
  - `shell`: シェル実行設定（`command`）

バリデーション概要:

- `id` の一意性、`deps` の存在、DAG（循環なし）
- `codex.prompt` は必須
- `output_last_message` は `mode=research` のみ許可

## 使用例

### 基本的な使い方

- 複数タスクの調査 → 実装 → 検証を DAG で分解して並列化
- DB マイグレーションは `locks: ["db-migrate"]` で排他実行
- Codex とシェルを混在させ、実装後に `shell` でテストを実行
- 長時間タスクを `start` でバックグラウンド実行し、`status`/`logs` で追跡

### v1 機能の活用

**TUI でリアルタイム監視:**

```bash
# planを実行
quedex start plan.json
# → run_id: abc123...

# TUIで監視
quedex tui abc123
```

**失敗タスクの再実行:**

```bash
# 失敗したタスクを確認
quedex status abc123

# 特定タスクを再実行
quedex retry abc123 task-id
```

**プロセス死亡からの復元:**

```bash
# quedex が途中で落ちた場合、Running タスクのプロセスを確認して復元
quedex run plan.json --resume

# または、状態をクリアして最初から
quedex run plan.json --clean-start
```

## ライセンス

現時点ではライセンスは未設定です。公開時に追記予定です。
