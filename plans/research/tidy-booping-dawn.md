# 実装計画: Issue #26 (Web UI)

## 実装順序

| 順序 | Issue | 理由 |
|------|-------|------|
| 1 | **#26 Web UI** | 依存関係追加が必要、実装規模が大きい |

---

## Issue #26: Web UI / ダッシュボード

### 概要
- `quedex serve --port 8080` コマンド
- TUIと同等の機能をブラウザで提供
- WebSocketでリアルタイム更新
- retry/cancel操作

### アーキテクチャ

```
quedex serve --port 8080
      │
      ▼
┌─────────────────────────────────────┐
│           axum Web Server           │
├─────────────────────────────────────┤
│  HTTP Routes          WebSocket     │
│  ├─ GET /             ├─ /ws        │
│  ├─ GET /api/state                  │
│  ├─ GET /api/logs/:task_id          │
│  ├─ POST /api/retry/:task_id        │
│  └─ POST /api/cancel/:task_id       │
└─────────────────────────────────────┘
```

### 実装ステップ

#### Step 1: 依存関係の追加
**ファイル:** `Cargo.toml`

```toml
axum = "0.7"
tokio-tungstenite = "0.24"
tower-http = { version = "0.5", features = ["fs", "cors"] }
```

#### Step 2: CLIコマンド追加
**ファイル:** `src/cli.rs`

```rust
Serve {
    run_id: Option<String>,
    #[arg(short, long, default_value = "8080")]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}
```

#### Step 3: Webモジュール構造
**ディレクトリ:** `src/web/`

```
src/web/
├── mod.rs           # router構築
├── handlers.rs      # HTTPハンドラ
├── websocket.rs     # WebSocket処理
└── assets/          # 静的ファイル
    ├── index.html
    ├── app.js
    └── style.css
```

#### Step 4: WebSocket状態配信
- ファイル監視（notify）で state.json 変更を検知
- broadcast channelで全クライアントに配信

#### Step 5: フロントエンド
- Vanilla JS + WebSocket
- タスク一覧表示、ログ表示
- retry/cancelボタン

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `Cargo.toml` | axum, tokio-tungstenite, tower-http追加 |
| `src/cli.rs` | `Commands::Serve`追加 |
| `src/lib.rs` | `pub mod web;`追加 |
| `src/main.rs` | `handle_serve()`実装 |
| `src/web/` (新規) | Webモジュール全体 |

### テスト
- APIハンドラのレスポンス検証
- WebSocket接続テスト
- ブラウザでの手動確認

---

## 検証方法

### Issue #26
```bash
# 1. サーバー起動
cargo run -- serve --port 8080

# 2. 別ターミナルでrun実行
cargo run -- start plan.json

# 3. ブラウザで http://localhost:8080 を確認
# - タスク状態のリアルタイム更新
# - ログ表示
# - retry/cancel操作
```
