# quedex 実装案（Rust CLI）

## 0. 目的

Claude Code / Claude Agent が生成した **DAG（機械可読 plan）**を受け取り、Codex CLI を中心にタスクを

* 依存解決しつつ dispatch
* 並列度・排他（locks）を守って実行
* 途中で落ちても状態復元できるように永続化
* TUI で「今何が走ってて、何が詰まってて、どこで落ちたか」を見れる

LLM は「計画（plan）」まで。実行・監視・状態管理は quedex が責務。

---

## 1. 前提・設計方針

* タスクは **research / implement を分離**（1タスク内で混ぜない）。plan の `mode` で明示。
* **バックグラウンド実行**をファーストクラスにする（detach / 再接続）。
* Codex CLI が対話要求する可能性があるので、v0 では **非対話（approval=never）**を基本にする。

  * 対話が必要になったら `tmux` backend / `PTY attach` を後追いで入れる。
* 永続化は最初から入れる（「落ちたら終わり」を避ける）。

---

## 2. CLI 仕様

### 2.1 コマンド一覧（最小）

* `quedex run <plan.(json|yaml)|->`

  * 前面実行（終了コードで成功/失敗を返す）
* `quedex start <plan.(json|yaml)|->`

  * バックグラウンド実行（run_id を返して終了）
* `quedex status [run_id] [--json]`

  * 実行中/直近の run 一覧、または指定 run の状態
* `quedex tui [run_id]`

  * TUI 監視（ログ tail + タスク状態）
* `quedex logs <run_id> <task_id> [-f] [--stderr]`

  * ログ閲覧
* `quedex cancel <run_id> [task_id]`

  * run 全体または単一 task をキャンセル
* `quedex retry <run_id> <task_id>`

  * 失敗/キャンセル task を再実行（依存が満たされている場合）
* `quedex clean [run_id] [--all]`

  * run の状態・ログを削除（実行中は不可）
  * `--all`: まとめて削除（実行中はスキップ）
* `quedex graph <plan|run_id> [--mermaid|--ascii]`

  * DAG 表示

### 2.2 グローバルオプション

* `--store <path>`: 状態保存ディレクトリ（default: `./.quedex` 優先→なければ `~/.quedex`）
* `--max-concurrency <n>`: plan 未指定時の並列数
* `--fail-fast/--no-fail-fast`

### 2.3 終了コード（run）

* `0`: 成功（全タスク完了）
* `1`: 失敗（fail_fast で止まった / 依存が失敗で実行不可が発生）
* `2`: キャンセル
* `3`: plan バリデーションエラー
* `4`: 実行環境エラー（codex が見つからない等）

---

## 3. plan スキーマ

### 3.1 最小 JSON（v1）

```json
{
  "version": 1,
  "run": {
    "name": "auth-feature",
    "cwd": ".",
    "env": {"FOO": "bar"},
    "max_concurrency": 3,
    "fail_fast": true,
    "default_timeout_sec": 3600
  },
  "tasks": [
    {
      "id": "A",
      "title": "調査: 既存認証の把握",
      "mode": "research",
      "deps": [],
      "locks": [],
      "timeout_sec": 1800,
      "kind": "codex",
      "codex": {
        "prompt": "このリポジトリの認証フローを調査して要点をまとめて",
        "output_last_message": "artifacts/A_research.md",
        "sandbox": "workspace-write",
        "ask_for_approval": "never"
      }
    },
    {
      "id": "B",
      "title": "実装: 認証API",
      "mode": "implement",
      "deps": ["A"],
      "locks": ["workspace"],
      "kind": "codex",
      "codex": {
        "prompt": "認証APIを実装して。実装後 build→lint→test を実行し、エラーがあれば修正して。",
        "full_auto": true,
        "sandbox": "workspace-write",
        "ask_for_approval": "never"
      }
    },
    {
      "id": "C",
      "title": "検証: E2E",
      "mode": "verify",
      "deps": ["B"],
      "locks": ["workspace"],
      "kind": "shell",
      "shell": {
        "command": "bash -lc 'npm test:e2e'"
      }
    }
  ]
}
```

### 3.2 フィールド定義（要点）

* `tasks[].mode`: `research | implement | verify`

  * **バリデーション**: 1 task 内で mixed を禁止（文字列で強制）。
  * `implement` は原則 `locks` に `workspace` を自動付与するオプションも可（安全側デフォルト）。
* `tasks[].deps`: 依存 task id
* `tasks[].locks`: 排他リソース名

  * 例: `workspace`, `db-migrate`, `gpu0`, `net`, `docs` …
* `tasks[].timeout_sec`: 個別タイムアウト（なければ run の default）
* `kind`:

  * `codex`: `codex exec` を構築して実行
  * `shell`: 任意コマンド

### 3.3 バリデーション

* id 一意
* deps が存在
* cycles 無し（DAG）
* `codex.prompt` 非空
* `codex.output_last_message` は `mode=research` のみ許可（混乱を避ける）

---

## 4. 実行モデル

### 4.1 タスク状態

* `Pending`（未実行）
* `Ready`（deps satisfied）
* `Running`
* `Succeeded`
* `Failed(exit_code)`
* `Canceled`
* `Skipped(reason)`（依存失敗で実行不可など）
* `NeedsAttention(reason)`（将来: 対話が必要等）

### 4.2 fail_fast

* `true`: いずれかが `Failed` になった時点で

  * 未開始タスクを `Skipped`
  * 実行中タスクは `cancel_on_fail_fast` オプションで kill する/しない
* `false`: 可能なものは最後まで走らせる

### 4.3 リトライ

* `tasks[].retry.max`
* `tasks[].retry.backoff_sec`
* 失敗理由が一時的（ネット/タイムアウト）かの自動判定は v0 ではしない（手動 retry で十分）。

---

## 5. スケジューラ（依存＋locks＋並列）

### 5.1 コア要件

* deps で Ready になった task をキューに積む
* グローバル `max_concurrency` を超えない
* `locks` が衝突する task を同時に走らせない

### 5.2 実装方針（Tokio）

* `Semaphore(max_concurrency)`
* `LockTable: HashMap<String, Option<TaskId>>` を `Mutex` で保護
* Ready キュー（`VecDeque<TaskId>`）

擬似コード：

```text
loop:
  refresh ready set
  while has_ready_task && permits_available:
    pick task
    if locks available:
      acquire locks
      acquire permit
      spawn runner(task) -> on_exit release locks/permit, emit event
    else:
      rotate task back
  if all done: break
  wait for any event (task exit / cancel)
```

### 5.3 公平性

* locks が取れず詰まる task があるので「回転」させる
* それでも飢餓が起きるなら、`locks` ごとに待ち行列を持つ（v1で可）

---

## 6. Runner（実行バックエンド）

### 6.1 共通インターフェイス

* `Runner::spawn(task, ctx) -> ChildHandle`
* `ChildHandle`:

  * `pid`
  * `stdout_path`, `stderr_path`
  * `kill()`

### 6.2 Codex runner（v0）

#### 方針

* デフォルトは **非対話**

  * `ask_for_approval: never`
* `research`:

  * `codex exec <prompt> --output-last-message <path>`
* `implement`:

  * `codex exec <prompt> --full-auto` 相当を組み立てる
  * `--sandbox workspace-write` をデフォルト

#### コマンド構築例

* research:

  * `codex exec "..." --output-last-message artifacts/A.md`
* implement:

  * `codex exec "..." --full-auto --ask-for-approval never --sandbox workspace-write`

※ `--full-auto` が内部で approval を持っている場合でも、明示で上書きできる設計にしておく。

### 6.3 Shell runner

* `bash -lc '<cmd>'` を基本（cwd/env を引き継ぐ）

### 6.4 対話が欲しくなった場合（後続）

* `--backend tmux`:

  * 各 task を `tmux new-window -n <task_id> -- <command>` で実行
  * `quedex attach <run_id>` で tmux session に入る
* `PTY attach`:

  * `portable-pty` で疑似端末を確保し、daemon が PTY を持ったまま走る
  * attach は IPC が要るので v2 以降（最初からやると重い）

---

## 7. 永続化（落ちても復元）

### 7.1 ストレージレイアウト

`<store>/runs/<run_id>/`

* `plan.json`（正規化して保存）
* `state.json`（最新スナップショット）
* `events.jsonl`（追記イベントログ）
* `tasks/<task_id>/stdout.log`
* `tasks/<task_id>/stderr.log`
* `tasks/<task_id>/meta.json`（pid, start/end, exit_code, locks など）

### 7.2 イベント設計（JSONL）

例：

```json
{"ts":"...","type":"RunStarted","run_id":"..."}
{"ts":"...","type":"TaskStarted","task_id":"A","pid":123}
{"ts":"...","type":"TaskExited","task_id":"A","code":0}
```

### 7.3 state.json

* TUI/status 用の集約状態
* 更新は「イベント追記 → state 更新 → atomic rename」
* TUI は state.json を見るだけで高速に描画可能

---

## 8. TUI 仕様（ratatui）

### 8.1 画面構成

* 左（一覧）: task_id / title / status / duration / deps残
* 右（ログ）: 選択 task の stdout tail（切替で stderr）
* 下（全体）: done/total, running数, fail数, locks状態（簡易）

### 8.2 操作

* ↑↓: 選択
* `Enter`: ログフォーカス
* `t`: stdout/stderr 切替
* `r`: retry
* `c`: cancel task
* `C`: cancel run
* `g`: graph 表示（別画面）
* `q`: quit

### 8.3 更新方式

* v0: `state.json` とログファイルを `notify` で watch（できなければ 200ms タイマー fallback）

---

## 9. 実装構成（Rust モジュール）

```
src/
  main.rs
  cli.rs            // clap
  plan.rs           // serde + validation
  run_id.rs         // run id generator
  scheduler.rs      // deps + locks + concurrency
  runner/
    mod.rs
    codex.rs
    shell.rs
  store/
    mod.rs
    fs.rs           // events.jsonl + state.json + logs
  tui/
    mod.rs
    app.rs
    ui.rs
    input.rs
```

主要 trait:

* `Store`: `append_event`, `write_state`, `read_state`, `open_log_tail`
* `Runner`: `spawn`, `kill`

---

## 10. 重要な仕様決め（v0で固定すべき）

1. **non-interactive をデフォルト**にする（approval=never）

* 背景実行の安定性が最優先

2. **locks のデフォルト**

* `mode=implement|verify` は `workspace` lock を暗黙付与（安全）
* `mode=research` は lock 無し
* ただし plan で明示したらそれを優先

3. **cwd の扱い**

* run.cwd を基準に、task.cwd があれば上書き

4. **停止シグナル**

* v0: SIGTERM → 一定時間後 SIGKILL（Unix前提）
* WSL でも概ね動く想定

---

## 11. 開発ロードマップ（最短で動かす）

### v0（動く最小）

* plan parse/validate
* scheduler（deps+locks+concurrency）
* runner（codex/shell）
* store（logs + state.json）
* `run/start/status/logs/cancel/clean/graph`

### v1（使い勝手）

* `retry`
* TUI
* state 復元強化（途中再開の扱い）

### v2（対話対応が必要になったら）

* `--backend tmux`
* `attach`（tmux sessionへ）

---

## 12. テスト方針

* scheduler 単体テスト

  * deps順序
  * locks 排他
  * fail_fast
* runner は `echo/sleep` の shell で結合テスト
* store は `tempdir` で events/state の整合性テスト

---

## 13. 次に決めるべき最小の合意点

* plan を JSON/YAML どっちを正式にするか（両対応でも良いが v0 は JSON のみでもOK）
* `implement` の暗黙 lock（安全デフォルト）を採用するか
* `--ask-for-approval` を plan 側に残すか、CLI 側で強制するか（v0 は CLI 強制が安全）
