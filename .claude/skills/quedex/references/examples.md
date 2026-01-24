# Quedex Plan Examples

## 原則: 最小限のフィールドのみ書く

以下は**省略する**：
- `deps: []` → 依存がないなら書かない
- `locks: []` → ロックが不要なら書かない
- `kind: "codex"` → runner設定から自動推論
- `json: true` → デフォルト値
- `cwd: "."` → デフォルト値
- `verify_after: true` → implementモードのデフォルト
- `sandbox: "workspace-write"` → researchモードのデフォルト

---

## 基本パターン

### 単一の調査タスク

```json
{
  "version": 1,
  "run": { "name": "explore-auth" },
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "codex": {
        "prompt": "認証機能の実装を調査して",
        "output_last_message": "artifacts/auth.md"
      }
    }
  ]
}
```

### 調査 → 実装

最も一般的なパターン:

```json
{
  "version": 1,
  "run": { "name": "add-feature" },
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "codex": {
        "prompt": "ログイン機能の実装を調査して",
        "output_last_message": "artifacts/login.md"
      }
    },
    {
      "id": "implement",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "codex": { "prompt": "artifacts/login.md を参考にパスワードリセット機能を実装して" }
    }
  ]
}
```

### 並列調査 → 統合実装

複数領域を同時に調査:

```json
{
  "version": 1,
  "run": { "name": "multi-research", "max_concurrency": 3 },
  "tasks": [
    {
      "id": "research-api",
      "mode": "research",
      "codex": {
        "prompt": "API構造を調査して",
        "output_last_message": "artifacts/api.md"
      }
    },
    {
      "id": "research-db",
      "mode": "research",
      "codex": {
        "prompt": "DBスキーマを調査して",
        "output_last_message": "artifacts/db.md"
      }
    },
    {
      "id": "implement",
      "mode": "implement",
      "deps": ["research-api", "research-db"],
      "locks": ["workspace"],
      "codex": { "prompt": "artifacts/*.md の調査結果を踏まえて新機能を実装して" }
    }
  ]
}
```

---

## 高度なパターン（必要な場合のみ）

### locksで排他制御

同じリソースを操作するタスクを直列化:

```json
{
  "version": 1,
  "run": { "name": "migrations", "max_concurrency": 3 },
  "tasks": [
    {
      "id": "migration-users",
      "mode": "implement",
      "locks": ["db-migrate"],
      "codex": { "prompt": "usersテーブルにカラムを追加" }
    },
    {
      "id": "migration-posts",
      "mode": "implement",
      "locks": ["db-migrate"],
      "codex": { "prompt": "postsテーブルにカラムを追加" }
    }
  ]
}
```

deps不要でも`locks: ["db-migrate"]`で同時実行を防ぐ。

### 条件付き実行

```json
{
  "version": 1,
  "run": { "name": "conditional" },
  "tasks": [
    {
      "id": "build",
      "mode": "implement",
      "codex": { "prompt": "ビルドを実行" }
    },
    {
      "id": "deploy",
      "mode": "implement",
      "deps": ["build"],
      "condition": { "task": "build", "status": "succeeded" },
      "codex": { "prompt": "デプロイを実行" }
    }
  ]
}
```

### 自動リトライ

不安定なタスク（E2Eテスト等）に:

```json
{
  "id": "e2e-test",
  "mode": "verify",
  "retry_count": 3,
  "retry_delay_sec": 30,
  "codex": { "prompt": "E2Eテストを実行" }
}
```

### 変数（3箇所以上で同じ値を使う場合のみ）

```json
{
  "version": 1,
  "variables": {
    "target": "src/auth",
    "test_cmd": "npm test"
  },
  "run": { "name": "with-vars" },
  "tasks": [
    {
      "id": "research",
      "mode": "research",
      "codex": {
        "prompt": "${target} を調査して",
        "output_last_message": "artifacts/research.md"
      }
    },
    {
      "id": "implement",
      "mode": "implement",
      "deps": ["research"],
      "locks": ["workspace"],
      "codex": { "prompt": "${target} を改善して、完了後 ${test_cmd} を実行" }
    },
    {
      "id": "verify",
      "mode": "verify",
      "deps": ["implement"],
      "codex": { "prompt": "${test_cmd} で全テストがパスすることを確認" }
    }
  ]
}
```

**注意:** 1-2箇所でしか使わない値は変数にせず直書きする。

---

## アンチパターン（避けるべき例）

### ❌ 冗長なフィールド

```json
{
  "id": "task",
  "title": "タスク",
  "mode": "research",
  "deps": [],
  "locks": [],
  "kind": "codex",
  "codex": {
    "prompt": "調査して",
    "output_last_message": "out.md",
    "sandbox": "workspace-write",
    "json": true
  }
}
```

### ✅ 正しい書き方

```json
{
  "id": "task",
  "mode": "research",
  "codex": {
    "prompt": "調査して",
    "output_last_message": "out.md"
  }
}
```

### ❌ 不要な変数

```json
{
  "variables": { "dir": "src/api" },
  "tasks": [
    { "codex": { "prompt": "${dir} を調査" } }
  ]
}
```

1箇所でしか使わないなら変数にしない:

### ✅ 直書き

```json
{
  "tasks": [
    { "codex": { "prompt": "src/api を調査" } }
  ]
}
```
