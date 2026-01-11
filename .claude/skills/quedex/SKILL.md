---
name: quedex
description: DAG-based task execution with Codex CLI integration. Use when users ask to (1) create execution plans for multi-step implementation tasks, (2) execute tasks using quedex, (3) monitor quedex runs with TUI, or (4) manage failed tasks with retry. Triggers include "create a plan", "use quedex", "execute with quedex", "monitor quedex", "retry failed task", or references to DAG/parallel execution workflows.
---

# Quedex

## Overview

Generate and execute DAG-based task plans using Codex CLI. Quedex handles dependency resolution, parallel execution, state persistence, and failure recovery while LLMs focus on planning.

**Core capabilities:**
- DAG-based dependency resolution and scheduling
- Codex CLI integration (non-interactive by default)
- Parallel execution with concurrency control
- Exclusive resource locks (workspace, db-migrate, etc.)
- State and log persistence with recovery from crashes
- Real-time TUI monitoring
- Failed task retry

## Quick Start

### 1. Create a plan

When users request multi-step implementations, create a `plan.json`:

```json
{
  "version": 1,
  "run": {
    "name": "demo",
    "cwd": ".",
    "max_concurrency": 2,
    "fail_fast": true
  },
  "tasks": [
    {
      "id": "research",
      "title": "調査: 既存実装の把握",
      "mode": "research",
      "deps": [],
      "kind": "codex",
      "codex": {
        "prompt": "このリポジトリの構成を調査して要点をまとめて",
        "output_last_message": "artifacts/research.md",
        "sandbox": "workspace-write",
        "json": true
      }
    },
    {
      "id": "implement",
      "title": "実装: 機能追加",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "artifacts/research.md を参考に新機能を実装して",
        "verify_after": true,
        "json": true
      }
    }
  ]
}
```

**For plan schema details**, see [schema.md](references/schema.md).

**For common patterns**, see [examples.md](references/examples.md).

**For templates**, copy from [assets/templates/](assets/templates/).

### 2. Execute

```bash
quedex run plan.json
```

Or background execution:

```bash
quedex start plan.json
# Returns: run_id abc123...

quedex tui abc123  # Monitor with TUI
```

### 3. Monitor and recover

```bash
quedex status [run_id]              # Check status
quedex logs <run_id> <task_id>      # View logs
quedex retry <run_id> <task_id>     # Retry failed task
quedex tui [run_id]                 # Interactive monitoring
```

## Creating Plans

### Workflow decision tree

When users request implementation tasks:

1. **Single simple task?**
   → Don't use quedex, implement directly

2. **Multi-step with dependencies?**
   → Create quedex plan with DAG structure

3. **Need context before implementing?**
   → Use research → implement pattern (see [examples.md](references/examples.md))

4. **Multiple independent areas?**
   → Use parallel research tasks, then synthesize (see [examples.md](references/examples.md))

5. **Sequential phases required?**
   → Chain tasks with deps (see [examples.md](references/examples.md))

### Task modes

**research**: Sandboxed exploration
- Read-only codebase access
- Saves findings to `output_last_message` file
- Use `sandbox: "workspace-write"` to allow artifact creation
- Good for: understanding existing code, gathering context

**implement**: Full write access
- Uses `--dangerously-bypass-approvals-and-sandbox` (non-interactive)
- Set `verify_after: true` to auto-append build/lint/test instruction
- Use `locks: ["workspace"]` to prevent concurrent modifications
- Good for: adding features, refactoring, writing docs

**verify**: Testing and validation
- Uses `--dangerously-bypass-approvals-and-sandbox` (non-interactive)
- Good for: running tests, checking builds, validating fixes

### Using templates

Quick-start with pre-built templates:

```bash
cp assets/templates/minimal.json plan.json         # Single task
cp assets/templates/research-only.json plan.json   # Parallel research
cp assets/templates/full-pipeline.json plan.json   # Complete workflow
```

Edit task prompts, IDs, and deps as needed.

### Schema validation

Quedex validates plans before execution:
- Unique task IDs
- All deps reference existing tasks
- No circular dependencies
- DAG structure enforced
- Required fields present (see [schema.md](references/schema.md))

To test validation without executing:

```bash
quedex graph plan.json
```

## Executing Plans

### Foreground execution

```bash
quedex run plan.json
```

Returns exit code 0 on success, non-zero on failure.

**Options:**
- `--resume`: Continue from previous run (checks Running tasks for process death)
- `--clean-start`: Clear state and start fresh

### Background execution

```bash
quedex start plan.json
```

Returns `run_id` immediately. Use for long-running tasks.

**Monitor progress:**
```bash
quedex status <run_id>
quedex tui <run_id>
```

### Execution behavior

**Dependency resolution**: Tasks wait for all `deps` to complete successfully.

**Parallel execution**: Tasks with satisfied deps run concurrently up to `max_concurrency`.

**Locks**: Tasks with same lock never run simultaneously (even without deps).

**fail_fast**: If `true`, stops scheduling new tasks when any task fails. Running tasks continue.

**Timeout**: Tasks exceeding `timeout_sec` are killed and marked failed.

## Monitoring and Management

### Real-time TUI

```bash
quedex tui [run_id]
```

**Features:**
- Task list with status, duration, deps
- Log viewer (stdout/stderr)
- Overall stats (completed/running/failed/locks)
- Graph view

**Key bindings:**
- `↑↓`: Select task
- `Enter`: Focus logs
- `t`: Toggle stdout/stderr
- `r`: Retry failed task
- `c`: Cancel task
- `C`: Cancel run
- `g`: Show graph
- `q`: Quit

### Checking status

```bash
quedex status              # Latest or running run
quedex status <run_id>     # Specific run
quedex status --json       # JSON output
```

### Viewing logs

```bash
quedex logs <run_id> <task_id>          # View stdout
quedex logs <run_id> <task_id> --stderr # View stderr
quedex logs <run_id> <task_id> -f       # Follow (tail)
```

### Retrying failed tasks

```bash
quedex retry <run_id> <task_id>
```

Re-executes failed or cancelled task. Only allowed if deps are satisfied.

### Cancelling

```bash
quedex cancel <run_id> <task_id>   # Cancel single task
quedex cancel <run_id>              # Cancel entire run
```

### State recovery

If quedex crashes, Running tasks are detected on `--resume`:

```bash
quedex run plan.json --resume
```

Checks Running tasks for process death (using `kill -0` on Unix), updates state, and continues execution.

## Codex Integration Details

### Task execution

For each task with `kind: "codex"`, quedex spawns:

```bash
codex exec "<prompt>" [options]
```

**Mode-specific options:**

**research**:
```bash
--output-last-message <path>
-s <sandbox>         # Optional
--json               # If codex.json: true
```

**implement / verify**:
```bash
--dangerously-bypass-approvals-and-sandbox
--json               # If codex.json: true
```

### verify_after behavior

When `mode: "implement"` and `verify_after: true`, quedex auto-appends:

```
実装後 build→lint→test を実行し、エラーがあれば修正して
```

Use this to ensure implementation tasks validate their changes.

### json output

Set `codex.json: true` (default) for JSONL event streaming. Provides:
- File read events
- Tool execution events
- Thinking process events
- Better TUI progress visualization

Recommended for all tasks unless you need plain text output.

## Common Patterns by Use Case

### Exploring unfamiliar codebase

Use parallel research tasks:

```json
{
  "tasks": [
    {"id": "research-api", "mode": "research", ...},
    {"id": "research-db", "mode": "research", ...},
    {"id": "research-auth", "mode": "research", ...}
  ]
}
```

See [examples.md](references/examples.md) for complete example.

### Implementing new feature

Use research → implement pipeline:

```json
{
  "tasks": [
    {"id": "research", "mode": "research", ...},
    {"id": "implement", "mode": "implement", "deps": ["research"], ...}
  ]
}
```

### Multi-phase rollout

Chain implementation tasks:

```json
{
  "tasks": [
    {"id": "impl-models", ...},
    {"id": "impl-api", "deps": ["impl-models"], ...},
    {"id": "impl-ui", "deps": ["impl-api"], ...},
    {"id": "verify", "deps": ["impl-ui"], ...}
  ]
}
```

### Database migrations

Use locks to prevent concurrent schema changes:

```json
{
  "tasks": [
    {"id": "migrate-1", "locks": ["db-migrate"], ...},
    {"id": "migrate-2", "locks": ["db-migrate"], ...}
  ]
}
```

### Mixing Codex and shell

Combine AI implementation with deterministic commands:

```json
{
  "tasks": [
    {"id": "implement", "kind": "codex", ...},
    {"id": "build", "kind": "shell", "shell": {"command": "npm run build"}, ...},
    {"id": "test", "kind": "shell", "shell": {"command": "npm test"}, ...}
  ]
}
```

## Resources

### references/schema.md

Complete Plan JSON schema reference:
- All fields and types
- Validation rules
- Constraints and requirements
- Common patterns

**Read when:** Creating plans, troubleshooting validation errors, or understanding advanced options.

### references/examples.md

Common quedex patterns and real-world examples:
- Basic patterns (single research, research→implement)
- Advanced patterns (multi-phase, mixed Codex/shell, locks)
- Full dogfooding example (quedex v1 built with quedex)
- Usage tips (when to use modes, lock strategy, concurrency tuning)

**Read when:** Designing workflows, choosing patterns, or learning best practices.

### assets/templates/

Pre-built plan templates:
- `minimal.json`: Single task starter
- `research-only.json`: Parallel research workflow
- `full-pipeline.json`: Complete research→implement→verify pipeline

**Use when:** Quick-starting new plans instead of writing from scratch.

## Tips

### When to use quedex

**Good fits:**
- Multi-step implementations with dependencies
- Parallel research across multiple subsystems
- Complex features requiring staged rollout
- Tasks that need failure recovery and retry
- Long-running operations requiring background execution

**Poor fits:**
- Single simple tasks
- Highly interactive workflows
- Tasks requiring real-time user input

### Optimizing plans

**Maximize parallelism**: Only add deps when truly required. Independent tasks run concurrently.

**Use locks sparingly**: Only for genuine conflicts (workspace modifications, database migrations).

**Tune concurrency**: Start with `max_concurrency: 2`, increase if tasks are I/O bound.

**fail_fast strategy**: Use `fail_fast: true` for pipelines where later tasks depend on earlier success.

### Debugging failures

1. Check logs: `quedex logs <run_id> <task_id> --stderr`
2. Review task status: `quedex status <run_id>`
3. Fix issue (code, prompt, or plan)
4. Retry: `quedex retry <run_id> <task_id>`

Or use TUI for interactive debugging: `quedex tui <run_id>`
