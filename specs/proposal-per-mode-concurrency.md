# Proposal: モード別同時実行制御 (Per-Mode Concurrency Control)

## 背景

現在のquedexには以下の同時実行制御メカニズムがある:

- `max_concurrency`: 全タスク共通のグローバル上限（`Semaphore`で制御）
- `locks`: 名前付きロックによる排他制御（`LockTable`で管理）

workspaceロックにより、implementタスクの同時実行を防ぐ運用は可能だが、明示的なモード別制御は存在しない。OpenAI Symphonyのper-state concurrency limitsに着想を得て、より粒度の細かい制御を提案する。

## 提案内容

### `max_concurrency_by_mode` の追加

`RunConfig`に`max_concurrency_by_mode`フィールドを追加し、TaskMode（research / implement / verify）ごとに同時実行上限を設定可能にする。

```yaml
run:
  max_concurrency: 6
  max_concurrency_by_mode:
    research: 4
    implement: 1
    verify: 2
```

### セマンティクス

- タスク実行時に **グローバルpermit** と **モード別permit** の**両方**を取得する必要がある
- いずれか一方でも取得できない場合、タスクはready queueに戻る
- `max_concurrency_by_mode`が未指定の場合、グローバル`max_concurrency`のみが適用される（後方互換）
- 個別モードの指定は任意。例えば`implement: 1`のみ指定し、researchとverifyは制限なし（グローバル上限のみ）という設定も可能

## スキーマ変更

### `RunConfig` (`src/plan.rs`)

```rust
pub struct RunConfig {
    // ... 既存フィールド ...
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    #[serde(default)]
    pub max_concurrency_by_mode: Option<HashMap<TaskMode, usize>>,
}
```

`TaskMode`に`Hash`を derive追加。YAMLでのキーは`research` / `implement` / `verify`（既存の`rename_all = "snake_case"`を利用）。

### バリデーション (`Plan::validate()`)

- モード別上限がグローバル上限を超える場合は警告（エラーにはしない）
- 値が0の場合はエラー（そのモードのタスクが永久に実行不能になるため）

## 実装方針

### Scheduler (`src/scheduler.rs`)

```rust
pub struct SchedulerOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub max_concurrency_by_mode: HashMap<TaskMode, usize>,  // 追加
}
```

`run()`メソッド内で、グローバルSemaphoreに加えてモード別Semaphoreを作成:

```rust
let semaphore = Arc::new(Semaphore::new(max_concurrency));
let mode_semaphores: HashMap<TaskMode, Arc<Semaphore>> = options
    .max_concurrency_by_mode
    .iter()
    .map(|(mode, limit)| (*mode, Arc::new(Semaphore::new(*limit))))
    .collect();
```

タスクのディスパッチ時:

1. ロック取得を試行（既存の`try_acquire_locks`）
2. モード別Semaphoreが設定されている場合、`try_acquire_owned`を試行
3. グローバルSemaphoreの`try_acquire_owned`を試行
4. すべて成功したらタスクを実行、失敗したら取得済みのものを解放してqueueに戻す

タスク完了時:

- グローバルpermit・モード別permit共にdropで自動解放

## locksシステムとの関係

| 機能 | 用途 | 粒度 |
|------|------|------|
| `locks` | 特定リソースへの排他アクセス | タスク単位（任意の名前） |
| `max_concurrency_by_mode` | モード全体の並列度制御 | モード単位 |

**使い分け**:

- **locksが適切な場合**: 特定のファイルやディレクトリへの排他アクセスが必要な場合（例: workspaceロック）
- **per-mode concurrencyが適切な場合**: モード全体の負荷制御（例: researchタスクのAPI呼び出し上限、implementの同時書き込み数制限）

workspaceロックで`implement`タスクの同時実行を1に制限するのと`implement: 1`の設定は実質的に同等だが、`max_concurrency_by_mode`はより宣言的かつ明示的であり、ロック名の規約に依存しない。両方の指定は許容し、より厳しい方が実質的に適用される。

## 影響範囲

### 変更対象ファイル

- `src/plan.rs` - `RunConfig`にフィールド追加、`TaskMode`にHash derive追加、バリデーション追加
- `src/scheduler.rs` - `SchedulerOptions`にフィールド追加、モード別Semaphore管理
- `src/main.rs` - `SchedulerOptions`構築時にper-mode設定を渡す

### 変更不要

- `src/tui/` - 表示層への影響なし
- `src/dry_run.rs` - wave計算ロジックは変更不要（将来的にper-mode制約を反映する拡張は可能）
- 既存のlocksシステム - 変更なし

## テスト方針

- モード別上限を設定した場合、該当モードのタスクが上限を超えて同時実行されないことを確認
- モード別上限未設定時、既存の動作と同一であることを確認
- グローバル上限とモード別上限の組み合わせが正しく機能することを確認
- per-mode設定のYAML/JSONパース・バリデーションのユニットテスト
