# Per-Mode Concurrency 実装調査

仕様書: `specs/proposal-per-mode-concurrency.md`

---

## 1. 現在のコード構造

### `TaskMode` enum (`src/plan.rs:10-17`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Research,
    #[default]
    Implement,
    Verify,
}
```

**注**: `Eq` は既に存在するため、`Hash` derive のみ追加すれば `HashMap<TaskMode, _>` のキーとして使用可能。

---

### `RunConfig` struct (`src/plan.rs:48-84`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct RunConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub worktree: Option<WorktreeRunConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    // ... deprecated フィールド（reject用）...
    #[serde(default)]
    pub notifications: Option<NotificationConfig>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}
```

新規追加対象: `max_concurrency_by_mode: Option<HashMap<String, usize>>` (YAML互換のため `String` キー)

---

### `SchedulerOptions` struct (`src/scheduler.rs:43-47`)

```rust
#[derive(Debug, Clone, Copy)]
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
}
```

**注意**: 現在 `Clone, Copy` を derive しているが、`HashMap` を追加すると `Copy` は外す必要がある。

---

### `Scheduler::run()` Semaphore 管理ロジック (`src/scheduler.rs:157-287`)

**初期化部分**:
```rust
pub async fn run(self, env_vars: &HashMap<String, String>) -> ScheduleReport {
    let max_concurrency = self.options.max_concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(max_concurrency));
    let lock_table = Arc::new(Mutex::new(init_lock_table(&self.tasks)));
    let (tx, mut rx) = mpsc::unbounded_channel();
```

**タスクディスパッチのセマフォ取得部分**:
```rust
while !ready_queue.is_empty() && semaphore.available_permits() > 0 {
    // ... ready_queue からタスク取得 ...

    if !try_acquire_locks(&lock_table, &task_id, &task.locks) {
        ready_queue.push_back(task_id);
        rotations += 1;
        continue;
    }

    let permit = match semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            release_locks(&lock_table, &task_id, &task.locks);
            ready_queue.push_front(task_id);
            break;
        }
    };
    // ... tokio::spawn でタスク起動 ...
```

**permit の解放部分**:
```rust
tokio::spawn(async move {
    let result = future.await;
    release_locks(&lock_table, &task_id_clone, &locks);
    drop(permit);  // Semaphore permit の自動解放
    let _ = tx.send(SchedulerEvent::TaskFinished { ... });
});
```

**重要**: ループの継続条件 `semaphore.available_permits() > 0` もモード別セマフォに対応させる必要がある。

---

### `handle_run` 内の `SchedulerOptions` 構築 (`src/main.rs:1014-1078`)

**concurrency 決定ロジック**:
```rust
let max_concurrency = plan
    .run
    .max_concurrency
    .or(effective.max_concurrency)
    .unwrap_or(1);
let fail_fast = plan.run.fail_fast.unwrap_or(effective.fail_fast);
```

**SchedulerOptions 構築箇所（2箇所）**:
```rust
// Scheduler::new_with_initial_state 用
Scheduler::new_with_initial_state(
    task_specs,
    SchedulerOptions {
        max_concurrency,
        fail_fast,
    },
    runner,
    initial_states,
)

// Scheduler::new 用
Scheduler::new(
    task_specs,
    SchedulerOptions {
        max_concurrency,
        fail_fast,
    },
    runner,
)
```

---

### `Plan::validate()` のバリデーションパターン (`src/plan.rs:608-892`)

既存の検証パターン例（`max_concurrency` に対する検証が現在は**存在しない**）:

```rust
// エラー追加パターン
errors.push(format!("tasks[{}].id: {}", i, reason));

// 警告追加パターン
warnings.push(format!("run.env: ..."));

// 最終チェック
if !errors.is_empty() {
    return Err(ValidationError { errors, warnings });
}
Ok(warnings)
```

`validate()` の戻り値: `Result<Vec<String>, ValidationError>`（成功時はwarningsのVec）

---

## 2. 変更方針

### 2-1. `TaskMode` に `Hash` derive を追加

```rust
// before
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]

// after
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
```

---

### 2-2. `RunConfig` に `max_concurrency_by_mode` を追加

YAML での設定例:
```yaml
run:
  max_concurrency: 6
  max_concurrency_by_mode:
    research: 4
    implement: 1
    verify: 2
```

仕様書では `HashMap<TaskMode, usize>` を提案しているが、`serde` での JSON/YAML マップキーとして `TaskMode` (enum) を使う場合、デシリアライゼーションが機能するか確認が必要。`serde_yaml` は enum キーをサポートしているが、安全策として `HashMap<String, usize>` で受け取り、`main.rs` で変換する方が堅実。

```rust
// RunConfig への追加
#[serde(default)]
pub max_concurrency_by_mode: Option<HashMap<String, usize>>,
```

---

### 2-3. `SchedulerOptions` に `mode_concurrency` を追加

`Copy` traitは `HashMap` を持てないため削除する:

```rust
// before
#[derive(Debug, Clone, Copy)]
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
}

// after
#[derive(Debug, Clone)]
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub mode_concurrency: HashMap<TaskMode, usize>,
}
```

`SchedulerOptions` を `Copy` から参照に変更することで、呼び出し側への影響を確認する必要がある（`self.options` が `Copy` であることに依存したコードが存在する可能性）。

---

### 2-4. `Scheduler::run()` でモード別 Semaphore を作成・管理

**初期化**:
```rust
let semaphore = Arc::new(Semaphore::new(max_concurrency));
let mode_semaphores: HashMap<TaskMode, Arc<Semaphore>> = self.options
    .mode_concurrency
    .iter()
    .map(|(mode, limit)| (*mode, Arc::new(Semaphore::new((*limit).max(1)))))
    .collect();
let mode_semaphores = Arc::new(mode_semaphores);
```

**ループ継続条件の変更**:
現在の `semaphore.available_permits() > 0` にモード別チェックを加える。ただし、タスクのモードがわからない段階ではチェックできないため、ループ条件としては使わず、permit取得の失敗でブレークする既存パターンを踏襲する。

**ディスパッチ時（取得順序）**:
1. `try_acquire_locks`（既存）
2. モード別 `try_acquire_owned`（新規）
3. グローバル `try_acquire_owned`（既存）

```rust
// 1. ロック取得
if !try_acquire_locks(&lock_table, &task_id, &task.locks) {
    ready_queue.push_back(task_id);
    rotations += 1;
    continue;
}

// 2. モード別 Semaphore 取得（設定がある場合のみ）
let mode_permit = if let Some(mode_sem) = mode_semaphores.get(&task.mode) {
    match mode_sem.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            release_locks(&lock_table, &task_id, &task.locks);
            ready_queue.push_back(task_id);
            rotations += 1;
            continue;  // 他のモードのタスクを試せるよう continue
        }
    }
} else {
    None
};

// 3. グローバル Semaphore 取得
let permit = match semaphore.clone().try_acquire_owned() {
    Ok(permit) => permit,
    Err(_) => {
        drop(mode_permit);  // 取得済みを解放
        release_locks(&lock_table, &task_id, &task.locks);
        ready_queue.push_front(task_id);
        break;
    }
};
```

**タスク完了時（spawn 内）**:
```rust
tokio::spawn(async move {
    let result = future.await;
    release_locks(&lock_table, &task_id_clone, &locks);
    drop(mode_permit);   // モード別 permit 解放
    drop(permit);        // グローバル permit 解放
    let _ = tx.send(SchedulerEvent::TaskFinished { ... });
});
```

---

### 2-5. `main.rs` で `max_concurrency_by_mode` を parse して `SchedulerOptions` に渡す

```rust
// max_concurrency_by_mode の parse
let mode_concurrency: HashMap<TaskMode, usize> = plan
    .run
    .max_concurrency_by_mode
    .unwrap_or_default()
    .into_iter()
    .filter_map(|(k, v)| {
        // "research" | "implement" | "verify" を TaskMode に変換
        let mode = match k.as_str() {
            "research" => TaskMode::Research,
            "implement" => TaskMode::Implement,
            "verify" => TaskMode::Verify,
            _ => return None,  // 未知のキーは無視（validate で捕捉済み）
        };
        Some((mode, v))
    })
    .collect();

// SchedulerOptions 構築
SchedulerOptions {
    max_concurrency,
    fail_fast,
    mode_concurrency,
}
```

---

### 2-6. `Plan::validate()` にバリデーションを追加

```rust
// max_concurrency_by_mode の検証
if let Some(ref mode_map) = self.run.max_concurrency_by_mode {
    let valid_modes = ["research", "implement", "verify"];
    for (key, &val) in mode_map.iter() {
        // 未知のモードキー
        if !valid_modes.contains(&key.as_str()) {
            errors.push(format!(
                "run.max_concurrency_by_mode: unknown mode key \"{}\" (valid: research, implement, verify)",
                key
            ));
        }
        // 値が 0 はエラー
        if val == 0 {
            errors.push(format!(
                "run.max_concurrency_by_mode.{}: must be >= 1 (0 would make all {} tasks permanently blocked)",
                key, key
            ));
        }
        // モード別上限がグローバル上限を超える場合は警告
        if let Some(global) = self.run.max_concurrency {
            if val > global {
                warnings.push(format!(
                    "run.max_concurrency_by_mode.{}: {} > max_concurrency ({}) — the mode limit will never be the effective constraint",
                    key, val, global
                ));
            }
        }
    }
}
```

---

## 3. 注意点

### 既存テストへの影響

- `SchedulerOptions` から `Copy` を外すことで、テスト内で `options` を複数回使用している箇所が影響を受ける可能性がある（`.clone()` を追加すれば解決）
- `SchedulerOptions { max_concurrency, fail_fast }` の struct literal が全てコンパイルエラーになる → `mode_concurrency: HashMap::new()` または `Default::default()` を追加するか、`SchedulerOptions::new(max_concurrency, fail_fast)` コンストラクタを用意する
- 既存テストで `SchedulerOptions` を直接初期化している箇所を全て更新する必要がある

### `serde` での enum キーの取り扱い

`serde_yaml` は `#[serde(rename_all = "snake_case")]` が付いた enum を map キーとして正しくデシリアライズできる（`serde` 1.0 + `serde_yaml` 0.9+ の範囲）。ただし、JSONSchemaの生成（`schemars`）でも enum キーのマップを正しく表現できるか確認が必要。

安全策として `RunConfig` では `HashMap<String, usize>` で定義し、`main.rs` で `HashMap<TaskMode, usize>` に変換するアプローチが堅実。

### `JsonSchema` derive

`RunConfig` が `JsonSchema` を derive しているため、`HashMap<String, usize>` の場合はそのまま機能するが、`HashMap<TaskMode, usize>` にする場合は `schemars` が enum キーをサポートしているか確認が必要（`schemars` 0.8 では制限あり）。

### モード別セマフォの `continue` vs `break` の選択

- グローバルセマフォが枯渇した場合: `break`（他のタスクも同じ状況なので待機）
- モード別セマフォが枯渇した場合: `continue`（他のモードのタスクは実行可能かもしれないため）
- ただし `continue` を使うと `rotations` の無限ループ検出が重要になる（既存の rotations カウンタを活用）

### `available_permits()` によるループ継続条件

現在の `while !ready_queue.is_empty() && semaphore.available_permits() > 0` という条件は、モード別セマフォの状態を考慮していない。モード別セマフォが全て枯渇している場合でもループが継続してしまうが、内部の `continue`/`break` ロジックで適切にハンドリングされるため、ループ条件の変更は最小限でよい。

---

## 4. 変更ファイルサマリー

| ファイル | 変更内容 | 影響 |
|---------|---------|------|
| `src/plan.rs` | `TaskMode` に `Hash` derive 追加 | 既存テストへの影響なし |
| `src/plan.rs` | `RunConfig` に `max_concurrency_by_mode: Option<HashMap<String, usize>>` 追加 | デシリアライズ後方互換あり（`#[serde(default)]`） |
| `src/plan.rs` | `Plan::validate()` に mode_map バリデーション追加 | 新規バリデーション |
| `src/scheduler.rs` | `SchedulerOptions` から `Copy` を外し `mode_concurrency: HashMap<TaskMode, usize>` 追加 | **既存テスト・呼び出し箇所への影響大** |
| `src/scheduler.rs` | `Scheduler::run()` にモード別 Semaphore 作成・取得ロジック追加 | 既存動作への影響なし（空 HashMap の場合） |
| `src/main.rs` | `mode_concurrency` parse ロジック追加、`SchedulerOptions` 構築更新 | 2箇所の SchedulerOptions 構築を更新 |
