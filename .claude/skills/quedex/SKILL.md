---
name: quedex
description: DAG-based task execution with Codex CLI integration. Use when users ask to (1) create execution plans for multi-step implementation tasks, (2) execute tasks using quedex, (3) monitor quedex runs with TUI, or (4) manage failed tasks with retry. Triggers include "create a plan", "use quedex", "execute with quedex", "monitor quedex", "retry failed task", or references to DAG/parallel execution workflows.
---

# Quedex

## Overview

Generate and execute DAG-based task plans. Quedex handles dependency resolution, parallel execution, state persistence, and failure recovery while LLMs focus on planning.

## ⚠️ Plan作成の原則: 最小限のフィールドのみ使う

**省略できるフィールドは省略する。** 以下は書かない：
- `deps: []` → 依存がないなら省略
- `locks: []` → ロックが不要なら省略
- `kind: "codex"` → runner設定から自動推論される
- `json: true` → デフォルトがtrue
- `cwd: "."` → デフォルトがカレントディレクトリ
- `verify_after: true` → implementモードのデフォルト
- `variables` → 同じ値を複数箇所で使う場合のみ

## Quick Start

### 1. 最小限のplan例

```json
{
  "version": 1,
  "run": { "name": "add-feature" },
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "codex": {
        "prompt": "認証機能の実装を調査して",
        "output_last_message": "artifacts/research.md"
      }
    },
    {
      "id": "implement",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "codex": { "prompt": "artifacts/research.md を参考にパスワードリセット機能を実装して" }
    }
  ]
}
```

**ポイント:** 必要なフィールドだけ。`title`, `kind`, `json`, `verify_after` 等は省略。

**詳細**: [schema.md](references/schema.md) | [examples.md](references/examples.md)

### 2. Execute

```bash
quedex run plan.yaml          # Foreground
quedex start plan.yaml        # Background → returns run_id
quedex tui <run_id>           # Monitor with TUI
quedex serve -p 8080          # Web dashboard
```

### 3. Monitor and recover

```bash
quedex status [run_id] [--group <name>]    # Check status
quedex logs <run_id> <task_id>             # View logs
quedex outputs <run_id> [--task <id>]      # View output files
quedex retry <run_id> <task_id>            # Retry failed task
quedex retry <run_id> --group <name>       # Retry group
```

## CLI Commands

### Execution
- `quedex init [-o path]`: Generate plan template
- `quedex run <plan>`: Foreground execution
- `quedex start <plan>`: Background execution
- `quedex dry-run <plan> [--show-order] [--mermaid]`: Analyze plan

### Monitoring
- `quedex status [run_id] [--group name]`: Check status
- `quedex logs <run_id> <task_id> [-f] [--stderr]`: View logs
- `quedex outputs <run_id> [--task id]`: View output files
- `quedex tui [run_id]`: Interactive TUI
- `quedex serve [-p port]`: Web dashboard
- `quedex history [-n limit]`: Execution history
- `quedex stats [--since duration]`: Statistics

### Management
- `quedex retry <run_id> [task_id] [--group name]`: Retry tasks
- `quedex cancel <run_id> [task_id] [--group name]`: Cancel tasks
- `quedex clean [run_id] [--all] [--fix-orphans]`: Cleanup
- `quedex graph <plan|run_id> [--mermaid]`: Show DAG
- `quedex schema`: Output JSON schema

## Creating Plans

### Workflow decision tree

1. **Single simple task?** → Don't use quedex
2. **Multi-step with dependencies?** → Create DAG plan
3. **Need context first?** → Use research → implement pattern
4. **Multiple independent areas?** → Parallel research, then synthesize
5. **Sequential phases?** → Chain with deps

### Task modes

- **research**: 調査用。`output_last_message` で結果を保存
- **implement**: 実装用。自動でbuild/lint/test実行（`verify_after`デフォルトtrue）
- **verify**: テスト・検証用

### Runners（1つ選ぶ、通常はcodex）

```json
"codex": { "prompt": "..." }
"claude_code": { "prompt": "...", "model": "opus" }
"opencode": { "prompt": "...", "model": "gpt-4" }
```

## 高度な機能（必要なときだけ使う）

以下は**本当に必要な場合のみ**使用。シンプルなplanでは不要：

| 機能 | いつ使うか |
|------|----------|
| `locks` | 複数タスクが同じファイルを編集する場合 |
| `groups` | 大量のタスクをまとめて操作したい場合 |
| `condition` | 環境や前タスク結果で分岐が必要な場合 |
| `retry_count` | 不安定なタスク（E2Eテスト等）の場合 |
| `variables` | 同じ値を3箇所以上で使う場合 |
| `output_files` | 特定ファイルをキャプチャしたい場合 |
| `auto_commit` | テストで無効化したい場合（デフォルトtrue） |
| `squash` | 最終統合タスクで全コミットを1つにまとめたい場合 |
| `system_prompt` | 全タスクに共通のコンテキストを渡したい場合 |

### system_prompt（共通プロンプト）

全タスクのプロンプトの前に追加される共通コンテキスト。プロジェクト全体（quedex.toml）またはプラン単位で定義可能。

**quedex.toml（プロジェクト全体）:**
```toml
system_prompt = """
このプロジェクトは Rust で書かれています。
コーディング規約:
- snake_case を使用
- エラー処理には anyhow を使用
"""
```

**プランファイル（プラン単位で上書き）:**
```yaml
version: 1
run:
  system_prompt: |
    このタスク群は認証機能の実装です。
    既存の auth モジュールとの整合性を保ってください。
tasks:
  - id: task-1
    mode: implement
    codex:
      prompt: "ログイン機能を実装して"
```

プラン単位の `system_prompt` が設定されている場合、quedex.toml の設定を上書きします。

## TUI Key Bindings

- `↑↓`: Select task
- `Enter`: Focus logs
- `t`: Toggle stdout/stderr
- `r`: Retry failed task
- `c`: Cancel task
- `C`: Cancel run
- `g`: Show graph / toggle group collapse
- `q`: Quit

## Tips

### quedexを使うべきケース
- 複数ステップの実装（依存関係あり）
- 並列で複数領域を調査
- 長時間のバックグラウンド実行

### quedexを使うべきでないケース
- 単一のシンプルなタスク → 直接実行
- 対話的なワークフロー

### Plan作成のベストプラクティス
1. **フィールドは最小限に** - 省略できるものは書かない
2. **depsは本当に必要なときだけ** - 不要な依存は並列性を下げる
3. **locksは慎重に** - 本当に競合する場合のみ
4. **variablesは3箇所以上で使う場合のみ** - 1-2箇所なら直書き
