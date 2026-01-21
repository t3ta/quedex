**修正が必要なファイルと概要**
- `src/plan.rs` — Task struct はここにあり、現状のフィールドは `id/title/mode/deps/locks/timeout_sec/no_worktree/kind/codex/claude_code/opencode/retry_count/retry_delay_sec`。`condition` を追加するならここで型定義と `Plan::validate` を拡張。`Plan::parse_str` が plan.json/YAML のパース入口、`plan_json_schema()` がスキーマ生成。
- `src/scheduler.rs` — スケジューリングは `Scheduler::run` → `refresh_ready` が中核。`deps_satisfied` は **Succeeded のみ**、`deps_failed` は **Failed/Canceled/Skipped** を失敗扱い。`fail_fast` や `mark_stuck_tasks_skipped` も Skipped を付与。条件評価をここに入れる場合、TaskSpec/TaskRecord の拡張と依存判定の見直しが必須。
- `src/store/mod.rs` — TaskStatus は既に `Skipped` を含む。条件スキップとそれ以外を分けたいなら `SkipReason` などを `TaskState`/`TaskRecord` に追加するのが自然（`#[serde(default)]` で互換性維持）。
- `src/main.rs` — `TaskSpec` を構築して Scheduler に渡す場所。`finalize_run_status` は **Skipped が1つでもあると Failed** 扱い、`handle_retry` は依存が **Succeeded のみ** を許可。条件スキップを成功扱いにするならこのロジックの見直しが必要。plan.json の読み込みは `load_plan`/`parse_plan_with_fallback`/`load_plan_snapshot`。
- `tests/plan_validation_tests.rs` — Task 生成ヘルパーに `condition` のデフォルトを追加。条件バリデーション用テストを追加。
- `tests/scheduler_tests.rs` — 条件評価（env / task 結果）と「条件スキップは依存に影響しない」ことのテスト追加。`TaskSpec` 拡張に合わせて修正。

**実装方針の提案**
1. `src/plan.rs` に `TaskCondition` を定義し、`Task` に `condition: Option<TaskCondition>` を追加。形は `{"env": "...", "equals": "..."}` と `{"task": "...", "status": "failed"}` の **untagged enum** が最短。`Plan::validate` で env/equals の必須性、task の存在、status の妥当性を検証。`condition.task` は `deps` に含めることを **強制** するか、暗黙依存として扱うのを提案（条件評価の競合防止）。
2. `src/scheduler.rs` の `TaskSpec` を条件付きに拡張し、`refresh_ready` で **deps 満足後に条件評価**。条件不一致なら `Skipped` にし、`exit_code=None`。task条件は対象タスクが完了していなければ Pending のまま待つ（`deps` 強制なら不要）。
3. 条件スキップだけを成功扱いにするため、`TaskRecord`/`TaskState` に `skip_reason: Option<SkipReason>` を追加し、`deps_satisfied` は `Succeeded` か `Skipped + ConditionNotMet` を満たすよう変更。`deps_failed` は `Failed/Canceled` と「条件以外の Skipped」を失敗扱いにする。`finalize_run_status` と `handle_retry` も同じ判定に合わせる。
4. テスト追加: env 条件での Skipped、task 結果条件（failed など）、条件スキップ後に依存タスクが実行されること、非条件スキップが依存を止めること。

必要なら、次のステップとして「条件評価の具体的な仕様（env の参照元は OS 環境 or plan.run.env のマージか）」を決めると、実装がぶれません。