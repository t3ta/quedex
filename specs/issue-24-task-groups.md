# Spec: タスクグループ / 階層化 (Issue #24)

## Phase 0: Discovered Information

### Project Overview
- **Project**: quedex - DAG-based task execution with Codex CLI integration
- **Language**: Rust
- **Build**: Cargo

### Relevant Files
- `src/plan.rs` - Plan/Taskスキーマ定義
- `src/cli.rs` - CLIコマンド定義
- `src/main.rs` - コマンドハンドラ (retry, cancel, status, graph)
- `src/tui/app.rs` - TUI状態管理
- `src/tui/ui.rs` - TUIレンダリング
- `src/dry_run.rs` - wave表示

### Existing Structures
```rust
// Plan構造 (src/plan.rs)
pub struct Plan {
    pub version: u32,
    pub run: RunConfig,
    pub variables: HashMap<String, String>,
    pub tasks: Vec<Task>,
}

// Task構造
pub struct Task {
    pub id: String,
    pub title: Option<String>,
    pub mode: TaskMode,
    pub deps: Vec<String>,
    pub locks: Vec<String>,
    // ... other fields
}
```

### CLI Commands to Extend
- `quedex retry <run_id> [task_id]`
- `quedex cancel <run_id> [task_id]`
- `quedex status <run_id>`
- `quedex graph`

---

## Phase 1: Project Overview

### 目的
大規模計画の整理のため、タスクをグループ化する機能を追加する。

### 成功基準
- **最重要**: グループ単位でのコマンド操作 (retry, cancel, status)
- TUIでのグループ折りたたみ表示
- グラフ表示でのsubgraph対応

### スコープ
- CLI: retry, cancel, status に `--group` オプション追加
- TUI: グループ折りたたみ表示
- グラフ: Mermaid/ASCII でのsubgraph表示

---

## Phase 2: Features

### Feature 1: スキーマ拡張 (Must)
**User Story**: 開発者として、plan.jsonでタスクをグループ化したい。

**Acceptance Criteria**:
- [ ] Plan構造に `groups: Option<HashMap<String, Vec<String>>>` を追加
- [ ] Task構造に `group: Option<String>` を追加
- [ ] 両方の定義方法をサポート（groups定義 / task.group）
- [ ] バリデーション（存在しないタスクIDの参照チェック）
- [ ] JSONスキーマ更新

### Feature 2: CLI拡張 (Must)
**User Story**: 開発者として、グループ単位でretry/cancel/statusを実行したい。

**Acceptance Criteria**:
- [ ] `quedex retry <run_id> --group <group_name>` が動作
- [ ] `quedex cancel <run_id> --group <group_name>` が動作
- [ ] `quedex status <run_id> --group <group_name>` が動作
- [ ] 存在しないグループ指定時のエラーメッセージ

### Feature 3: TUI拡張 (Should)
**User Story**: 開発者として、TUIでグループを折りたたんで表示を整理したい。

**Acceptance Criteria**:
- [ ] グループヘッダ行の表示（例: `▼ backend (3 tasks)`）
- [ ] `g` キーで折りたたみ切り替え
- [ ] 折りたたみ状態の保持
- [ ] グラフモードでもグループ表示

### Feature 4: グラフ拡張 (Should)
**User Story**: 開発者として、グラフ表示でグループを視覚的に確認したい。

**Acceptance Criteria**:
- [ ] Mermaidグラフでsubgraph対応
- [ ] ASCIIグラフでグループ区切り線
- [ ] dry-runでのグループ情報表示

---

## Phase 3: Technical Hints

### 参考パターン
- **Wave機構**: `src/dry_run.rs` の `generate_execution_waves()`
- **ロック管理**: `src/scheduler.rs` の `try_acquire_locks()`
- **再帰ツリー描画**: `src/tui/ui.rs` の `render_task_tree()`
- **CLIオプション**: `src/cli.rs` の `#[arg(long)]` パターン

### 設計方針
- `groups` はオプショナル（後方互換性維持）
- スケジューラのdeps/locks処理には影響なし（UI/表示層のみ）
- タスクIDの一意性は維持

---

## Phase 4: Components

### Component 1: スキーマ拡張
- **Files**: `src/plan.rs`
- **Lock**: plan.rs

### Component 2: CLI拡張
- **Files**: `src/cli.rs`, `src/main.rs`
- **Lock**: cli.rs, main.rs

### Component 3: TUI拡張
- **Files**: `src/tui/app.rs`, `src/tui/ui.rs`, `src/tui/mod.rs`
- **Lock**: tui

### Component 4: グラフ拡張
- **Files**: `src/main.rs` (print_mermaid_graph, print_ascii_graph)
- **Lock**: main.rs
- **Depends on**: Component 1 (スキーマ)

### Component 5: テスト・検証
- **Files**: `tests/`
- **No Lock**
- **Depends on**: All components

---

## Phase 5: Runner Selection

- **Default Runner**: claude_code
- **Model**: opus
- **全タスク共通設定**
