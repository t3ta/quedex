**調査結果**
- CLI終了コード処理: `main` は `dispatch` の戻り値をそのまま `exit` し、`Err` の場合は標準エラー出力して `1` を返します（`src/main.rs`）。`quedex run`/`retry` の最終終了コードは `finalize_run_status` によって決まり、`Completed=0 / FailedまたはSkipped=1 / Canceled=2` です（`src/main.rs`）。計画バリデーション失敗は `3`、バックエンドCLI未検出は `4` を返します（`src/main.rs`）。
- タスク単位の終了コード: 子プロセスの `ExitStatus` を `TaskResult` に変換し、`status.code()` をそのまま `exit_code` として保持します。タイムアウトは `124`、スポーン/待機エラーは `1`、シグナル(2/15)はキャンセル扱いです（`src/main.rs`）。タスク結果は `state.json` の `exit_code` に保存されます（`src/store/mod.rs`）。
- 出力形式（JSON/サマリー）: `quedex status --json` は `State` の pretty JSON、`history --json` は `State` 配列の pretty JSON を出力します（`src/main.rs`, `src/cli.rs`）。通常出力はテーブル/テキスト形式（`print_state`, `print_states_table`）です（`src/main.rs`）。`schema` は plan JSON schema を出力（`src/main.rs`）。タスクログは `stdout.log`/`stderr.log` に保存され、plan の `json` フラグが `true`（デフォルト）だと codex/claude/opencode の JSON 出力がそのままログに残ります（`src/runner/*.rs`, `src/plan.rs`, `src/store/mod.rs`, `src/store/fs.rs`）。TUIにはサマリー表示がありますが CLI には `GITHUB_STEP_SUMMARY` 連携はありません（`src/tui/ui.rs`）。
- action.yml 作成に必要なCLI情報: `run` コマンドは plan ファイル（または `-` で stdin）を実行し、隠し引数として `--run-id` / `--base-dir` を受け取れます（`src/cli.rs`, `src/main.rs`）。`--store` で保存先を指定可能、未指定なら `.quedex` があればそこ、なければ `$HOME/.quedex` です（`src/main.rs`）。`quedex` は実行終了時に `state.json` を更新しており、ここから status・タスク結果・開始/終了時刻を取得できます（`src/store/fs.rs`, `src/store/mod.rs`）。`run` は run_id を標準出力しないため、Actionでは `--run-id` を明示的に与えるのが安全です（`src/main.rs`）。
- 環境変数の処理: plan の `run.env` は各タスクの子プロセス環境に追加されます（`src/plan.rs`, `src/runner/*.rs`）。テンプレート `${env.VAR}` は **OSの環境変数**から読み、`run.env` は参照しません（`src/template.rs`）。`PATH` は親プロセスの `PATH` を明示的にセットして子プロセスへ引き継ぎます（`src/runner/*.rs`）。`HOME` は store 既定パス解決に使われます（`src/main.rs`）。`quedex.toml` はカレントにあれば読み込まれ、`max_concurrency/fail_fast/store/default_timeout` に影響します（`src/config.rs`）。

**必要なファイル構成（案）**
- `.github/actions/quedex/action.yml`（composite action 定義）
- `.github/actions/quedex/action.sh`（実行ロジック：インストール/実行/サマリー/出力/終了コード）
- 任意: `.github/actions/quedex/README.md`（利用方法・inputs/outputs）

**実装方針（提案）**
- `action.yml` で composite action を定義し、`action.sh` を `bash` で実行するステップを用意する。`actions/upload-artifact@v4` は `if: always()` で別ステップ化し、失敗時もログを回収。
- `action.sh` 内で `quedex` をインストール。入力 `version` が指定されていればそのバージョンを優先し、未指定なら `cargo build --release` または `cargo install --path .` でローカルビルドする（どちらにするかは運用方針次第）。
- `run_id` を明示的に発行して `quedex run --run-id "$RUN_ID" --store "$STORE"` を実行。`inputs.fail-fast=false` の場合は `--no-fail-fast` を付与。`plan` は `inputs.plan` をそのまま渡す。
- 実行後に `state.json`（`$STORE/runs/$RUN_ID/state.json`）を解析して `status` と `duration` を算出し、`GITHUB_OUTPUT` に出力。`duration` は `completed_at - started_at`（RFC3339）を `python`/`jq` で計算。
- `GITHUB_STEP_SUMMARY` に run_id、status、成功/失敗数、失敗タスク一覧を出力。ログ/成果物としては `$STORE/runs/$RUN_ID/**` と `artifacts/**`（plan で `output_last_message` を置いている想定）をアップロード。

**次の確認事項（必要なら教えてください）**
1. `version` 入力は「crate公開版」を想定していますか、それとも「このリポジトリのコードをビルド」想定ですか？
2. アップロード対象を `.quedex/runs/<run_id>` のみで良いか、`artifacts/` など追加パスもデフォルトで含めたいか決めてください。