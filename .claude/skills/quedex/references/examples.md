# Common Quedex Patterns and Examples

## Basic Patterns

### Single Research Task

Explore a codebase and save findings:

```json
{
  "version": 1,
  "run": {
    "name": "explore-auth",
    "cwd": ".",
    "max_concurrency": 1
  },
  "tasks": [
    {
      "id": "research-auth",
      "title": "調査: 認証の実装状況",
      "mode": "research",
      "deps": [],
      "locks": [],
      "kind": "codex",
      "codex": {
        "prompt": "このプロジェクトの認証実装を調査して、使用している技術とフローを説明して",
        "output_last_message": "artifacts/auth-research.md",
        "sandbox": "workspace-write",
        "json": true
      }
    }
  ]
}
```

**Use when:** Need to understand existing code before making changes.

### Research → Implement Pipeline

Standard two-phase workflow:

```json
{
  "version": 1,
  "run": {
    "name": "add-feature",
    "cwd": ".",
    "max_concurrency": 1,
    "fail_fast": true
  },
  "tasks": [
    {
      "id": "research",
      "title": "調査: 既存のログイン実装",
      "mode": "research",
      "deps": [],
      "locks": [],
      "kind": "codex",
      "codex": {
        "prompt": "ログイン機能の実装箇所を調査し、パターンを把握して",
        "output_last_message": "artifacts/login-analysis.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "implement",
      "title": "実装: パスワードリセット機能",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "artifacts/login-analysis.md を参考に、既存パターンに従ってパスワードリセット機能を実装して",
        "verify_after": true,
        "json": true
      }
    }
  ]
}
```

**Use when:** Need context before implementing to match existing patterns.

### Parallel Research Tasks

Investigate multiple areas simultaneously:

```json
{
  "version": 1,
  "run": {
    "name": "multi-research",
    "cwd": ".",
    "max_concurrency": 3
  },
  "tasks": [
    {
      "id": "research-api",
      "title": "調査: API構造",
      "mode": "research",
      "deps": [],
      "kind": "codex",
      "codex": {
        "prompt": "API のエンドポイント設計を調査して",
        "output_last_message": "artifacts/api.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "research-db",
      "title": "調査: データベーススキーマ",
      "mode": "research",
      "deps": [],
      "kind": "codex",
      "codex": {
        "prompt": "データベーススキーマを調査して",
        "output_last_message": "artifacts/db.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "research-auth",
      "title": "調査: 認証フロー",
      "mode": "research",
      "deps": [],
      "kind": "codex",
      "codex": {
        "prompt": "認証フローを調査して",
        "output_last_message": "artifacts/auth.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "synthesize",
      "title": "実装: 統合実装",
      "mode": "implement",
      "deps": ["research-api", "research-db", "research-auth"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "artifacts/*.md の調査結果を踏まえて、新しい機能を実装して",
        "verify_after": true,
        "json": true
      }
    }
  ]
}
```

**Use when:** Large feature requires understanding multiple subsystems.

## Advanced Patterns

### Research → Multi-Phase Implementation

Complex features with staged rollout:

```json
{
  "version": 1,
  "run": {
    "name": "feature-rollout",
    "cwd": ".",
    "max_concurrency": 2,
    "fail_fast": true,
    "default_timeout_sec": 3600
  },
  "tasks": [
    {
      "id": "research",
      "title": "調査: 既存実装",
      "mode": "research",
      "deps": [],
      "kind": "codex",
      "codex": {
        "prompt": "既存のユーザー管理実装を調査",
        "output_last_message": "artifacts/user-mgmt.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "impl-models",
      "title": "実装: データモデル",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "ユーザープロフィール用のデータモデルを追加",
        "json": true
      }
    },
    {
      "id": "impl-api",
      "title": "実装: API エンドポイント",
      "mode": "implement",
      "deps": ["impl-models"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "プロフィール取得・更新の API エンドポイントを実装",
        "json": true
      }
    },
    {
      "id": "impl-ui",
      "title": "実装: UI コンポーネント",
      "mode": "implement",
      "deps": ["impl-api"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "プロフィール編集画面を実装",
        "json": true
      }
    },
    {
      "id": "verify",
      "title": "検証: E2E テスト",
      "mode": "verify",
      "deps": ["impl-ui"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "プロフィール機能の E2E テストを実行して、失敗があれば修正",
        "json": true
      }
    }
  ]
}
```

**Use when:** Feature requires multiple distinct implementation phases.

### Mixing Runners (Codex + Claude Code)

Combine different runners based on task needs:

```yaml
version: 1
run:
  name: "mixed-runners"
  max_concurrency: 2

tasks:
  - id: research
    mode: research
    codex:
      prompt: "既存実装を調査して"
      output_last_message: "artifacts/research.md"

  - id: implement
    mode: implement
    deps: [research]
    locks: [workspace]
    claude_code:
      prompt: "artifacts/research.md を参考に実装して"
      model: opus

  - id: verify
    mode: verify
    deps: [implement]
    codex:
      prompt: "npm test を実行して、失敗があれば修正"
```

**Use when:** Different tasks benefit from different models/runners.

### Database Migrations with Locks

Prevent concurrent schema changes:

```json
{
  "version": 1,
  "run": {
    "name": "migrations",
    "cwd": ".",
    "max_concurrency": 3
  },
  "tasks": [
    {
      "id": "migration-users",
      "title": "マイグレーション: users テーブル",
      "mode": "implement",
      "deps": [],
      "locks": ["db-migrate"],
      "kind": "codex",
      "codex": {
        "prompt": "users テーブルにカラムを追加するマイグレーションを作成して実行",
        "json": true
      }
    },
    {
      "id": "migration-posts",
      "title": "マイグレーション: posts テーブル",
      "mode": "implement",
      "deps": [],
      "locks": ["db-migrate"],
      "kind": "codex",
      "codex": {
        "prompt": "posts テーブルにカラムを追加するマイグレーションを作成して実行",
        "json": true
      }
    }
  ]
}
```

**Use when:** Tasks must not run simultaneously even though no dependency exists.

## Full Dogfooding Example

Real example from quedex v1 development (using quedex to build quedex):

```json
{
  "version": 1,
  "run": {
    "name": "quedex-v1",
    "cwd": ".",
    "max_concurrency": 2,
    "fail_fast": true,
    "default_timeout_sec": 3600
  },
  "tasks": [
    {
      "id": "retry-command",
      "title": "実装: retry コマンド",
      "mode": "implement",
      "deps": [],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "失敗したタスクを再実行する retry コマンドを実装...",
        "verify_after": true,
        "json": true
      }
    },
    {
      "id": "tui-module",
      "title": "実装: TUI モジュール",
      "mode": "implement",
      "deps": [],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "ratatui を使ってリアルタイム TUI を実装...",
        "verify_after": true,
        "json": true
      }
    },
    {
      "id": "tui-command",
      "title": "実装: tui コマンド",
      "mode": "implement",
      "deps": ["tui-module"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "TUI モジュールを呼び出す tui コマンドを main.rs に統合...",
        "verify_after": true,
        "json": true
      }
    },
    {
      "id": "state-recovery",
      "title": "実装: 状態復元機能",
      "mode": "implement",
      "deps": [],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "プロセス死亡検出と状態復元を実装...",
        "verify_after": true,
        "json": true
      }
    },
    {
      "id": "integration-test",
      "title": "実装: v1 機能の統合テスト",
      "mode": "implement",
      "deps": ["retry-command", "tui-command", "state-recovery"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "retry, tui, recovery 機能の統合テストを作成...",
        "json": true
      }
    },
    {
      "id": "build-and-test",
      "title": "検証: ビルドとテスト実行",
      "mode": "verify",
      "deps": ["integration-test"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "cargo build && cargo test && cargo clippy を実行...",
        "json": true
      }
    },
    {
      "id": "update-readme",
      "title": "実装: README 更新",
      "mode": "implement",
      "deps": ["build-and-test"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "v1 機能を README.md に追加...",
        "json": true
      }
    }
  ]
}
```

**Demonstrates:**
- Parallel implementation of independent features
- Sequential dependencies where needed
- Workspace locks to prevent conflicts
- Verification integrated into workflow
- Real production usage

## Tips

### When to use research mode

- Understanding unfamiliar codebases
- Exploring API designs before implementation
- Documenting existing patterns
- Gathering context for complex changes

### When to use implement mode

- Adding new features
- Refactoring existing code
- Writing documentation
- Creating configuration files

### When to use verify mode

- Running tests
- Checking build output
- Validating deployments
- Confirming fixes

### Locks strategy

Use locks when:
- Multiple tasks modify same files (e.g., `locks: ["workspace"]`)
- Database migrations must run sequentially (e.g., `locks: ["db-migrate"]`)
- Tasks share stateful resources (e.g., `locks: ["test-db"]`)

Don't use locks when:
- Tasks operate on different files/modules
- True parallel execution is safe and desired

### Concurrency tuning

```json
"max_concurrency": 1  // Sequential execution
"max_concurrency": 2  // Moderate parallelism (safe default)
"max_concurrency": 4  // Aggressive parallelism (fast, more resource usage)
```

Omit for unlimited concurrency (controlled only by dependencies and locks).

## New Features Examples

### Task Groups

Organize tasks into logical groups for batch operations:

```yaml
version: 1
groups:
  backend: [api-research, api-impl, api-test]
  frontend: [ui-research, ui-impl, ui-test]

run:
  name: "grouped-tasks"
  max_concurrency: 4

tasks:
  - id: api-research
    mode: research
    codex:
      prompt: "API構造を調査"
      output_last_message: "artifacts/api.md"

  - id: api-impl
    mode: implement
    deps: [api-research]
    locks: [backend]
    claude_code:
      prompt: "API を実装"
      model: sonnet

  - id: api-test
    mode: verify
    deps: [api-impl]
    codex:
      prompt: "API テストを実行"

  - id: ui-research
    mode: research
    codex:
      prompt: "UI構造を調査"
      output_last_message: "artifacts/ui.md"

  - id: ui-impl
    mode: implement
    deps: [ui-research]
    locks: [frontend]
    claude_code:
      prompt: "UI を実装"
      model: sonnet

  - id: ui-test
    mode: verify
    deps: [ui-impl]
    codex:
      prompt: "UI テストを実行"
```

**CLI usage:**
```bash
quedex status <run_id> --group backend
quedex retry <run_id> --group frontend
quedex cancel <run_id> --group backend
```

### Conditional Execution

Skip tasks based on environment or previous task results:

```yaml
version: 1
run:
  name: "conditional"
  max_concurrency: 2

tasks:
  - id: build
    mode: implement
    codex:
      prompt: "ビルドを実行"

  - id: deploy-staging
    mode: implement
    deps: [build]
    condition:
      task: build
      status: succeeded
    codex:
      prompt: "ステージングにデプロイ"

  - id: deploy-prod
    mode: implement
    deps: [deploy-staging]
    condition:
      env: "DEPLOY_PROD"
      equals: "true"
    codex:
      prompt: "本番にデプロイ"

  - id: notify-failure
    mode: implement
    deps: [build]
    condition:
      task: build
      status: failed
    codex:
      prompt: "失敗を通知"
```

### Output File Capture

Capture task outputs for later reference:

```yaml
version: 1
run:
  name: "with-outputs"

tasks:
  - id: generate-report
    mode: implement
    output_files:
      - "reports/summary.md"
      - "reports/metrics.json"
    codex:
      prompt: "レポートを生成して reports/ に保存"

  - id: analyze
    mode: research
    deps: [generate-report]
    codex:
      prompt: "reports/summary.md を分析して"
      output_last_message: "artifacts/analysis.md"
```

**CLI usage:**
```bash
quedex outputs <run_id> --task generate-report
```

### Automatic Retry

Configure automatic retry for flaky tasks:

```yaml
version: 1
run:
  name: "with-retry"

tasks:
  - id: flaky-test
    mode: verify
    retry_count: 3
    retry_delay_sec: 30
    codex:
      prompt: "E2E テストを実行（ネットワーク依存あり）"

  - id: stable-task
    mode: implement
    deps: [flaky-test]
    codex:
      prompt: "次のステップを実行"
```

### Dynamic Timeout

Use historical data for smart timeouts:

```yaml
version: 1
run:
  name: "dynamic-timeout"

tasks:
  - id: quick-task
    mode: research
    timeout_sec: 300  # Fixed 5 minutes
    codex:
      prompt: "簡単な調査"

  - id: variable-task
    mode: implement
    timeout_sec: "auto"  # Average + 2σ from history
    codex:
      prompt: "実行時間が変動するタスク"

  - id: long-task
    mode: implement
    timeout_sec: "2x_average"  # 2× average from history
    codex:
      prompt: "長時間かかる可能性があるタスク"
```

### Template Variables

Use variables for reusable prompts:

```yaml
version: 1
variables:
  target_module: "src/auth"
  test_command: "npm test -- --coverage"
  output_dir: "artifacts"

run:
  name: "templated"

tasks:
  - id: research
    mode: research
    codex:
      prompt: "${target_module} を調査して ${output_dir}/research.md に保存"
      output_last_message: "${output_dir}/research.md"

  - id: implement
    mode: implement
    deps: [research]
    codex:
      prompt: |
        ${target_module} を改善して
        完了後 ${test_command} を実行
```

Environment variables can also be used: `${env.CI}`, `${env.NODE_ENV}`
