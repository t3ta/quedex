# Proposal: External State Reconciliation (外部状態との突合)

## 背景と動機

### 現状の recovery 機構

quedex は event sourcing ベースのリカバリ機構を持つ:

- `events.jsonl` によるイベント再生
- `state.json` のスナップショット
- `recovery.rs` でのプロセス生存チェック (`kill -0`)

この仕組みは quedex 自身の内部状態に閉じている。外部の情報源（issue tracker 等）との整合性は検証しない。

### OpenAI Symphony からの着想

OpenAI Symphony は DB を持たず、Linear（issue tracker）を唯一の情報源 (source of truth) として扱う。再起動時に Linear をポーリングし、ローカル状態と突合することでステートレスなリカバリを実現している。

quedex では `quedex watch` による issue tracker 連携が想定されている。この場合、issue tracker が外部の source of truth となり、内部状態との不整合が発生しうる:

- 外部で issue が close されたが、quedex 側のタスクがまだ running/pending
- 外部で issue が reopen されたが、quedex 側では canceled/succeeded のまま
- quedex 側で running だが、プロセスも死んでおり、対応する issue も存在しない（孤立した run）

本 proposal は、既存の recovery 機構を**置き換えるものではなく補完する**ものである。

---

## 前提

- Issue tracker 連携 proposal（`quedex watch`）が先行して実装されていること
- 外部 tracker の API アクセスが可能であること（認証設定済み）

---

## 設計方針

### 権威性: 外部状態が勝つ

Reconciliation の基本戦略は **external-wins**（外部状態優先）とする。

- issue tracker の状態が quedex の内部状態と矛盾する場合、issue tracker 側を正とする
- これは `quedex watch` モードにおいて issue tracker が「何をすべきか」を定義する存在であるため

### 既存 recovery との関係

```
restart/resume フロー:

1. events.jsonl からの内部状態復元      ← 既存 (recovery.rs)
2. プロセス生存チェック (kill -0)        ← 既存 (recovery.rs)
3. 外部 tracker への問い合わせ           ← 本 proposal (新規)
4. 内部状態と外部状態の突合・修正         ← 本 proposal (新規)
```

ステップ 1-2 は変更しない。ステップ 3-4 が新たに追加される。
`quedex watch` を使わない通常の `quedex run` では、ステップ 3-4 はスキップされる。

---

## Feature 1: Reconciliation エンジン (Must)

### 概要

内部状態と外部 tracker の状態を突合し、差分を検出・解消する reconciliation エンジンを実装する。

### 突合ルール

| 内部状態 | 外部状態 | アクション |
|---------|---------|-----------|
| Running / Pending | issue closed | タスクを cancel |
| Canceled / Failed | issue reopened | タスクを re-queue (Pending に戻す) |
| Running | プロセス死亡 + issue なし | タスクを fail (孤立 run の検出) |
| Succeeded | issue closed | 変更なし (正常完了) |
| Pending | issue open | 変更なし (実行待ち) |

### ReconciliationReport

```rust
pub struct ReconciliationReport {
    /// 突合対象の run_id
    pub run_id: String,
    /// cancel されたタスク (外部で close)
    pub canceled_tasks: Vec<ReconcileAction>,
    /// re-queue されたタスク (外部で reopen)
    pub requeued_tasks: Vec<ReconcileAction>,
    /// 孤立として検出されたタスク
    pub orphaned_tasks: Vec<ReconcileAction>,
    /// 変更なし (状態一致)
    pub unchanged_tasks: Vec<String>,
}

pub struct ReconcileAction {
    pub task_id: String,
    pub internal_status: TaskStatus,
    pub external_status: ExternalIssueStatus,
    pub action: ReconcileActionType,
    pub reason: String,
}

pub enum ReconcileActionType {
    Cancel,
    Requeue,
    MarkFailed,
    NoOp,
}
```

### イベントログ

すべての reconciliation アクションは `events.jsonl` に記録する:

```rust
// Event enum への追加
Event::TaskReconciled {
    task_id: String,
    action: String,        // "cancel" | "requeue" | "mark_failed"
    reason: String,        // 例: "issue closed externally"
    external_status: String,
    #[serde(rename = "ts")]
    timestamp: DateTime<Utc>,
}
```

これにより、reconciliation による状態変更も既存の event sourcing の仕組みで追跡・再生可能になる。

---

## Feature 2: `quedex reconcile` コマンド (Must)

### 概要

手動で reconciliation を実行するための CLI コマンド。

### コマンド仕様

```
quedex reconcile [run_id]           # 指定 run の突合を実行
quedex reconcile --all              # 全 active run の突合を実行
quedex reconcile [run_id] --dry-run # 変更せず差分のみ表示
```

### dry-run モード

`--dry-run` フラグを指定すると、実際の状態変更を行わずに突合結果のみを表示する:

```
$ quedex reconcile abc123 --dry-run

Reconciliation preview for run abc123:
  [CANCEL]  task-auth    Running → Cancel  (issue #42 closed)
  [REQUEUE] task-api     Canceled → Pending (issue #43 reopened)
  [ORPHAN]  task-db      Running → Failed  (process dead, no matching issue)
  [OK]      task-ui      Pending           (issue #44 open)

3 changes would be applied. Run without --dry-run to execute.
```

### Acceptance Criteria

- [ ] `quedex reconcile <run_id>` で指定 run の突合が実行される
- [ ] `quedex reconcile --all` で全 active run の突合が実行される
- [ ] `--dry-run` で変更なしのプレビューが表示される
- [ ] 突合結果が `ReconciliationReport` として構造化される
- [ ] すべてのアクションが `events.jsonl` に記録される
- [ ] 外部 tracker 未設定時は明確なエラーメッセージを表示

---

## Feature 3: `quedex watch` での定期 reconciliation (Should)

### 概要

`quedex watch` 実行中に、設定可能な間隔で自動的に reconciliation を実行する。

### 設定

```yaml
# quedex.toml or plan.json
watch:
  reconcile_interval_sec: 60   # デフォルト: 60秒
  reconcile_on_resume: true    # resume 時に自動実行 (デフォルト: true)
```

### 動作フロー

```
quedex watch 起動
  ├── 初回: reconcile 実行
  ├── issue tracker ポーリング (既存)
  ├── タスクスケジューリング (既存)
  └── 定期: reconcile_interval_sec ごとに reconcile 実行
        ├── 外部状態取得
        ├── 内部状態と突合
        ├── 差分があれば状態更新 + イベント記録
        └── ログ出力
```

### Acceptance Criteria

- [ ] `quedex watch` 起動時（resume 含む）に自動 reconciliation が実行される
- [ ] `reconcile_interval_sec` で指定した間隔で定期実行される
- [ ] reconciliation 中もタスク実行はブロックされない（非同期実行）
- [ ] 設定で定期 reconciliation を無効化できる (`reconcile_interval_sec: 0`)

---

## Feature 4: Reconciliation trait (Must)

### 概要

外部 tracker との通信を抽象化する trait を定義する。これにより、異なる tracker (GitHub Issues, Linear, Jira 等) に対応可能にする。

### インターフェース

```rust
/// 外部 issue tracker との通信を抽象化する trait
#[async_trait]
pub trait ExternalTracker: Send + Sync {
    /// タスクに対応する issue の現在の状態を取得
    async fn get_issue_status(&self, task_id: &str) -> Result<Option<ExternalIssueStatus>>;

    /// 複数タスクの issue 状態を一括取得（バッチ API 対応）
    async fn get_issue_statuses(
        &self,
        task_ids: &[String],
    ) -> Result<HashMap<String, ExternalIssueStatus>>;
}

pub enum ExternalIssueStatus {
    Open,
    InProgress,
    Closed,
    /// Tracker 上に対応する issue が存在しない
    NotFound,
}
```

### 設計意図

- `ExternalTracker` trait により、テスト時にモック実装が使える
- `get_issue_statuses` のバッチ API により、API レート制限への対策が可能
- `NotFound` は issue が削除された場合のハンドリングに使用

---

## 実装方針

### ファイル構成

```
src/
  store/
    mod.rs          # Event enum に TaskReconciled を追加
    recovery.rs     # 既存のまま (変更なし)
  reconcile/
    mod.rs          # ReconciliationEngine, ReconciliationReport
    tracker.rs      # ExternalTracker trait
    actions.rs      # 突合ルール・アクション実行
  cli.rs            # reconcile サブコマンド追加
```

### 既存コードへの影響

- `src/store/mod.rs`: `Event` enum に `TaskReconciled` variant を追加
- `src/store/recovery.rs`: 変更なし
- `src/cli.rs`: `reconcile` サブコマンドを追加
- `src/main.rs`: `reconcile` コマンドハンドラを追加

recovery.rs の `recover_running_tasks()` は内部状態のリカバリに専念し、外部状態との突合は新規の reconciliation エンジンが担当する。責務の分離を維持する。

---

## スコープ外

- 外部 tracker への書き戻し（quedex 側の状態変更を tracker に反映する）は本 proposal の対象外。将来の双方向同期 proposal で扱う
- 具体的な tracker 実装（GitHub Issues adapter, Linear adapter 等）は issue tracker 連携 proposal の範囲
- Conflict resolution の UI（ユーザーに確認を求めるインタラクティブモード）

---

## リスクと対策

| リスク | 対策 |
|-------|------|
| 外部 API の一時的な障害で誤った reconciliation が発生 | API エラー時は reconciliation をスキップし、次回に持ち越す。状態変更は行わない |
| API レート制限 | バッチ API (`get_issue_statuses`) の利用、`reconcile_interval_sec` の調整 |
| reconciliation 中にタスク状態が変化 | reconciliation は内部状態のスナップショットに対して実行。競合時は internal 変更を優先（外部 wins は次回の reconciliation で反映） |

---

## 参考

- [OpenAI Symphony](https://github.com/openai/symphony) - DB-free stateless recovery pattern
- `src/store/recovery.rs` - 既存の内部状態リカバリ機構
- `src/store/mod.rs` - Event / State / TaskStatus 定義
