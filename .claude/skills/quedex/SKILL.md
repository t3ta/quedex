---
name: quedex
description: DAG-based task execution with Codex CLI integration. Use when users ask to (1) create execution plans for multi-step implementation tasks, (2) execute tasks using quedex, (3) monitor quedex runs with TUI, or (4) manage failed tasks with retry. Triggers include "create a plan", "use quedex", "execute with quedex", "monitor quedex", "retry failed task", or references to DAG/parallel execution workflows.
---

# Quedex

## Overview

Generate and execute DAG-based task plans using Codex CLI, Claude Code, or Opencode. Quedex handles dependency resolution, parallel execution, state persistence, and failure recovery while LLMs focus on planning.

**Core capabilities:**
- DAG-based dependency resolution and scheduling
- Multiple runners: Codex CLI / Claude Code / Opencode
- Parallel execution with concurrency control
- Exclusive resource locks (workspace, db-migrate, etc.)
- Task groups for batch operations
- Conditional execution (env vars, task results)
- Automatic retry with configurable delays
- Output file capture
- Real-time TUI and Web dashboard monitoring
- State persistence and crash recovery

## Quick Start

### 1. Create a plan

```yaml
version: 1
run:
  name: "demo"
  max_concurrency: 2

groups:
  backend: [research, implement]

tasks:
  - id: research
    title: "調査: 既存実装の把握"
    mode: research
    codex:
      prompt: "このリポジトリの構成を調査して要点をまとめて"
      output_last_message: "artifacts/research.md"

  - id: implement
    title: "実装: 機能追加"
    mode: implement
    deps: [research]
    locks: [workspace]
    claude_code:
      prompt: "artifacts/research.md を参考に新機能を実装して"
      model: sonnet
```

**For plan schema details**, see [schema.md](references/schema.md).
**For common patterns**, see [examples.md](references/examples.md).

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

**research**: Sandboxed exploration
- Use `output_last_message` to save findings
- Use `sandbox: "workspace-write"` for artifact creation

**implement**: Full write access
- Set `verify_after: true` (default) for auto build/lint/test
- Use `locks: ["workspace"]` to prevent conflicts

**verify**: Testing and validation

### Runners

**codex**: Codex CLI
```yaml
codex:
  prompt: "Implement feature X"
  verify_after: true
  json: true
```

**claude_code**: Claude Code
```yaml
claude_code:
  prompt: "Implement feature X"
  model: opus  # or sonnet
```

**opencode**: Opencode
```yaml
opencode:
  prompt: "Implement feature X"
  model: gpt-4
```

## Advanced Features

### Task groups

Group tasks for batch operations:

```yaml
groups:
  backend: [api-research, api-impl]
  frontend: [ui-research, ui-impl]
```

```bash
quedex status <run_id> --group backend
quedex retry <run_id> --group backend
quedex cancel <run_id> --group frontend
```

### Conditional execution

Skip tasks based on conditions:

```yaml
# Environment variable condition
condition:
  env: "CI"
  equals: "true"

# Previous task result condition
condition:
  task: "build"
  status: succeeded
```

### Automatic retry

```yaml
retry_count: 3
retry_delay_sec: 30
```

### Output file capture

```yaml
output_files:
  - "artifacts/report.md"
  - "coverage/lcov.info"
```

View with: `quedex outputs <run_id> --task <task_id>`

### Dynamic timeout

```yaml
timeout_sec: 300       # Fixed
timeout_sec: "auto"    # Average + 2σ from history
timeout_sec: "2x_average"  # 2× average
```

### Template variables

```yaml
variables:
  target_dir: "src/api"
  test_cmd: "npm test"

tasks:
  - id: impl
    codex:
      prompt: "Implement in ${target_dir}, run ${test_cmd}"
```

Environment variables: `${env.CI}`, `${env.NODE_ENV}`

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

### When to use quedex

**Good fits:**
- Multi-step implementations with dependencies
- Parallel research across subsystems
- Complex features with staged rollout
- Tasks needing failure recovery
- Long-running background operations

**Poor fits:**
- Single simple tasks
- Highly interactive workflows

### Optimizing plans

- **Maximize parallelism**: Only add deps when truly required
- **Use locks sparingly**: Only for genuine conflicts
- **Tune concurrency**: Start with `max_concurrency: 2`
- **Use groups**: Organize related tasks for batch operations
