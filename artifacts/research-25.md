**修正が必要なファイルとその概要**
- `src/cli.rs`  
  既存の `run --dry-run` フラグと `graph` サブコマンド定義がある。`quedex dry-run` サブコマンド追加、`--show-order/--estimate/--check-locks/--mermaid` のCLI定義が必要。互換維持なら `run --dry-run` を新ハンドラへ委譲。
- `src/main.rs`  
  `handle_dry_run` は Kahn のトポロジカルソート（`in_degree` + `dependents`）で順序を1列表示。`handle_graph` は `print_mermaid_graph`/`print_ascii_graph` を呼び出すだけ。ここに dry-run 拡張（Wave表示、ロック競合、推定、Mermaid）を実装・共通化するのが本線。
- `src/plan.rs`  
  DAG検証は `Plan::validate` で `petgraph::algo::is_cyclic_directed` を使ってサイクル検出。依存関係の正当性（存在確認・自己依存禁止）もここ。`Task.locks`/`timeout_sec` が既にあるので、推定時間に使うならここ由来を使うか、新しい見積もりフィールド追加なら schema/validation 変更が必要。
- `src/scheduler.rs`  
  依存解決は `refresh_ready` + `deps_satisfied/failed`。並列制御は `Semaphore`、ロックは `LockTable` + `try_acquire_locks`/`release_locks`。dry-run のWave生成やロック競合検出はここにある実行アルゴリズムを“模擬”するのが一番正確。
- `tests/scheduler_tests.rs`  
  ロック排他・依存順序・並列度のテストがある。dry-run のWave/lock競合/estimateのユニットテスト追加場所の候補。
- `README.md` / `OVERVIEW.md`  
  CLI一覧と挙動の説明更新が必要（`dry-run` 新設とオプション追加）。

**実装方針の提案**
- **CLI拡張**  
  `dry-run` サブコマンドを追加し、`run --dry-run` は互換のため新ハンドラに委譲。`--mermaid` は `graph` と同じ出力（DAG）を出せるよう共通関数化。
- **Wave表示（--show-order）**  
  依存関係のみでWaveを作るなら、Kahn法の「同時に取り出せる集合」をWaveとして表示（各Wave内はソートして決定的に）。  
  実行に近づけたいなら、`src/scheduler.rs` のロジックを模擬する “planner” を作り、`max_concurrency` と `locks` を考慮してWaveを生成（ready_queue + locks + semaphore を時間ステップで回す）。
- **ロック競合検出（--check-locks）**  
  `lock -> tasks` の逆引きを作り、同一ロックを持つタスク同士が DAG 上で順序づけされていない（到達関係がない）場合を「競合候補」として列挙。  
  依存グラフの到達性は `petgraph` を使って `has_path_connecting` 相当か、DFSで reachability を計算。
- **推定時間（--estimate）**  
  すぐに実装するなら `task.timeout_sec`（or `run.default_timeout_sec`）を「上限見積もり」としてWave合計（各Wave内は最大値、合計で総時間）を出す。  
  もう一歩なら `tasks[].estimate_sec` を新設して明示見積もりにする（`plan.rs`のschema更新が必要）。
- **Mermaid出力（--mermaid）**  
  既存の `print_mermaid_graph` を再利用。WaveをMermaidで表現したい場合は `subgraph Wave 1` などに拡張するが、まずは DAG 出力を優先。

必要なら、`artifacts/research-25.md` へこの内容を書き出す形にもできます。