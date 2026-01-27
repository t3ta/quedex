# Plan JSON Schema Reference

## 原則: 最小限のフィールドのみ使う

**省略できるフィールドは省略する。** デフォルト値があるものは書かない。

### 省略すべきフィールド一覧

| フィールド | デフォルト | 書くべき場合 |
|-----------|----------|-------------|
| `deps` | `[]` | 依存がある場合のみ |
| `locks` | `[]` | 排他制御が必要な場合のみ |
| `kind` | (自動推論) | **常に省略** |
| `json` | `true` | **常に省略** |
| `cwd` | `.` | カレント以外で実行する場合のみ |
| `verify_after` | `true` | falseにしたい場合のみ |
| `sandbox` | `"workspace-write"` | 変更したい場合のみ |
| `title` | なし | 必要な場合のみ |
| `variables` | なし | 3箇所以上で同じ値を使う場合のみ |
| `groups` | なし | バッチ操作が必要な場合のみ |
| `system_prompt` | なし | 全タスクに共通コンテキストを渡す場合のみ |

---

## 最小限のplan例

```json
{
  "version": 1,
  "run": { "name": "feature" },
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "codex": { "prompt": "調査して", "output_last_message": "out.md" }
    },
    {
      "id": "impl",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "codex": { "prompt": "実装して" }
    }
  ]
}
```

---

## Root Structure

```json
{
  "version": 1,
  "variables": { "key": "value" },
  "groups": { "group-name": ["task-a", "task-b"] },
  "run": { /* RunConfig */ },
  "tasks": [ /* Task[] */ ]
}
```

### version (required)

- **Type**: `number`
- **Value**: Must be `1`

### variables (optional)

- **Type**: `object`
- **Purpose**: Template variables for prompt expansion
- **Usage**: Reference with `${variable}` in prompts
- **Environment variables**: Use `${env.VAR}` syntax

### groups (optional)

- **Type**: `object`
- **Purpose**: Logical grouping of tasks for batch operations
- **Format**: `{ "group-name": ["task-id-1", "task-id-2"] }`
- **Validation**: All referenced task IDs must exist; task cannot be in multiple groups

### run (required)

RunConfig object defining execution settings.

### tasks (required)

Array of Task objects. Must contain at least one task.

---

## RunConfig

```json
{
  "name": "demo",
  "cwd": ".",
  "env": { "KEY": "value" },
  "max_concurrency": 2,
  "fail_fast": true,
  "default_timeout_sec": 3600,
  "worktree": { "enabled": true },
  "notifications": { "url": "https://hooks.slack.com/..." },
  "system_prompt": "全タスク共通のコンテキスト"
}
```

### name (optional)

- **Type**: `string`
- **Purpose**: Human-readable run name

### cwd (optional)

- **Type**: `string`
- **Purpose**: Working directory for task execution
- **Default**: Current directory

### env (optional)

- **Type**: `object`
- **Purpose**: Additional environment variables

### max_concurrency (optional)

- **Type**: `number`
- **Purpose**: Maximum tasks running simultaneously
- **Default**: Unlimited

### fail_fast (optional)

- **Type**: `boolean`
- **Purpose**: Stop scheduling new tasks when any task fails
- **Default**: `false`

### default_timeout_sec (optional)

- **Type**: `number`
- **Purpose**: Default timeout for tasks in seconds

### worktree (optional)

- **Type**: `object`
- **Purpose**: Git worktree configuration for isolated execution
- **Fields**:
  - `enabled`: Enable worktree mode
  - `base_dir`: Base directory for worktrees
  - `shallow_depth`: Shallow clone depth

### notifications (optional)

- **Type**: `object`
- **Purpose**: Webhook notification configuration
- **Fields**:
  - `url`: Webhook URL (Slack/Discord compatible)
  - `events`: Array of events (`"on_start"`, `"on_task_complete"`, `"on_complete"`, `"on_failure"`)
  - `username`: Custom username for notifications

### system_prompt (optional)

- **Type**: `string`
- **Purpose**: 全タスクのプロンプトの前に追加される共通コンテキスト
- **Priority**: プラン単位の設定が `quedex.toml` の設定を上書き
- **Example**:
```yaml
run:
  system_prompt: |
    このプロジェクトは Rust で書かれています。
    コーディング規約:
    - snake_case を使用
    - エラー処理には anyhow を使用
```

---

## Task

```json
{
  "id": "task-a",
  "title": "Research existing implementation",
  "mode": "research",
  "group": "backend",
  "deps": ["task-b"],
  "locks": ["workspace"],
  "timeout_sec": 1800,
  "retry_count": 2,
  "retry_delay_sec": 30,
  "output_files": ["artifacts/report.md"],
  "condition": { "env": "CI", "equals": "true" },
  "no_worktree": false,
  "kind": "codex",
  "codex": { /* CodexConfig */ },
  "claude_code": { /* ClaudeCodeConfig */ },
  "opencode": { /* OpencodeConfig */ }
}
```

### id (required)

- **Type**: `string`
- **Validation**: Must be unique, non-empty, alphanumeric with `_` and `-` only

### title (optional)

- **Type**: `string`
- **Purpose**: Human-readable task description

### mode (required)

- **Type**: `"research" | "implement" | "verify"`
- **Behaviors**:
  - `research`: Sandboxed exploration, saves output to file
  - `implement`: Full write access, automated code changes
  - `verify`: Full access for testing/validation

### group (optional)

- **Type**: `string`
- **Purpose**: Group this task belongs to (alternative to Plan.groups)

### deps (optional)

- **Type**: `string[]`
- **Purpose**: Task IDs that must complete before this task starts
- **Validation**: All IDs must exist, no circular dependencies

### locks (optional)

- **Type**: `string[]`
- **Purpose**: Exclusive resource names (prevents parallel execution)
- **Example**: `["workspace", "db-migrate"]`

### timeout_sec (optional)

- **Type**: `number | "auto" | "2x_average"`
- **Purpose**: Task-specific timeout
- **Dynamic options**:
  - `"auto"`: Calculate as average + 2σ from history
  - `"2x_average"`: Calculate as 2× average from history

### retry_count (optional)

- **Type**: `number`
- **Purpose**: Number of automatic retry attempts on failure
- **Default**: `0` (no retry)

### retry_delay_sec (optional)

- **Type**: `number`
- **Purpose**: Delay between retry attempts in seconds
- **Default**: `0`

### output_files (optional)

- **Type**: `string[]`
- **Purpose**: Files to capture as task outputs
- **Validation**: Relative paths only, no `..` or absolute paths
- **Usage**: View with `quedex outputs <run_id> --task <task_id>`

### condition (optional)

- **Type**: `object`
- **Purpose**: Conditional execution based on environment or task result

**Environment condition:**
```json
{
  "env": "CI",
  "equals": "true"
}
```
- `env`: Environment variable name
- `equals`: Value to match (optional)
- `not_equals`: Value to not match (optional)
- `exists`: Check existence (optional, boolean)

**Task result condition:**
```json
{
  "task": "build",
  "status": "succeeded"
}
```
- `task`: Task ID to check (must be in deps)
- `status`: `"succeeded"` or `"failed"`

### no_worktree (optional)

- **Type**: `boolean`
- **Purpose**: Disable worktree for this task even if enabled globally
- **Default**: `false`

### kind (optional) ⚠️ 常に省略

- **Type**: `"codex" | "claude_code" | "opencode"`
- **Note**: runner設定から自動推論されるため**常に省略する**

### Runner configs (one required)

Exactly one of `codex`, `claude_code`, or `opencode` must be present.

---

## CodexConfig

```json
{
  "prompt": "Implement user authentication",
  "output_last_message": "artifacts/research.md",
  "verify_after": true,
  "sandbox": "workspace-write",
  "json": true
}
```

### prompt (required)

- **Type**: `string`
- **Validation**: Must not be empty

### output_last_message (optional)

- **Type**: `string`
- **Purpose**: Save final message to file
- **Validation**: Only allowed for `mode: "research"`

### verify_after (optional)

- **Type**: `boolean`
- **Purpose**: Auto-append build/lint/test instruction
- **Default**: `true`

### sandbox (optional)

- **Type**: `string`
- **Purpose**: Sandbox mode for research tasks
- **Common values**: `"workspace-write"`, `"danger-full-access"`
- **Note**: Only used for `mode: "research"`

### json (optional) ⚠️ 常に省略

- **Type**: `boolean`
- **Default**: `true`
- **Purpose**: JSONL event output for TUI monitoring
- **Note**: デフォルトがtrueなので**常に省略する**

---

## ClaudeCodeConfig

```json
{
  "prompt": "Implement user authentication",
  "model": "opus",
  "json": true
}
```

### prompt (required)

- **Type**: `string`
- **Validation**: Must not be empty

### model (optional)

- **Type**: `string`
- **Purpose**: Model to use (e.g., `"sonnet"`, `"opus"`)

### json (optional) ⚠️ 常に省略

- **Type**: `boolean`
- **Default**: `true`
- **Note**: デフォルトがtrueなので**常に省略する**

---

## OpencodeConfig

```json
{
  "prompt": "Implement user authentication",
  "model": "gpt-4",
  "json": true
}
```

### prompt (required)

- **Type**: `string`
- **Validation**: Must not be empty

### model (optional)

- **Type**: `string`
- **Purpose**: Model to use

### json (optional) ⚠️ 常に省略

- **Type**: `boolean`
- **Default**: `true`
- **Note**: デフォルトがtrueなので**常に省略する**

---

## Validation Rules

1. **Structural**: Version must be 1, at least one task required
2. **IDs**: Unique, non-empty, alphanumeric with `_` and `-` only
3. **Dependencies**: All deps must exist, no cycles, no self-deps
4. **Runners**: Exactly one of `codex`, `claude_code`, or `opencode` required
5. **Conditions**: Referenced task must be in deps
6. **Groups**: No task in multiple groups, all task IDs must exist
7. **output_files**: Relative paths only, no `..` or absolute paths

---

## Common Patterns

### Sequential workflow

```json
{
  "tasks": [
    {"id": "A", "deps": [], ...},
    {"id": "B", "deps": ["A"], ...},
    {"id": "C", "deps": ["B"], ...}
  ]
}
```

### Parallel with fan-in

```json
{
  "tasks": [
    {"id": "A", "deps": [], ...},
    {"id": "B", "deps": [], ...},
    {"id": "C", "deps": ["A", "B"], ...}
  ]
}
```

### Task groups

```json
{
  "groups": {
    "backend": ["api-research", "api-impl"],
    "frontend": ["ui-research", "ui-impl"]
  },
  "tasks": [...]
}
```

### Conditional execution

```json
{
  "tasks": [
    {"id": "build", ...},
    {
      "id": "deploy",
      "deps": ["build"],
      "condition": { "task": "build", "status": "succeeded" },
      ...
    }
  ]
}
```
