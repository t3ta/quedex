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

- `quedex run <plan.json|->`: 前面実行（終了コードで成功/失敗を返す）
- `quedex start <plan.json|->`: バックグラウンド実行（run_id を返す）
- `quedex status [run_id] [--json]`: 実行中/直近の run、または指定 run の状態
- `quedex logs <run_id> <task_id> [-f] [--stderr]`: ログ閲覧
- `quedex cancel <run_id> [task_id]`: run 全体または単一 task をキャンセル
- `quedex graph <plan|run_id> [--mermaid|--ascii]`: DAG 表示

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
  - `codex`: Codex 実行設定（`prompt`, `full_auto`, `output_last_message`, `sandbox`, `ask_for_approval` など）
  - `shell`: シェル実行設定（`command`）

バリデーション概要:

- `id` の一意性、`deps` の存在、DAG（循環なし）
- `codex.prompt` は必須
- `output_last_message` は `mode=research` のみ許可

## 使用例

- 複数タスクの調査 → 実装 → 検証を DAG で分解して並列化
- DB マイグレーションは `locks: ["db-migrate"]` で排他実行
- Codex とシェルを混在させ、実装後に `shell` でテストを実行
- 長時間タスクを `start` でバックグラウンド実行し、`status`/`logs` で追跡

## ライセンス

現時点ではライセンスは未設定です。公開時に追記予定です。
