調査結果を `artifacts/research.md` にまとめました。TUI が期待する JSON イベント構造は `src/tui/app.rs` の整形ロジックから推測可能で、リポジトリ内に Codex CLI の正式スキーマ定義はありません（その旨も記載しています）。

- 主要参照: `src/tui/app.rs`, `src/tui/ui.rs`, `src/tui/mod.rs`, `src/runner/codex.rs`, `src/plan.rs`, `src/main.rs`
- 成果物: `artifacts/research.md`

必要なら、Codex CLI の実ログを取得してイベント構造を実測する手順も追記できます。