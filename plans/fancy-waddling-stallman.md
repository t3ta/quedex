# retryコマンドでSkippedタスクも対象にする

## 概要
retryコマンドの対象ステータスに`Skipped`を追加し、fail_fast等でスキップされたタスクも再試行可能にする。

## 変更箇所

### `src/main.rs` (handle_retry関数内)

**現在のコード (行541-546)**:
```rust
if !matches!(task_state.status, TaskStatus::Failed | TaskStatus::Canceled) {
    return Err(anyhow!(
        "task {} must be Failed or Canceled to retry (current: {:?})",
        task_id,
        task_state.status
    ));
}
```

**変更後**:
```rust
if !matches!(task_state.status, TaskStatus::Failed | TaskStatus::Canceled | TaskStatus::Skipped) {
    return Err(anyhow!(
        "task {} must be Failed, Canceled or Skipped to retry (current: {:?})",
        task_id,
        task_state.status
    ));
}
```

## 検証方法

1. `cargo build` でビルドが通ることを確認
2. `cargo test` でテストが通ることを確認
3. 動作確認:
   - fail_fastモードでタスクを実行し、Skippedになったタスクを作る
   - `quedex retry <run_id> <task_id>` でSkippedタスクをretryできることを確認
