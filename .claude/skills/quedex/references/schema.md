# Plan JSON Schema Reference

## Overview

Plan files define DAG-based task execution for quedex. This reference covers all fields, validation rules, and constraints.

## Root Structure

```json
{
  "version": 1,
  "run": { /* RunConfig */ },
  "tasks": [ /* Task[] */ ]
}
```

### version (required)

- **Type**: `number`
- **Value**: Must be `1`
- **Validation**: Only version 1 is supported

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
  "default_timeout_sec": 3600
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
- **Format**: Key-value pairs

### max_concurrency (optional)

- **Type**: `number`
- **Purpose**: Maximum number of tasks running simultaneously
- **Default**: Unlimited

### fail_fast (optional)

- **Type**: `boolean`
- **Purpose**: Stop execution immediately when any task fails
- **Default**: `false`

### default_timeout_sec (optional)

- **Type**: `number`
- **Purpose**: Default timeout for tasks in seconds
- **Default**: No timeout

---

## Task

```json
{
  "id": "task-a",
  "title": "Research existing implementation",
  "mode": "research",
  "deps": ["task-b"],
  "locks": ["workspace"],
  "timeout_sec": 1800,
  "kind": "codex",
  "codex": { /* CodexConfig */ },
  "shell": { /* ShellConfig */ }
}
```

### id (required)

- **Type**: `string`
- **Validation**:
  - Must not be empty
  - Must be unique across all tasks
  - Cannot depend on itself

### title (optional)

- **Type**: `string`
- **Purpose**: Human-readable task description

### mode (required)

- **Type**: `"research" | "implement" | "verify"`
- **Purpose**: Determines execution context

**Mode behaviors:**
- `research`: Sandboxed exploration, saves output to file
- `implement`: Full write access, automated code changes
- `verify`: Full access for testing/validation

### deps (optional)

- **Type**: `string[]`
- **Purpose**: Task IDs that must complete before this task starts
- **Validation**:
  - All referenced task IDs must exist
  - No circular dependencies allowed
  - DAG structure enforced

### locks (optional)

- **Type**: `string[]`
- **Purpose**: Exclusive resource names (prevents parallel execution of tasks with same lock)
- **Example**: `["workspace", "db-migrate"]`

### timeout_sec (optional)

- **Type**: `number`
- **Purpose**: Task-specific timeout in seconds
- **Overrides**: `run.default_timeout_sec`

### kind (optional)

- **Type**: `"codex" | "shell"`
- **Validation**:
  - If `"codex"`, `codex` config must be present
  - If `"shell"`, `shell` config must be present
- **Note**: Inferred from which config is present if omitted

### codex (conditional)

CodexConfig object. Required if `shell` is not present.

### shell (conditional)

ShellConfig object. Required if `codex` is not present.

**Constraint**: Cannot specify both `codex` and `shell`.

---

## CodexConfig

```json
{
  "prompt": "Implement user authentication",
  "output_last_message": "artifacts/research.md",
  "verify_after": true,
  "sandbox": "workspace-write",
  "ask_for_approval": "never",
  "json": true
}
```

### prompt (required)

- **Type**: `string`
- **Validation**: Must not be empty (after trimming)
- **Purpose**: Instruction for Codex CLI

**Auto-appended for implement mode with verify_after:**
```
実装後 build→lint→test を実行し、エラーがあれば修正して
```

### output_last_message (conditional)

- **Type**: `string` (file path)
- **Purpose**: Save Codex's final message to file
- **Validation**: Only allowed for `mode: "research"`

### verify_after (optional)

- **Type**: `boolean`
- **Purpose**: For `mode: "implement"`, auto-append verification instruction to prompt
- **Default**: `false`

### sandbox (optional)

- **Type**: `string`
- **Purpose**: Sandbox mode for research tasks
- **Common values**: `"workspace-write"`, `"danger-full-access"`
- **Behavior**: Only used for `mode: "research"`. For `implement`/`verify`, `--dangerously-bypass-approvals-and-sandbox` is always used.

### ask_for_approval (optional)

- **Type**: `string`
- **Purpose**: Approval strategy
- **Note**: Currently unused in quedex implementation

### json (optional)

- **Type**: `boolean`
- **Default**: `true`
- **Purpose**: Pass `--json` to Codex CLI for JSONL event output (recommended for TUI monitoring)

---

## ShellConfig

```json
{
  "command": "npm test"
}
```

### command (required)

- **Type**: `string`
- **Purpose**: Shell command to execute
- **Execution**: Runs in `run.cwd` with `run.env`

---

## Validation Rules

quedex validates plans before execution:

1. **Structural validation:**
   - Version must be 1
   - At least one task required
   - All task IDs must be unique and non-empty

2. **Dependency validation:**
   - All `deps` must reference existing task IDs
   - No circular dependencies
   - No self-dependencies

3. **Config validation:**
   - Each task must have exactly one of `codex` or `shell`
   - If `kind` is specified, matching config must exist
   - `codex.prompt` must not be empty
   - `output_last_message` only for research mode

4. **DAG validation:**
   - Dependency graph must be acyclic
   - Enforced using petgraph library

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

### Parallel execution

```json
{
  "tasks": [
    {"id": "A", "deps": [], ...},
    {"id": "B", "deps": [], ...},
    {"id": "C", "deps": ["A", "B"], ...}
  ]
}
```

### Exclusive resource access

```json
{
  "tasks": [
    {"id": "migrate-1", "locks": ["db-migrate"], ...},
    {"id": "migrate-2", "locks": ["db-migrate"], ...}
  ]
}
```
Tasks with same lock never run simultaneously.
