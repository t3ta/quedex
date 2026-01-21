# Issue #24: タスクグループ/階層化 - CLI拡張設計書

## 1. 現在のCLI構造分析

### 1.1 Retryコマンド (src/cli.rs:82-87)

```rust
Retry {
    run_id: String,
    task_id: String,
    #[arg(long, help = "Reload plan from the run directory before retrying")]
    reload_plan: bool,
}
```

**特徴**:
- 必須引数: `run_id`, `task_id`
- オプション: `--reload-plan` フラグ
- 単一タスク指定のみ対応

### 1.2 Cancelコマンド (src/cli.rs:89-92)

```rust
Cancel {
    run_id: String,
    task_id: Option<String>,
}
```

**特徴**:
- 必須引数: `run_id`
- オプション: `task_id` (未指定時はrun全体をキャンセル)

### 1.3 Statusコマンド (src/cli.rs:63-67)

```rust
Status {
    run_id: Option<String>,
    #[arg(long)]
    json: bool,
}
```

**特徴**:
- オプション: `run_id` (未指定時は全runを表示)
- オプション: `--json` フラグ
- フィルタリング機能なし

---

## 2. --group オプション追加設計

### 2.1 CLI定義の変更

```rust
// src/cli.rs

Retry {
    run_id: String,
    /// Task ID to retry (conflicts with --group)
    #[arg(conflicts_with = "group")]
    task_id: Option<String>,
    /// Group name to retry all failed/canceled/skipped tasks
    #[arg(long, conflicts_with = "task_id")]
    group: Option<String>,
    #[arg(long, help = "Reload plan from the run directory before retrying")]
    reload_plan: bool,
}

Cancel {
    run_id: String,
    /// Task ID to cancel (conflicts with --group)
    #[arg(conflicts_with = "group")]
    task_id: Option<String>,
    /// Group name to cancel all running/pending tasks
    #[arg(long, conflicts_with = "task_id")]
    group: Option<String>,
}

Status {
    run_id: Option<String>,
    /// Filter by group name
    #[arg(long)]
    group: Option<String>,
    #[arg(long)]
    json: bool,
}
```

### 2.2 使用例

```bash
# グループ内の全失敗タスクをリトライ
quedex retry <run_id> --group backend

# グループ内の全タスクをキャンセル
quedex cancel <run_id> --group frontend

# グループでフィルタリングしてステータス表示
quedex status <run_id> --group database

# 従来の単一タスク操作も引き続き可能
quedex retry <run_id> task-1
quedex cancel <run_id> task-2
```

### 2.3 引数の競合制御

`clap`の`conflicts_with`属性を使用:
- `task_id`と`--group`は同時指定不可
- `--group`のみ指定時は、グループ内全タスクを対象
- `task_id`のみ指定時は、従来の単一タスク操作

---

## 3. ハンドラ関数の拡張設計

### 3.1 handle_retry() の拡張 (src/main.rs:1163-1320)

```rust
async fn handle_retry(
    _global: &GlobalOptions,
    effective: &EffectiveOptions,
    run_id: &str,
    task_id: Option<&str>,
    group: Option<&str>,
    reload_plan: bool,
) -> Result<i32> {
    // プラン読み込み
    let plan = load_plan_snapshot(effective, run_id)?;

    // グループまたはタスクIDからタスクリストを解決
    let target_tasks = resolve_target_tasks(&plan, task_id, group)?;

    // 各タスクの検証と状態リセット
    let mut state = read_state(effective, run_id)?;
    let mut tasks_to_retry = Vec::new();

    for task_id in &target_tasks {
        let task = plan.tasks.iter()
            .find(|t| &t.id == task_id)
            .ok_or_else(|| anyhow!("task {} not found", task_id))?;

        let task_state = state.tasks.get_mut(task_id)
            .ok_or_else(|| anyhow!("task {} not found in state", task_id))?;

        // Failed/Canceled/Skipped のみリトライ可能
        match task_state.status {
            TaskStatus::Failed | TaskStatus::Canceled | TaskStatus::Skipped => {
                // 依存タスクがすべてSucceededか確認
                validate_dependencies(&plan, &state, task)?;

                // 状態リセット
                task_state.status = TaskStatus::Pending;
                task_state.exit_code = None;
                task_state.stderr_tail = None;
                task_state.started_at = None;
                task_state.completed_at = None;
                task_state.pid = None;

                tasks_to_retry.push(task.clone());
            }
            _ => {
                // グループ指定時はスキップ、単一指定時はエラー
                if group.is_some() {
                    continue;
                }
                return Err(anyhow!("task {} is not in a retryable state", task_id));
            }
        }
    }

    if tasks_to_retry.is_empty() {
        if group.is_some() {
            println!("No retryable tasks found in group '{}'", group.unwrap());
            return Ok(0);
        }
        return Err(anyhow!("no tasks to retry"));
    }

    // 状態保存とスケジューラ実行
    write_state(effective, run_id, &state)?;

    println!("Retrying {} task(s)...", tasks_to_retry.len());
    // ... スケジューラ実行
}
```

### 3.2 handle_cancel() の拡張 (src/main.rs:1322-1357)

```rust
fn handle_cancel(
    effective: &EffectiveOptions,
    run_id: &str,
    task_id: Option<&str>,
    group: Option<&str>,
) -> Result<i32> {
    let plan = load_plan_snapshot(effective, run_id)?;
    let state = read_state(effective, run_id)?;

    // グループまたはタスクIDからタスクリストを解決
    let target_tasks = resolve_target_tasks(&plan, task_id, group)?;

    let mut cancelled_count = 0;

    for task_id in &target_tasks {
        if let Some(task_state) = state.tasks.get(task_id) {
            // Running または Pending のみキャンセル可能
            match task_state.status {
                TaskStatus::Running => {
                    if let Some(pid) = task_state.pid {
                        terminate_pid(pid)?;
                        cancelled_count += 1;
                    }
                }
                TaskStatus::Pending => {
                    // Pending状態のタスクはCanceledに更新
                    // (実際には実行前なのでPIDはない)
                    cancelled_count += 1;
                }
                _ => {
                    // グループ指定時はスキップ
                    if group.is_some() {
                        continue;
                    }
                    // 単一指定時はエラー
                    return Err(anyhow!("task {} is not running or pending", task_id));
                }
            }
        }
    }

    if cancelled_count == 0 && group.is_some() {
        println!("No cancellable tasks found in group '{}'", group.unwrap());
    } else {
        println!("Cancelled {} task(s)", cancelled_count);
    }

    Ok(0)
}
```

### 3.3 handle_status() の拡張 (src/main.rs:1087-1110)

```rust
fn handle_status(
    effective: &EffectiveOptions,
    run_id: Option<String>,
    group: Option<&str>,
    json: bool,
) -> Result<i32> {
    match run_id {
        Some(run_id) => {
            let plan = load_plan_snapshot(effective, &run_id)?;
            let state = read_state(effective, &run_id)?;

            // グループ指定時はフィルタリング
            let filtered_state = if let Some(group_name) = group {
                let group_tasks = plan.get_group_tasks(group_name)?;
                filter_state_by_tasks(&state, &group_tasks)
            } else {
                state
            };

            if json {
                print_state_json(&filtered_state)?;
            } else {
                print_state(&filtered_state)?;
            }
        }
        None => {
            // run_id未指定時はグループフィルタは無視
            if group.is_some() {
                eprintln!("Warning: --group requires --run-id, ignoring");
            }
            print_states_table(effective)?;
        }
    }

    Ok(0)
}
```

---

## 4. グループ名解決ヘルパー関数の設計

### 4.1 Plan構造体への追加メソッド (src/plan.rs)

```rust
impl Plan {
    /// グループ名からタスクIDリストを解決
    /// groups定義とtask.groupフィールドの両方をマージ
    pub fn resolve_groups(&self) -> HashMap<String, Vec<String>> {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();

        // Plan.groups からの定義を追加
        if let Some(groups) = &self.groups {
            for (name, task_ids) in groups {
                result.insert(name.clone(), task_ids.clone());
            }
        }

        // 各タスクの group フィールドからも追加
        for task in &self.tasks {
            if let Some(group_name) = &task.group {
                result.entry(group_name.clone())
                    .or_default()
                    .push(task.id.clone());
            }
        }

        // 重複を除去
        for tasks in result.values_mut() {
            tasks.sort();
            tasks.dedup();
        }

        result
    }

    /// 指定グループのタスクIDリストを取得
    pub fn get_group_tasks(&self, group_name: &str) -> Result<Vec<String>> {
        let groups = self.resolve_groups();
        groups.get(group_name)
            .cloned()
            .ok_or_else(|| anyhow!("group '{}' not found in plan", group_name))
    }

    /// タスクIDが所属するグループ名を取得
    pub fn get_task_group(&self, task_id: &str) -> Option<String> {
        // まずtask.groupフィールドを確認
        for task in &self.tasks {
            if task.id == task_id {
                if let Some(group) = &task.group {
                    return Some(group.clone());
                }
            }
        }

        // Plan.groupsからも検索
        if let Some(groups) = &self.groups {
            for (name, task_ids) in groups {
                if task_ids.contains(&task_id.to_string()) {
                    return Some(name.clone());
                }
            }
        }

        None
    }

    /// 全グループ名のリストを取得
    pub fn list_groups(&self) -> Vec<String> {
        let groups = self.resolve_groups();
        let mut names: Vec<_> = groups.keys().cloned().collect();
        names.sort();
        names
    }
}
```

### 4.2 共通ヘルパー関数 (src/main.rs)

```rust
/// task_idまたはgroupからターゲットタスクリストを解決
fn resolve_target_tasks(
    plan: &Plan,
    task_id: Option<&str>,
    group: Option<&str>,
) -> Result<Vec<String>> {
    match (task_id, group) {
        (Some(id), None) => {
            // 単一タスク指定
            if !plan.tasks.iter().any(|t| t.id == id) {
                return Err(anyhow!("task '{}' not found in plan", id));
            }
            Ok(vec![id.to_string()])
        }
        (None, Some(group_name)) => {
            // グループ指定
            plan.get_group_tasks(group_name)
        }
        (None, None) => {
            // 両方未指定（Cancelコマンドで全タスク対象の場合）
            Ok(plan.tasks.iter().map(|t| t.id.clone()).collect())
        }
        (Some(_), Some(_)) => {
            // CLIレベルで排他されているはずだが念のため
            Err(anyhow!("cannot specify both task_id and --group"))
        }
    }
}

/// StateをタスクIDリストでフィルタリング
fn filter_state_by_tasks(state: &State, task_ids: &[String]) -> State {
    let task_set: HashSet<_> = task_ids.iter().collect();
    State {
        run_id: state.run_id.clone(),
        run_name: state.run_name.clone(),
        status: state.status.clone(),
        tasks: state.tasks.iter()
            .filter(|(id, _)| task_set.contains(id))
            .map(|(id, ts)| (id.clone(), ts.clone()))
            .collect(),
        started_at: state.started_at,
        completed_at: state.completed_at,
    }
}
```

---

## 5. エラーハンドリング設計

### 5.1 グループ関連エラー

| エラー条件 | エラーメッセージ | 対応 |
|-----------|----------------|------|
| グループが存在しない | `group 'xxx' not found in plan` | 即座にエラー終了 |
| グループにタスクがない | `group 'xxx' has no tasks` | 即座にエラー終了 |
| グループ内にリトライ可能タスクがない | `No retryable tasks found in group 'xxx'` | 警告表示して正常終了 |
| グループ内にキャンセル可能タスクがない | `No cancellable tasks found in group 'xxx'` | 警告表示して正常終了 |

### 5.2 エラーコード

```rust
// 終了コード
const EXIT_SUCCESS: i32 = 0;
const EXIT_TASK_NOT_FOUND: i32 = 1;
const EXIT_GROUP_NOT_FOUND: i32 = 2;
const EXIT_INVALID_STATE: i32 = 3;
```

### 5.3 実装例

```rust
fn validate_group_exists(plan: &Plan, group_name: &str) -> Result<()> {
    let groups = plan.resolve_groups();
    if !groups.contains_key(group_name) {
        return Err(anyhow!("group '{}' not found in plan. Available groups: {:?}",
            group_name,
            groups.keys().collect::<Vec<_>>()));
    }
    Ok(())
}
```

---

## 6. バリデーション拡張 (src/plan.rs)

### 6.1 Plan::validate() への追加

```rust
impl Plan {
    pub fn validate(&self) -> Result<()> {
        // 既存のバリデーション...

        // グループバリデーション追加
        self.validate_groups()?;

        Ok(())
    }

    fn validate_groups(&self) -> Result<()> {
        let task_ids: HashSet<_> = self.tasks.iter().map(|t| t.id.as_str()).collect();

        // Plan.groups の検証
        if let Some(groups) = &self.groups {
            for (group_name, group_task_ids) in groups {
                // 空のグループをチェック
                if group_task_ids.is_empty() {
                    bail!("group '{}' has no tasks", group_name);
                }

                // 存在しないタスクIDをチェック
                for task_id in group_task_ids {
                    if !task_ids.contains(task_id.as_str()) {
                        bail!("group '{}' references non-existent task '{}'",
                            group_name, task_id);
                    }
                }

                // 重複タスクIDをチェック
                let unique: HashSet<_> = group_task_ids.iter().collect();
                if unique.len() != group_task_ids.len() {
                    bail!("group '{}' has duplicate task IDs", group_name);
                }
            }
        }

        // Task.group の検証
        for task in &self.tasks {
            if let Some(group_name) = &task.group {
                // グループ名が空でないことを確認
                if group_name.is_empty() {
                    bail!("task '{}' has empty group name", task.id);
                }
            }
        }

        Ok(())
    }
}
```

---

## 7. 実装優先度

### Phase 1: 必須機能 (MVP)

1. **CLI定義変更** (`src/cli.rs`)
   - `--group` オプション追加
   - `conflicts_with` 属性設定

2. **Plan拡張** (`src/plan.rs`)
   - `groups` フィールド追加
   - `Task.group` フィールド追加
   - `resolve_groups()` メソッド実装
   - `get_group_tasks()` メソッド実装
   - `validate_groups()` 実装

3. **ハンドラ拡張** (`src/main.rs`)
   - `resolve_target_tasks()` ヘルパー実装
   - `handle_retry()` のグループ対応
   - `handle_cancel()` のグループ対応
   - `handle_status()` のグループフィルタ対応

### Phase 2: 拡張機能

4. **グラフ表示対応**
   - Mermaidでsubgraph出力
   - ASCIIグラフでセクション分け

5. **TUI対応**
   - グループヘッダ行表示
   - 折りたたみ機能

---

## 8. テストケース

### 8.1 CLIテスト

```rust
#[test]
fn test_retry_with_group() {
    let cmd = Cli::parse_from(["quedex", "retry", "run-1", "--group", "backend"]);
    // ...
}

#[test]
fn test_retry_conflicts() {
    // task_idと--groupの同時指定でエラー
    let result = Cli::try_parse_from(["quedex", "retry", "run-1", "task-1", "--group", "backend"]);
    assert!(result.is_err());
}
```

### 8.2 グループ解決テスト

```rust
#[test]
fn test_resolve_groups() {
    let plan = Plan {
        groups: Some(hashmap!{
            "backend".to_string() => vec!["api".to_string(), "db".to_string()]
        }),
        tasks: vec![
            Task { id: "api".to_string(), group: Some("backend".to_string()), ..Default::default() },
            Task { id: "db".to_string(), group: None, ..Default::default() },
            Task { id: "ui".to_string(), group: Some("frontend".to_string()), ..Default::default() },
        ],
        ..Default::default()
    };

    let groups = plan.resolve_groups();
    assert_eq!(groups.get("backend"), Some(&vec!["api".to_string(), "db".to_string()]));
    assert_eq!(groups.get("frontend"), Some(&vec!["ui".to_string()]));
}
```

### 8.3 エラーハンドリングテスト

```rust
#[test]
fn test_group_not_found() {
    let plan = Plan::default();
    let result = plan.get_group_tasks("nonexistent");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}
```

---

## 9. 後方互換性

- `groups` フィールド: `#[serde(default)]` で optional
- `Task.group` フィールド: `#[serde(default)]` で optional
- 既存のCLI使用法は変更なし
- グループ未定義の plan.json も引き続き動作

---

## 10. 参考: 既存コードの重要な場所

| ファイル | 行番号 | 内容 |
|---------|--------|------|
| src/cli.rs | 63-67 | Status コマンド定義 |
| src/cli.rs | 82-87 | Retry コマンド定義 |
| src/cli.rs | 89-92 | Cancel コマンド定義 |
| src/main.rs | 1087-1110 | handle_status() |
| src/main.rs | 1163-1320 | handle_retry() |
| src/main.rs | 1322-1357 | handle_cancel() |
| src/plan.rs | 63-73 | Plan 構造体 |
| src/plan.rs | 164-194 | Task 構造体 |
| src/plan.rs | 210-341 | Plan::validate() |
| src/store/mod.rs | 89-110 | State, TaskState 構造体 |
