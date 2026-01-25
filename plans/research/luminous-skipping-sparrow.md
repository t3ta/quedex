# quedex 機能追加・改善計画

## 概要

quedexに以下の6機能を追加し、ユーザー体験・信頼性・運用性を向上させる。
「小さな改善を複数」の方針に基づき、実装コストと効果のバランスが良い機能を選定。

## 選定機能一覧

| # | 機能 | カテゴリ | コスト | 効果 |
|---|------|----------|--------|------|
| 1 | `quedex init` | A. UX向上 | 小 | 高 |
| 2 | `--dry-run` | D. 運用改善 | 小 | 高 |
| 3 | `--verbose` | C. 信頼性 | 小 | 中 |
| 4 | `quedex history` | D. 運用改善 | 小 | 中 |
| 5 | 自動リトライ | C. 信頼性 | 中 | 高 |
| 6 | JSON Schema公開 | A. UX向上 | 中 | 高 |

---

## Phase 1: 独立した小機能（コスト小）

### 1. `quedex init` コマンド

**目的**: plan.jsonテンプレートを生成し、新規ユーザーの導入を容易にする

**変更ファイル**:
- `src/cli.rs`: Initサブコマンド追加
- `src/main.rs`: handle_init() 関数追加

**使用例**:
```bash
quedex init                    # plan.json を生成
quedex init -o myplan.json     # 出力ファイル名指定
quedex init --force            # 上書き許可
```

### 2. `--dry-run` オプション

**目的**: 実際にタスクを実行せず、実行計画を表示する

**変更ファイル**:
- `src/cli.rs`: GlobalOptionsにdry_runフラグ追加
- `src/main.rs`: handle_run()でドライラン処理

**使用例**:
```bash
quedex run plan.json --dry-run
# 出力例:
# Dry run mode - no tasks will be executed
# Plan: my-plan
# Tasks: 5
# Execution order:
#   1. research (deps: none)
#   2. design (deps: research)
#   ...
```

### 3. `--verbose` フラグ

**目的**: デバッグ用の詳細ログ出力

**変更ファイル**:
- `src/cli.rs`: GlobalOptionsにverboseフラグ追加
- `src/main.rs`: 各所でverboseログ出力

**使用例**:
```bash
quedex run plan.json --verbose
# [verbose] Loading plan from plan.json
# [verbose] Resolved cwd: /home/user/project
# [verbose] Task research started with pid 12345
```

### 4. `quedex history` コマンド

**目的**: 過去の実行履歴を一覧表示

**変更ファイル**:
- `src/cli.rs`: Historyサブコマンド追加
- `src/main.rs`: handle_history()関数追加

**使用例**:
```bash
quedex history              # 直近10件表示
quedex history --limit 20   # 件数指定
quedex history --all        # 全履歴
quedex history --json       # JSON形式出力
```

---

## Phase 2: 中規模機能

### 5. 自動リトライ機能

**目的**: タスク失敗時の自動リトライで信頼性向上

**変更ファイル**:
- `src/plan.rs`: Taskにretry_count, retry_delay_secフィールド追加
- `src/main.rs`: PlanTaskRunnerでリトライ処理

**plan.json例**:
```json
{
  "tasks": [
    {
      "id": "flaky-task",
      "retry_count": 3,
      "retry_delay_sec": 10,
      "codex": { "prompt": "..." }
    }
  ]
}
```

### 6. JSON Schema公開

**目的**: VSCode等でplan.jsonの自動補完・バリデーション対応

**変更ファイル**:
- `Cargo.toml`: schemars依存追加
- `src/plan.rs`: JsonSchema derive追加
- `src/cli.rs`: Schemaサブコマンド追加
- `src/main.rs`: handle_schema()関数追加

**使用例**:
```bash
quedex schema                    # スキーマをstdout出力
quedex schema -o schema.json     # ファイル出力
```

**VSCode設定例**:
```json
{
  "json.schemas": [
    {
      "fileMatch": ["plan.json", "*.plan.json"],
      "url": "./schema.json"
    }
  ]
}
```

---

## 実装順序

```
Phase 1 (並列実装可能):
  ├─ 1. quedex init
  ├─ 2. --dry-run
  ├─ 3. --verbose
  └─ 4. quedex history

Phase 2 (依存あり):
  5. 自動リトライ (plan.rs → main.rs)
  6. JSON Schema公開 (Cargo.toml → plan.rs → cli.rs → main.rs)
```

---

## 主要な変更ファイル

| ファイル | 変更内容 |
|----------|----------|
| `src/cli.rs` | Init, History, Schemaサブコマンド、--dry-run, --verboseフラグ追加 |
| `src/main.rs` | handle_init(), handle_history(), handle_schema()、ドライラン/verbose処理 |
| `src/plan.rs` | retry_count, retry_delay_sec、JsonSchema derive |
| `Cargo.toml` | schemars依存追加 |

---

## 検証方法

### 各機能のテスト

1. **quedex init**
   ```bash
   quedex init && cat plan.json
   quedex init --force  # 上書きテスト
   ```

2. **--dry-run**
   ```bash
   quedex run examples/dependencies.json --dry-run
   ```

3. **--verbose**
   ```bash
   quedex run plan.json --verbose 2>&1 | grep "\[verbose\]"
   ```

4. **quedex history**
   ```bash
   quedex run plan.json && quedex history
   ```

5. **自動リトライ**
   - retry_count: 2を設定したタスクを意図的に失敗させ、3回実行されることを確認

6. **JSON Schema**
   ```bash
   quedex schema > schema.json
   # VSCodeでplan.jsonを開き、補完が効くことを確認
   ```

### 既存テストの実行

```bash
cargo test
cargo clippy
```

---

## 後方互換性

- plan.jsonの新フィールド（retry_count, retry_delay_sec）は`#[serde(default)]`で既存planと互換
- 新コマンド/オプションは既存の動作に影響しない
