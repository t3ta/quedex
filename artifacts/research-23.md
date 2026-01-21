# Issue #23 「実行統計・メトリクス」実装調査報告書

## 調査概要

Issue #23 で要求される `quedex stats` コマンドの実装に必要な情報を調査しました。

---

## 1. state.json の構造と保存場所

### 保存場所
- **パス**: `{store_root}/runs/{run_id}/state.json`
- **デフォルト store_root**: `~/.quedex` または `--store` オプション指定値

### 関連ファイル
| ファイル | 行番号 | 役割 |
|---------|--------|------|
| `src/store/fs.rs` | 41-42 | `state_path()` メソッドでパス構築 |
| `src/store/mod.rs` | 74-93 | State/TaskState の型定義 |

### State スキーマ

```rust
// src/store/mod.rs:74-93
pub struct State {
    pub run_id: String,                       // 実行ID (UUID)
    pub run_name: String,                     // 実行名
    pub status: RunStatus,                    // Running / Completed / Failed / Canceled
    pub tasks: HashMap<String, TaskState>,    // タスク状態マップ
    pub started_at: DateTime<Utc>,            // 開始時刻 (UTC)
    pub completed_at: Option<DateTime<Utc>>,  // 終了時刻 (UTC)
}

pub struct TaskState {
    pub status: TaskStatus,                   // Pending/Ready/Running/Succeeded/Failed/Canceled/Skipped
    pub exit_code: Option<i32>,               // 終了コード
    pub stderr_tail: Option<String>,          // 最後のエラー出力
    pub started_at: Option<DateTime<Utc>>,    // タスク開始時刻
    pub completed_at: Option<DateTime<Utc>>,  // タスク終了時刻
    pub pid: Option<u32>,                     // プロセスID
}

pub enum RunStatus {
    Running, Completed, Failed, Canceled
}

pub enum TaskStatus {
    Pending, Ready, Running, Succeeded, Failed, Canceled, Skipped
}
```

---

## 2. 実行履歴の保存形式

### ディレクトリ構造
```
~/.quedex/
└── runs/
    └── {run_id}/
        ├── state.json          # 実行状態 (State)
        ├── events.jsonl        # イベントログ (JSON Lines)
        └── tasks/
            └── {task_id}/
                ├── stdout.log  # 標準出力
                └── stderr.log  # 標準エラー
```

### イベントログ形式 (events.jsonl)
```rust
// src/store/mod.rs:29-53
#[serde(tag = "type")]
pub enum Event {
    RunStarted { run_id, timestamp },
    TaskStarted { task_id, pid, timestamp },
    TaskExited { task_id, exit_code, timestamp },
    TaskCanceled { task_id, timestamp },
}
```

### 一覧取得方法
```rust
// src/store/fs.rs:96-121
pub fn list_states(store_path: &Path) -> Result<Vec<State>>
```
- `runs/` ディレクトリを走査
- 各 `state.json` を読み込み
- エラーは警告を出してスキップ

---

## 3. CLIコマンドの追加方法

### 既存コマンド定義
**ファイル**: `src/cli.rs`

```rust
#[derive(Debug, Subcommand)]
pub enum Commands {
    Run { ... },
    Status { ... },
    Retry { ... },
    Logs { ... },
    Cancel { ... },
    Monitor { ... },
    Clean { ... },
    History { ... },  // ← 参考実装
}
```

### 新コマンド追加手順

#### Step 1: cli.rs にコマンド定義を追加
```rust
// src/cli.rs の Commands enum に追加
/// Show execution statistics and metrics
Stats {
    /// Time period to analyze (e.g., "7d", "24h", "1w")
    #[arg(long, value_name = "DURATION")]
    since: Option<String>,

    /// Output in JSON format
    #[arg(long)]
    json: bool,
},
```

#### Step 2: main.rs に dispatch を追加
```rust
// src/main.rs の run() 関数内
Commands::Stats { since, json } => handle_stats(&effective, since, json),
```

#### Step 3: ハンドラ関数を実装
```rust
fn handle_stats(
    cfg: &EffectiveConfig,
    since: Option<String>,
    json_output: bool,
) -> Result<()> {
    // 実装
}
```

### 参考になる既存ハンドラ
| ハンドラ | 行番号 | 参考になる点 |
|---------|--------|------------|
| `handle_history()` | 268-310 | `list_states()` の使い方、JSON出力の分岐 |
| `handle_status()` | 703-761 | 単一runの詳細表示 |

---

## 4. 時間計測の既存実装

### 時刻記録のタイミング

| イベント | 関数 | 行番号 | 記録先 |
|---------|------|--------|-------|
| 実行開始 | `handle_run()` | 460-481 | `State.started_at` |
| 実行終了 | `update_run_status()` | 1919-1925 | `State.completed_at` |
| タスク開始 | `task_started()` | 1865-1882 | `TaskState.started_at` |
| タスク終了 | `task_finished()` | 1884-1917 | `TaskState.completed_at` |

### 時刻取得方法
```rust
use chrono::{DateTime, Utc};
let now = Utc::now();  // 現在のUTC時刻
```

### 期間計算例
```rust
use chrono::Duration;

// 7日前からのフィルタリング
let cutoff = Utc::now() - Duration::days(7);
let filtered: Vec<_> = states
    .into_iter()
    .filter(|s| s.started_at >= cutoff)
    .collect();

// 実行時間の計算
if let (Some(start), Some(end)) = (task.started_at, task.completed_at) {
    let duration = end - start;
    let seconds = duration.num_seconds();
}
```

---

## 5. 実装方針の提案

### 修正が必要なファイル

| ファイル | 修正内容 | 工数目安 |
|---------|---------|---------|
| `src/cli.rs` | `Stats` コマンドの定義追加 | 小 |
| `src/main.rs` | `handle_stats()` 関数の実装 + dispatch追加 | 中 |

### 統計計算ロジック

```rust
fn handle_stats(cfg: &EffectiveConfig, since: Option<String>, json: bool) -> Result<()> {
    let store_root = resolve_store_path(cfg)?;
    let mut states = list_states(&store_root)?;

    // 1. 期間フィルタリング
    if let Some(since_str) = since {
        let duration = parse_duration(&since_str)?;  // "7d" -> Duration::days(7)
        let cutoff = Utc::now() - duration;
        states.retain(|s| s.started_at >= cutoff);
    }

    // 2. 統計計算
    let total_runs = states.len();
    let successful_runs = states.iter()
        .filter(|s| s.status == RunStatus::Completed)
        .count();
    let success_rate = if total_runs > 0 {
        (successful_runs as f64 / total_runs as f64) * 100.0
    } else {
        0.0
    };

    // 3. 平均実行時間
    let durations: Vec<_> = states.iter()
        .filter_map(|s| {
            s.completed_at.map(|end| (end - s.started_at).num_seconds())
        })
        .collect();
    let avg_duration = if !durations.is_empty() {
        durations.iter().sum::<i64>() / durations.len() as i64
    } else {
        0
    };

    // 4. 最も失敗したタスク
    let mut task_failures: HashMap<String, usize> = HashMap::new();
    for state in &states {
        for (task_id, task) in &state.tasks {
            if task.status == TaskStatus::Failed {
                *task_failures.entry(task_id.clone()).or_default() += 1;
            }
        }
    }
    let most_failed = task_failures.iter()
        .max_by_key(|(_, count)| *count)
        .map(|(id, count)| (id.clone(), *count));

    // 5. 最も時間がかかるタスク（平均）
    let mut task_durations: HashMap<String, Vec<i64>> = HashMap::new();
    for state in &states {
        for (task_id, task) in &state.tasks {
            if let (Some(start), Some(end)) = (task.started_at, task.completed_at) {
                task_durations.entry(task_id.clone())
                    .or_default()
                    .push((end - start).num_seconds());
            }
        }
    }
    let longest_task = task_durations.iter()
        .map(|(id, durs)| {
            let avg = durs.iter().sum::<i64>() / durs.len() as i64;
            (id.clone(), avg)
        })
        .max_by_key(|(_, avg)| *avg);

    // 6. 出力
    if json {
        // JSON出力
    } else {
        // テキスト出力
    }

    Ok(())
}
```

### 期間パーサー

```rust
fn parse_duration(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse()?;

    match unit {
        "s" => Ok(Duration::seconds(num)),
        "m" => Ok(Duration::minutes(num)),
        "h" => Ok(Duration::hours(num)),
        "d" => Ok(Duration::days(num)),
        "w" => Ok(Duration::weeks(num)),
        _ => Err(anyhow!("Invalid duration format: {}", s)),
    }
}
```

### 出力フォーマット

#### テキスト出力 (デフォルト)
```
Execution Statistics (last 7 days)
==================================
Total runs:       42
Success rate:     85.7% (36/42)
Avg duration:     2m 34s
Most failed task: build (5 failures)
Longest task:     test (avg 1m 45s)
```

#### JSON出力 (--json)
```json
{
  "period": {
    "since": "2024-01-14T00:00:00Z",
    "until": "2024-01-21T15:30:00Z"
  },
  "total_runs": 42,
  "successful_runs": 36,
  "failed_runs": 6,
  "success_rate": 85.71,
  "avg_duration_seconds": 154,
  "most_failed_task": {
    "task_id": "build",
    "failure_count": 5
  },
  "longest_task": {
    "task_id": "test",
    "avg_duration_seconds": 105
  }
}
```

---

## 6. 実装チェックリスト

- [ ] `src/cli.rs`: `Stats` コマンド variant を追加
- [ ] `src/main.rs`: dispatch に `Commands::Stats` の match arm を追加
- [ ] `src/main.rs`: `handle_stats()` 関数を実装
- [ ] `src/main.rs`: `parse_duration()` ヘルパー関数を実装
- [ ] テキスト出力フォーマットの実装
- [ ] JSON出力フォーマットの実装
- [ ] エラーハンドリング（無効な期間形式など）
- [ ] `cargo build` で動作確認
- [ ] `cargo test` でテスト確認

---

## 7. 依存関係

既存の依存関係で対応可能:
- `chrono`: 時刻計算（既に使用中）
- `serde_json`: JSON出力（既に使用中）
- `clap`: CLI引数パース（既に使用中）
- `anyhow`: エラーハンドリング（既に使用中）

新規依存関係は不要です。

---

## 8. テスト方針

1. **単体テスト**
   - `parse_duration()` のパース正確性
   - 統計計算ロジックの正確性

2. **統合テスト**
   - 複数の state.json を用意してstatsコマンドを実行
   - `--since` フィルタリングの動作確認
   - `--json` 出力フォーマットの確認

3. **エッジケース**
   - 実行履歴が0件の場合
   - 全て成功/全て失敗の場合
   - 未完了の実行がある場合
