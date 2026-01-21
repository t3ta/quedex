# Issue #24 TUI拡張設計 - 調査報告書

## 概要

タスクグループ / 階層化機能のTUI拡張に必要な調査結果をまとめる。

---

## 1. App構造体のフィールド一覧

**ファイル**: `src/tui/app.rs` L31-48

```rust
pub struct App {
    pub store_root: PathBuf,
    pub run_id: String,
    pub plan: Plan,                    // 実行計画
    pub state: State,                  // 実行状態
    pub tasks: Vec<TaskInfo>,          // タスク一覧
    pub list_state: TableState,        // テーブル選択状態
    pub log_stream: LogStream,         // stdout/stderr
    pub log_lines: Vec<String>,        // ログ行
    pub log_offset: usize,             // ログスクロール位置
    pub log_focus: bool,               // ログフォーカス状態
    pub graph_mode: bool,              // グラフ表示モード
    pub status_message: Option<String>, // ステータスメッセージ
    pub should_quit: bool,             // 終了フラグ
    store: FsStore,                    // ファイルストア
    log_path: PathBuf,                 // ログファイルパス
}
```

### 拡張ポイント

グループ折りたたみ状態を管理するため、以下の追加が必要:

```rust
pub collapsed_groups: HashSet<String>,  // 折りたたまれたグループ名の集合
```

追加メソッド:
- `toggle_group_collapse(&mut self, group: &str)` - 折りたたみ切り替え
- `get_visible_tasks(&self) -> Vec<&TaskInfo>` - 表示対象タスク取得

---

## 2. TaskInfo構造体のフィールド一覧

**ファイル**: `src/tui/app.rs` L15-21

```rust
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: String,           // タスクID
    pub title: String,        // タスクタイトル
    pub deps: Vec<String>,    // 依存タスクID
    pub locks: Vec<String>,   // ロック名
}
```

### 拡張ポイント

グループ情報をTaskInfoに追加:

```rust
pub group: Option<String>,  // タスクが属するグループ名（Planから初期化）
```

---

## 3. draw_tasks()関数の処理フロー

**ファイル**: `src/tui/ui.rs` L33-79

```
draw_tasks()
├─ app.tasks イテレーション
│  └─ 各タスク対象:
│     ├─ task_status() : TaskStatus取得
│     ├─ task_duration() : 実行時間計算
│     ├─ deps_remaining() : 残り依存関係数
│     └─ Row生成 → status_style()で色付け
├─ ヘッダ行生成
│  └─ ["id", "title", "status", "dur", "deps"]
├─ Table構築
│  ├─ widths制約設定
│  ├─ highlight_style (log_focus で判定)
│  └─ block (title, borders)
└─ render_stateful_widget() : list_state と共に描画
```

### 拡張ポイント

1. イテレーション前にタスクをグループでグループ化・ソート
2. グループヘッダ行を挿入 (例: `▼ backend (3 tasks)`)
3. 折りたたまれたグループのタスクをフィルタリング
4. グループヘッダ行の選択状態も管理

### 拡張後の処理フロー

```rust
draw_tasks() {
    // 1. グループ情報取得
    let all_groups = app.plan.resolve_groups();

    // 2. グループごとにタスクをまとめる
    let mut grouped_tasks: HashMap<Option<String>, Vec<TaskInfo>> = HashMap::new();
    for task in &app.tasks {
        let group = task.group.clone();
        grouped_tasks.entry(group).or_default().push(task.clone());
    }

    // 3. グループ順でソート
    let mut sorted_groups: Vec<_> = grouped_tasks.keys().collect();
    sorted_groups.sort();

    // 4. 各グループごとに行を生成
    for group in sorted_groups {
        if let Some(group_name) = group {
            // グループヘッダ行
            let is_collapsed = app.collapsed_groups.contains(group_name);
            let icon = if is_collapsed { "▶" } else { "▼" };
            let count = grouped_tasks[group].len();
            let header = format!("{} {} ({} tasks)", icon, group_name, count);
            rows.push(Row::new(vec![Cell::from(header)]));

            // タスク行（折りたたみ時は非表示）
            if !is_collapsed {
                for task in &grouped_tasks[group] {
                    rows.push(format_task_row(task, ...));
                }
            }
        } else {
            // グループなしタスク
            for task in &grouped_tasks[&None] {
                rows.push(format_task_row(task, ...));
            }
        }
    }
}
```

---

## 4. 現在のキーバインド一覧

**ファイル**: `src/tui/input.rs` L4-31

```rust
pub enum Action {
    Quit,              // 'q'
    Up,                // Up Arrow
    Down,              // Down Arrow
    ToggleLogFocus,    // Enter
    ToggleStream,      // 't' (stdout/stderr切り替え)
    Retry,             // 'r'
    CancelTask,        // 'c'
    CancelRun,         // 'C'
    ToggleGraph,       // 'g' (グラフモード切り替え)
}
```

### 拡張ポイント

新しいアクション追加:

```rust
ToggleGroupCollapse,  // グループ折りたたみ切り替え
```

**キーバインドの選択肢**:

| キー | 利点 | 欠点 |
|------|------|------|
| `Space` | 直感的、多くのTUIで使用 | 誤操作の可能性 |
| `Enter` (グループヘッダ行選択時) | コンテキスト依存で自然 | 実装が複雑 |
| `Tab` | 折りたたみで一般的 | 他用途との競合 |
| `z` | vim風（折りたたみ = fold） | 学習コスト |

**推奨**: `Space` または グループヘッダ行選択時の `Enter`

> 注意: `g` キーは既にグラフモード切り替えに使用されている

---

## 5. グラフモード（build_dependency_graph）の実装

**ファイル**: `src/tui/ui.rs` L140-290

### 5.1 グラフモード全体フロー

```
draw_graph()
├─ Layout: 垂直3分割
│  ├─ プログレスバー (3行)
│  ├─ 統計情報 (3行)
│  └─ グラフ描画エリア (可変)
├─ Gauge: 進捗率表示
├─ 統計情報: running/failed/locks
└─ build_dependency_graph() → グラフテキスト生成
```

### 5.2 build_dependency_graph() の実装 (L196-230)

```rust
fn build_dependency_graph(app: &App) -> Vec<Line<'_>> {
    // 1. children マップ構築
    //    - 各タスクが依存されている子タスクをマップ化

    // 2. ルートタスク特定
    //    - deps.is_empty() なタスク = 依存元なし

    // 3. 再帰的ツリー走査
    //    - render_task_tree() で各ルートから開始

    // 4. 出力: Vec<Line>
}
```

### 5.3 render_task_tree() の実装 (L232-290)

```rust
fn render_task_tree(
    task: &TaskInfo,
    children: &HashMap<&str, Vec<&str>>,
    app: &App,
    prefix: &str,      // インデント・接続線
    is_last: bool,     // 最後の子かどうか
    lines: &mut Vec<Line>,
    now: DateTime<Utc>
)
```

### グラフ表示例

```
* task-a [Running] 2m30s
├─ * task-b [Pending] -
│
└─ * task-c [Running] 1m00s
   ├─ * task-d [Succeeded] 30s
   │
   └─ * task-e [Failed] -
```

### 拡張ポイント

グループ対応の選択肢:

1. **グループ区切り線の挿入**
   ```
   ─────── backend ───────
   * api [Running] 2m30s
   └─ * db [Pending] -

   ─────── frontend ───────
   * ui [Pending] -
   ```

2. **グループヘッダ行の挿入**
   ```
   ▼ backend
     * api [Running] 2m30s
     └─ * db [Pending] -

   ▼ frontend
     * ui [Pending] -
   ```

3. **Mermaid subgraph対応** (main.rs)
   ```mermaid
   graph TD
     subgraph backend
       api --> db
     end
     subgraph frontend
       ui
     end
   ```

---

## 6. main.rs のグラフ関数

**ファイル**: `src/main.rs` L2112-2133

### print_mermaid_graph()

```rust
fn print_mermaid_graph(plan: &Plan) {
    println!("graph TD");

    for task in &plan.tasks {
        if task.deps.is_empty() {
            println!("  {};", task.id);
        }
        for dep in &task.deps {
            println!("  {} --> {};", dep, task.id);
        }
    }
}
```

### 拡張後

```rust
fn print_mermaid_graph(plan: &Plan) {
    println!("graph TD");

    // グループごとにsubgraph生成
    let groups = plan.resolve_groups();
    for (group_name, task_ids) in &groups {
        println!("  subgraph {}", group_name);
        for task_id in task_ids {
            if let Some(task) = plan.tasks.iter().find(|t| &t.id == task_id) {
                // タスクと依存関係を出力
            }
        }
        println!("  end");
    }

    // グループなしタスク
    // ...
}
```

---

## 7. TUI表示イメージ

### 折りたたみなし状態

```
┌─ Tasks (graph focus) ────────────────┐
│ id     title      status   dur  deps │
│ ▼ backend (3 tasks)                  │
│   api    API Task  Running  2m30s  0 │
│   db     DB Task   Ready    -      1 │
│   cache  Cache     Pending  -      1 │
│ ▼ frontend (2 tasks)                 │
│   ui     UI Task   Pending  -      3 │
│   test   Test      Pending  -      1 │
└──────────────────────────────────────┘
```

### 折りたたみあり状態

```
┌─ Tasks (graph focus) ────────────────┐
│ id     title      status   dur  deps │
│ ▶ backend (3 tasks)                  │
│ ▼ frontend (2 tasks)                 │
│   ui     UI Task   Pending  -      3 │
│   test   Test      Pending  -      1 │
└──────────────────────────────────────┘
```

---

## 8. 実装計画

### 8.1 変更対象ファイル

| ファイル | 変更内容 |
|---------|---------|
| `src/tui/app.rs` | `collapsed_groups` フィールド追加、グループ関連メソッド |
| `src/tui/ui.rs` | `draw_tasks()` グループ対応、`build_dependency_graph()` 拡張 |
| `src/tui/input.rs` | `ToggleGroupCollapse` アクション追加 |
| `src/main.rs` | `print_mermaid_graph()` / `print_ascii_graph()` グループ対応 |

### 8.2 依存関係

```
1. TaskInfo に group フィールド追加 (app.rs)
   ↓
2. App に collapsed_groups 追加 (app.rs)
   ↓
3. draw_tasks() グループ対応 (ui.rs)
   ↓
4. キーバインド追加 (input.rs)
   ↓
5. グラフモード拡張 (ui.rs, main.rs)
```

### 8.3 前提条件

- `src/plan.rs` で `Task.group` フィールドが既に定義されていること
- または `Plan.groups` から各タスクのグループを解決できること

---

## 9. 設計上の考慮事項

### 9.1 グループヘッダ行の選択

グループヘッダ行をテーブル内で選択可能にするかどうか:

**選択肢A: 選択可能**
- 利点: `Enter` でそのまま折りたたみ可能
- 欠点: `list_state` のインデックス管理が複雑化

**選択肢B: 選択不可（スキップ）**
- 利点: 実装シンプル
- 欠点: 別キー（Space等）での折りたたみが必要

**推奨**: 選択肢A（より直感的なUX）

### 9.2 グループの表示順序

1. アルファベット順
2. plan.json での定義順
3. タスク数順

**推奨**: plan.json での定義順（ユーザーの意図を尊重）

### 9.3 グループなしタスクの扱い

- 先頭に表示
- 末尾に表示
- `(ungrouped)` として表示

**推奨**: 先頭に表示（グループ未設定は基本タスクとして扱う）

---

## 10. 補足: 関連Issue

- Issue #22: グループ定義のスキーマ
- Issue #23: グループ操作CLI
- Issue #24: TUI拡張（本調査）
- Issue #25: グループ間依存関係

TUI拡張は Issue #22 のスキーマ定義に依存する。
