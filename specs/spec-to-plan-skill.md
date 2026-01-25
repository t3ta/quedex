# Requirements Definition: spec-to-plan Skill

## 1. Project Overview

### 1.1 Background

現在、quedexでplan.jsonを作成する際に以下の課題がある：
- plan.jsonの手動作成が煩雑（DAG構造や依存関係を考えながらJSONを書く必要がある）
- 複雑なタスクを適切なサブタスクに分解する判断が難しい
- 要件定義からplan.json作成、実行までのフローが断片的で一貫性がない

### 1.2 Objectives

requirements-definitionスキルの手法を活用して、対話形式でspecを作成し、そのspecからplan.jsonを自動生成し、quedexで実行するまでを一貫して行うスキルを提供する。

### 1.3 Success Criteria

- インタビューだけで実行可能なplan.jsonが生成される
- タスク間の依存関係が正しく設定される
- 要件定義から実行までの一貫した体験が提供される

---

## 2. Target Users

### 2.1 User Personas

| Persona | Description | Goals | Pain Points |
|---------|-------------|-------|-------------|
| 開発者（自分自身） | quedex上級者、個人利用 | 開発ワークフローの効率化 | plan.json手動作成の手間、タスク分解の判断 |

### 2.2 User Journeys

1. `/spec-to-plan`でスキルを起動
2. チェックリスト形式のインタビューに回答
3. 生成されたplan.jsonを確認（JSON + 要約 + グラフ）
4. 確認後、自動でquedex runが実行される

---

## 3. Feature List

| ID | Feature | Priority | Description |
|----|---------|----------|-------------|
| F-001 | チェックリスト形式インタビュー | Must | 必要な情報を順番に確認していく対話形式 |
| F-002 | ユーザーストーリー・受け入れ基準の収集 | Must | 各機能のユーザーストーリーと完了条件を定義 |
| F-003 | 技術的な実装ヒントの収集 | Must | 使うべきライブラリ、API、設計パターンなどを収集 |
| F-004 | コンポーネント間の関係定義 | Must | 作成されるコンポーネント間の依存関係を明確化 |
| F-005 | タスク自動分解 | Must | 機能要件から実行可能なタスクへの分解 |
| F-006 | 依存関係の推論 | Must | research→implement→verifyの順序を自動設定 |
| F-007 | モードの自動判定 | Must | タスク内容からresearch/implement/verifyを判定 |
| F-008 | ロックの自動設定 | Must | 同じファイルを編集するタスクにロックを設定 |
| F-009 | plan.json確認（JSON表示） | Must | 生成されたJSONを全体表示 |
| F-010 | plan.json確認（タスク要約） | Must | タスク一覧と依存関係のテキスト説明 |
| F-011 | plan.json確認（Mermaidグラフ） | Must | DAGをMermaid形式で可視化 |
| F-012 | 確認後の自動実行 | Must | ユーザー確認後にquedex runを実行 |
| F-013 | セッションリカバリー | Should | 中断したインタビューの再開対応 |
| F-014 | 保存先の確認 | Should | spec、plan.jsonの保存先を毎回確認 |
| F-015 | ランナー選択（タスクごと） | Must | Codex CLI / Claude Codeをタスク単位で選択可能 |
| F-016 | モデル選択（Claude Code用） | Should | Claude Code使用時のモデル指定（デフォルト: Sonnet） |

---

## 4. User Stories

### US-001: インタビューによるspec作成

**As a** 開発者,
**I want** チェックリスト形式の質問に答えるだけでspecを作成したい,
**so that** plan.jsonを手動で書く手間を省ける.

**Acceptance Criteria:**
- [ ] `/spec-to-plan`でスキルが起動する
- [ ] 必要な情報（機能、優先度、技術ヒント等）が順番に質問される
- [ ] 回答内容がspec文書として保存される
- [ ] 中断しても後から再開できる

**Priority:** Must

---

### US-002: plan.jsonの自動生成

**As a** 開発者,
**I want** 作成したspecからplan.jsonが自動生成されたい,
**so that** DAG構造や依存関係を手動で設計する必要がない.

**Acceptance Criteria:**
- [ ] 機能要件がタスクに分解される
- [ ] タスク間の依存関係が自動設定される
- [ ] 各タスクにresearch/implement/verifyのモードが適切に設定される
- [ ] 同じファイルを編集するタスクにlocksが設定される
- [ ] タスクごとにcodex/claude-codeのランナーが指定できる

**Priority:** Must

---

### US-003: plan.jsonの確認

**As a** 開発者,
**I want** 生成されたplan.jsonを複数の形式で確認したい,
**so that** 実行前に問題がないか確認できる.

**Acceptance Criteria:**
- [ ] JSON全体が表示される
- [ ] タスク一覧と依存関係がテキストで要約される
- [ ] DAGがMermaid形式のグラフで表示される
- [ ] 確認後に修正や実行を選択できる

**Priority:** Must

---

### US-004: quedexでの自動実行

**As a** 開発者,
**I want** 確認後に自動でquedex runが実行されたい,
**so that** 別途コマンドを打つ手間が省ける.

**Acceptance Criteria:**
- [ ] ユーザーが「実行」を選択したらquedex runが開始される
- [ ] 実行状況が確認できる
- [ ] 実行をキャンセルする選択肢もある

**Priority:** Must

---

## 5. Non-Functional Requirements

### 5.1 Performance
- インタビューの各質問への応答は即座に行われる
- plan.json生成はspec確定後に実行される

### 5.2 Security
- 特になし（ローカル実行のみ）

### 5.3 Scalability
- 単一プロジェクト内のタスクを対象（複数プロジェクトは非対応）

### 5.4 Availability
- Claude Codeスキルとして常時利用可能

### 5.5 Compatibility
- quedex CLIとの互換性
- 既存の.claude/skills/構造に準拠

---

## 6. Constraints

### 6.1 Technical Constraints
- **タスク実行**: Codex CLI または Claude Code（Sonnetデフォルト）
- **shell実行**: 非対応（codexまたはclaude-codeのみ）
- **スキル配置**: .claude/skills/パターンに従う

### 6.2 Business Constraints
- 個人利用（チーム共有は考慮しない）

### 6.3 Regulatory Constraints
- なし

---

## 7. Out of Scope

| Item | Reason |
|------|--------|
| TUI監視の統合 | 既存のquedex tuiコマンドに任せる |
| 複数プロジェクトの統合実行 | 複雑性が増すため初期バージョンでは除外 |
| 既存plan.jsonの編集 | 新規作成に集中し、編集は手動で行う |
| shellタスクの実行 | codexとclaude-codeに限定 |

---

## 8. Definition of Done

スキルは以下の条件を満たすとき「完了」とする：

- [ ] `/spec-to-plan`でスキルが起動する
- [ ] チェックリスト形式でspecが作成できる
- [ ] specからplan.jsonが自動生成される
- [ ] 依存関係、モード、ロックが適切に設定される
- [ ] JSON、要約、Mermaidグラフで確認できる
- [ ] 確認後にquedex runが実行される
- [ ] 中断したセッションが再開できる
- [ ] SKILL.mdとリファレンスドキュメントが整備されている

---

## Appendix: plan.json Schema Reference

quedexのplan.jsonスキーマに準拠する。詳細は以下を参照：
- `.claude/skills/quedex/references/schema.md`
- `.claude/skills/quedex/references/examples.md`

### 拡張: ランナー設定

タスクごとにランナーを選択可能とする：

```json
{
  "id": "implement-feature",
  "mode": "implement",
  "codex": {
    "prompt": "Implement the feature..."
  }
}
```

または

```json
{
  "id": "implement-feature",
  "mode": "implement",
  "claude_code": {
    "prompt": "Implement the feature...",
    "model": "sonnet"
  }
}
```

**Note**: `claude_code`ランナーのサポートはquedex本体の拡張が必要になる可能性がある。
