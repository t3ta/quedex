# Issue #24 タスクグループ / 階層化 スキーマ拡張設計

> 調査日: 2026-01-21（更新）
> 調査対象: quedex v0.x (Rust実装)
> ステータス: 調査完了 - 実装準備完了

---

## 概要

Issue #24「タスクグループ / 階層化」のスキーマ拡張設計書。
タスクを論理的にグループ化し、表示・管理を容易にする機能の追加。

**設計原則:**
- 後方互換性100%維持（既存plan.jsonがそのまま動作）
- 柔軟性重視（Plan.groups と Task.group の両方をサポート）
- スケジューラへの影響なし（表示/組織化のみ）

---

## 1. 現在の Plan/Task 構造

### Plan 構造体 (`src/plan.rs:62-73`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub version: u32,
    #[serde(default)]
    pub run: RunConfig,
    #[serde(default)]
    pub variables: HashMap<String, String>,
    pub tasks: Vec<Task>,
}
```

### Task 構造体 (`src/plan.rs:163-194`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub mode: TaskMode,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub locks: Vec<String>,
    #[serde(default)]
    pub timeout_sec: Option<u64>,
    #[serde(default)]
    pub no_worktree: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub codex: Option<CodexConfig>,
    #[serde(default)]
    pub claude_code: Option<ClaudeCodeConfig>,
    #[serde(default)]
    pub opencode: Option<OpencodeConfig>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub retry_delay_sec: u64,
    #[serde(default)]
    pub condition: Option<TaskCondition>,
}
```

### 主要な特徴

- **Derive マクロ**: `Debug, Clone, Serialize, Deserialize, JsonSchema`
- **schemars 使用**: バージョン 0.8、全構造体に `JsonSchema` derive 適用
- **フラット構造**: タスクは `Vec<Task>` で管理（階層なし）
- **依存関係**: 文字列ベースの ID 参照（`deps: Vec<String>`）

---

## 2. groups フィールド追加の設計

### 2.1 Plan 構造への追加

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub version: u32,
    #[serde(default)]
    pub run: RunConfig,
    #[serde(default)]
    pub variables: HashMap<String, String>,
    /// Task groups for logical organization.
    /// Maps group name to list of task IDs belonging to that group.
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
    pub tasks: Vec<Task>,
}
```

### 2.2 Task 構造への追加

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: String,
    // ... 既存フィールド ...

    /// Optional group this task belongs to.
    /// If specified, should match a key in Plan.groups.
    #[serde(default)]
    pub group: Option<String>,

    // ... 残りのフィールド ...
}
```

### 2.3 JSON スキーマの例

```json
{
  "version": 1,
  "groups": {
    "research": ["task-1", "task-2", "task-3"],
    "implementation": ["task-4", "task-5"],
    "verification": ["task-6"]
  },
  "tasks": [
    {
      "id": "task-1",
      "group": "research",
      "title": "調査タスク1",
      "mode": "Research",
      "deps": [],
      "codex": { "prompt": "..." }
    },
    {
      "id": "task-4",
      "group": "implementation",
      "title": "実装タスク1",
      "mode": "Implement",
      "deps": ["task-1", "task-2", "task-3"],
      "codex": { "prompt": "..." }
    }
  ]
}
```

---

## 3. バリデーションロジックの設計

### 3.1 追加するバリデーション項目

現在のバリデーション (`Plan::validate()` at `src/plan.rs:210-341`) に以下を追加:

```rust
// グループ名の検証（ID と同じルール）
fn validate_group_names(&self) -> Result<()> {
    for group_name in self.groups.keys() {
        if !group_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            bail!(
                "group name '{}' contains invalid characters (only alphanumeric, underscore, and hyphen allowed)",
                group_name
            );
        }
    }
    Ok(())
}

// groups 内のタスク ID が存在するか確認
fn validate_group_task_references(&self, task_ids: &HashSet<&str>) -> Result<()> {
    for (group_name, task_list) in &self.groups {
        for task_id in task_list {
            if !task_ids.contains(task_id.as_str()) {
                bail!(
                    "group '{}' references non-existent task '{}'",
                    group_name,
                    task_id
                );
            }
        }
    }
    Ok(())
}

// タスクの group フィールドが groups に定義されているか（警告のみ、エラーではない）
fn validate_task_group_references(&self) -> Vec<String> {
    let mut warnings = Vec::new();
    for task in &self.tasks {
        if let Some(ref group) = task.group {
            if !self.groups.contains_key(group) {
                warnings.push(format!(
                    "task '{}' references undefined group '{}' (this is allowed but may be unintentional)",
                    task.id,
                    group
                ));
            }
        }
    }
    warnings
}

// タスクが複数グループに属していないか確認
fn validate_no_duplicate_group_membership(&self) -> Result<()> {
    let mut task_groups: HashMap<&str, Vec<&str>> = HashMap::new();

    for (group_name, task_list) in &self.groups {
        for task_id in task_list {
            task_groups
                .entry(task_id.as_str())
                .or_default()
                .push(group_name.as_str());
        }
    }

    for (task_id, groups) in &task_groups {
        if groups.len() > 1 {
            bail!(
                "task '{}' is listed in multiple groups: {:?}",
                task_id,
                groups
            );
        }
    }
    Ok(())
}
```

### 3.2 バリデーション統合

```rust
pub fn validate(&self) -> Result<()> {
    // 既存のバリデーション...

    // 新規: グループ関連のバリデーション
    self.validate_group_names()?;
    self.validate_group_task_references(&ids)?;
    self.validate_no_duplicate_group_membership()?;

    // 警告（ログ出力のみ、エラーにはしない）
    let warnings = self.validate_task_group_references();
    for warning in warnings {
        tracing::warn!("{}", warning);
    }

    // 既存のバリデーション続き...
    Ok(())
}
```

---

## 4. JSON スキーマ（schemars）の更新方法

### 4.1 自動生成で対応可能

schemars は derive マクロで自動的にスキーマを生成するため、構造体にフィールドを追加するだけで JSON スキーマも更新される。

```rust
// Plan に追加
#[serde(default)]
pub groups: HashMap<String, Vec<String>>,

// Task に追加
#[serde(default)]
pub group: Option<String>,
```

### 4.2 スキーマ説明の追加（オプション）

より詳細なスキーマ説明が必要な場合:

```rust
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Task group definition for logical organization")]
pub struct Plan {
    // ...

    /// Task groups for logical organization.
    /// Maps group name to list of task IDs belonging to that group.
    #[schemars(description = "Map of group names to task ID lists")]
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,

    // ...
}
```

### 4.3 生成されるスキーマの例

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Plan",
  "type": "object",
  "required": ["version", "tasks"],
  "properties": {
    "version": { "type": "integer" },
    "groups": {
      "type": "object",
      "additionalProperties": {
        "type": "array",
        "items": { "type": "string" }
      },
      "default": {}
    },
    "tasks": {
      "type": "array",
      "items": { "$ref": "#/definitions/Task" }
    }
  },
  "definitions": {
    "Task": {
      "type": "object",
      "required": ["id", "mode"],
      "properties": {
        "id": { "type": "string" },
        "group": { "type": "string" },
        // ...
      }
    }
  }
}
```

---

## 5. 後方互換性の確認

### 5.1 互換性を保証する設計

| 項目 | 対応方法 | 互換性 |
|------|----------|--------|
| `Plan.groups` | `#[serde(default)]` で空の HashMap | ✅ 互換 |
| `Task.group` | `#[serde(default)]` で None | ✅ 互換 |
| バージョン番号 | version: 1 のまま維持 | ✅ 互換 |

### 5.2 既存 plan.json の動作確認

既存の plan.json（groups/group フィールドなし）:

```json
{
  "version": 1,
  "tasks": [
    { "id": "task-1", "mode": "Research", "codex": { "prompt": "..." } }
  ]
}
```

デシリアライズ結果:
- `plan.groups` → 空の HashMap `{}`
- `task.group` → `None`

**結論**: 既存の plan.json はそのまま動作する。

### 5.3 段階的移行パス

1. **Phase 1 (今回)**: groups/group フィールド追加（オプショナル）
2. **Phase 2 (将来)**: グループベースの依存関係構文サポート
3. **Phase 3 (将来)**: ネストしたグループのサポート

---

## 6. 実装チェックリスト

- [ ] `Plan` 構造体に `groups: HashMap<String, Vec<String>>` 追加
- [ ] `Task` 構造体に `group: Option<String>` 追加
- [ ] バリデーション関数の追加:
  - [ ] `validate_group_names()`
  - [ ] `validate_group_task_references()`
  - [ ] `validate_no_duplicate_group_membership()`
  - [ ] `validate_task_group_references()` (警告のみ)
- [ ] `Plan::validate()` にバリデーション呼び出し追加
- [ ] ユニットテスト追加:
  - [ ] 正常系: グループ定義あり
  - [ ] 正常系: グループ定義なし（後方互換性）
  - [ ] 異常系: 不正なグループ名
  - [ ] 異常系: 存在しないタスク参照
  - [ ] 異常系: 重複グループメンバーシップ
- [ ] `quedex schema` コマンドで更新されたスキーマを確認

---

## 7. 関連ファイル

| ファイル | 変更内容 |
|----------|----------|
| `src/plan.rs` | Plan/Task 構造体の拡張、バリデーション追加 |
| `src/scheduler.rs` | 変更不要（既存の依存関係ロジックで動作） |
| `src/store/mod.rs` | 変更不要 |
| `tests/` | ユニットテスト追加 |

---

## 8. 設計上の考慮事項

### 8.1 groups vs group の整合性

**設計選択**: Plan.groups と Task.group は独立して定義可能

- **Plan.groups**: グループの論理的な定義（タスクID一覧）
- **Task.group**: タスク側からのグループ所属宣言

**理由**:
- 柔軟性を確保（どちらか一方のみの使用も可能）
- 大規模プランでは Plan.groups でまとめて管理が便利
- 小規模プランでは Task.group で個別指定が便利

### 8.2 将来の拡張性

この設計は以下の拡張に対応可能:

1. **グループレベルの依存関係**: `deps: ["@research"]` でグループ全体に依存
2. **グループ設定の継承**: グループレベルの timeout_sec や retry_count
3. **ネストしたグループ**: `groups.subgroups` での階層化

---

## 9. 参考: 現在のバリデーション一覧

| 検証項目 | 場所 | エラー時の動作 |
|----------|------|----------------|
| バージョンチェック | L216-218 | bail! |
| 空タスク配列 | L221-223 | bail! |
| タスクID重複 | L226-234 | bail! |
| タスクID文字種 | L237-244 | bail! |
| 複数ランナー定義 | L248-265 | bail! |
| 空プロンプト | L268-298 | bail! |
| 依存タスク存在 | L301-307 | bail! |
| 自己参照依存 | L309-315 | bail! |
| 条件参照タスク存在 | L318-324 | bail! |
| 条件参照タスク依存 | L326-334 | bail! |
| 循環依存 | L337-341 | bail! |

---

## 10. ヘルパーメソッド設計

グループ機能を使いやすくするためのヘルパーメソッドを追加:

```rust
impl Plan {
    /// 指定グループに属するタスクIDリストを取得
    /// Plan.groups と Task.group の両方を統合
    pub fn get_group_tasks(&self, group: &str) -> Vec<&str> {
        let mut tasks = Vec::new();

        // Plan.groups から取得
        if let Some(task_ids) = self.groups.get(group) {
            tasks.extend(task_ids.iter().map(|s| s.as_str()));
        }

        // Task.group から取得（重複除外）
        for task in &self.tasks {
            if task.group.as_deref() == Some(group) && !tasks.contains(&task.id.as_str()) {
                tasks.push(&task.id);
            }
        }

        tasks
    }

    /// タスクが属するグループを取得
    pub fn get_task_group(&self, task_id: &str) -> Option<&str> {
        // Task.group を優先
        if let Some(task) = self.tasks.iter().find(|t| t.id == task_id) {
            if let Some(group) = &task.group {
                return Some(group);
            }
        }

        // Plan.groups から検索
        for (group_name, task_ids) in &self.groups {
            if task_ids.iter().any(|id| id == task_id) {
                return Some(group_name);
            }
        }

        None
    }

    /// 全グループ名を取得
    pub fn get_all_groups(&self) -> HashSet<&str> {
        let mut groups = HashSet::new();
        groups.extend(self.groups.keys().map(|s| s.as_str()));

        for task in &self.tasks {
            if let Some(group) = &task.group {
                groups.insert(group.as_str());
            }
        }

        groups
    }

    /// groups と task.group の両方を統合した結果を得る
    pub fn resolve_groups(&self) -> HashMap<String, Vec<String>> {
        let mut result: HashMap<String, Vec<String>> = self.groups.clone();

        // Task.group を追加
        for task in &self.tasks {
            if let Some(group) = &task.group {
                let entry = result.entry(group.clone()).or_default();
                if !entry.contains(&task.id) {
                    entry.push(task.id.clone());
                }
            }
        }

        result
    }
}
```

---

## 11. 追加テストケース

`tests/plan_validation_tests.rs` に追加するテスト:

```rust
#[test]
fn test_plan_with_groups() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "groups": {
            "research": ["task-1"],
            "impl": ["task-2"]
        },
        "tasks": [
            {"id": "task-1", "mode": "research", "codex": {"prompt": "test"}},
            {"id": "task-2", "mode": "implement", "deps": ["task-1"], "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    assert!(plan.validate().is_ok());
}

#[test]
fn test_plan_groups_invalid_task_id() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "groups": {
            "research": ["non-existent-task"]
        },
        "tasks": [
            {"id": "task-1", "mode": "research", "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    let err = plan.validate().unwrap_err();
    assert!(err.to_string().contains("non-existent task"));
}

#[test]
fn test_plan_task_in_multiple_groups() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "groups": {
            "group-a": ["task-1"],
            "group-b": ["task-1"]
        },
        "tasks": [
            {"id": "task-1", "mode": "research", "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    let err = plan.validate().unwrap_err();
    assert!(err.to_string().contains("multiple groups"));
}

#[test]
fn test_plan_task_group_field() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "tasks": [
            {"id": "task-1", "group": "research", "mode": "research", "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    assert!(plan.validate().is_ok());
    assert_eq!(plan.get_task_group("task-1"), Some("research"));
}

#[test]
fn test_plan_without_groups_backward_compat() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "tasks": [
            {"id": "task-1", "mode": "research", "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    assert!(plan.validate().is_ok());
    assert!(plan.groups.is_empty());
}

#[test]
fn test_plan_invalid_group_name() {
    let plan: Plan = serde_json::from_str(r#"{
        "version": 1,
        "groups": {
            "invalid group name!": ["task-1"]
        },
        "tasks": [
            {"id": "task-1", "mode": "research", "codex": {"prompt": "test"}}
        ]
    }"#).unwrap();
    let err = plan.validate().unwrap_err();
    assert!(err.to_string().contains("invalid characters"));
}
```

---

## 12. まとめ

### 設計決定事項

1. **Plan.groups**: `HashMap<String, Vec<String>>` として実装（`#[serde(default)]` で空マップ）
2. **Task.group**: `Option<String>` として実装
3. **バリデーション**: グループ名の文字種チェック、タスクID存在確認、重複所属チェック
4. **後方互換性**: 完全に維持（既存plan.jsonはそのまま動作）
5. **スキーマ更新**: `#[derive(JsonSchema)]` で自動対応

### 実装の優先順位

1. Plan/Task構造体へのフィールド追加
2. バリデーションロジック追加
3. ヘルパーメソッド追加
4. テスト追加
5. CLIコマンドへの `--group` オプション追加（別Issue）
6. TUIでのグループ表示対応（別Issue）

### 次のステップ

この設計書に基づいて `src/plan.rs` を実装する。
