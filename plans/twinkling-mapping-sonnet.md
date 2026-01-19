# opencode ランナー追加実装計画

## 概要
quedexプロジェクトに `opencode` CLI を使用する新しいランナーを追加する。

## opencode CLI 仕様
- 非対話モード: `opencode run "prompt"`
- モデル指定: `-m provider/model`
- 出力フォーマット: `--format json`

## 変更対象ファイル

### 1. `src/plan.rs`
- **OpencodeConfig 構造体追加**
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct OpencodeConfig {
      pub prompt: String,
      #[serde(default)]
      pub model: Option<String>,
      #[serde(default = "default_json")]
      pub json: bool,
  }
  ```
- **Task 構造体に `opencode` フィールド追加**
- **validate() メソッド更新**
  - runner_count に `task.opencode.is_some()` 追加
  - kind マッチに `"opencode"` 追加
  - opencode.prompt の空チェック追加

### 2. `src/runner/opencode.rs` (新規作成)
```rust
#[derive(Clone, Copy)]
pub struct OpencodeRunner;

impl Runner for OpencodeRunner {
    fn spawn(&self, task: &Task, ctx: &RunContext) -> Result<ChildHandle> {
        // opencode run --format json -m <model> "<prompt>"
    }
}
```

### 3. `src/runner/mod.rs`
- `pub mod opencode;` 追加

### 4. `src/main.rs`
- **import追加**: `use quedex::runner::opencode::OpencodeRunner;`
- **check_opencode_available() 関数追加**
- **handle_run, handle_start, handle_retry に環境確認追加**
- **PlanTaskRunner に opencode フィールド追加**
- **ランナー選択ロジック更新**

### 5. `tests/plan_validation_tests.rs`
- `OpencodeConfig` の import 追加
- `opencode_task` ヘルパー関数追加
- テストケース追加:
  - `plan_rejects_empty_opencode_prompt`
  - `plan_accepts_valid_opencode_task`
  - `plan_rejects_kind_mismatch_opencode`
  - `plan_rejects_multiple_runner_configs_with_opencode`

## 実装順序
1. `src/plan.rs` - OpencodeConfig定義、Task更新、バリデーション
2. `src/runner/opencode.rs` - 新規作成
3. `src/runner/mod.rs` - モジュール追加
4. `src/main.rs` - ランナー統合
5. `tests/plan_validation_tests.rs` - テスト追加

## 検証方法
1. `cargo build` - コンパイル確認
2. `cargo test` - ユニットテスト
3. `cargo clippy` - lint確認
4. 手動テスト: plan.json作成して `quedex run` 実行

## plan.json 使用例
```json
{
  "version": 1,
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "opencode": {
        "prompt": "プロジェクト構造を調査",
        "model": "anthropic/claude-sonnet"
      }
    }
  ]
}
```

## Sources
- [OpenCode CLI Documentation](https://opencode.ai/docs/cli/)
- [OpenCode GitHub](https://github.com/opencode-ai/opencode)
