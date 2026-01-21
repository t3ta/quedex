# 実装計画: Issue #26 (Web UI) と Issue #27 (動的タイムアウト)

## 実装順序

| 順序 | Issue | 理由 |
|------|-------|------|
| 1 | **#27 動的タイムアウト** | 変更範囲が限定的、新規依存なし |
| 2 | **#26 Web UI** | 依存関係追加が必要、実装規模が大きい |

---

## Issue #27: タスクタイムアウトの動的調整

### 概要
- `"timeout_sec": "auto"` - 過去の平均 + 2σ（標準偏差）
- `"timeout_sec": "2x_average"` - 過去の平均の2倍
- 履歴がない場合はデフォルト値を使用

### 実装ステップ

#### Step 1: TimeoutConfig enum の定義
**ファイル:** `src/plan.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TimeoutConfig {
    Fixed(u64),
    Dynamic(DynamicTimeout),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DynamicTimeout {
    Auto,
    #[serde(rename = "2x_average")]
    TwoXAverage,
    Multiplier(f64),
}
```

#### Step 2: 統計計算モジュールの追加
**ファイル:** `src/stats.rs` (新規)

- `TaskStats` 構造体（平均、標準偏差計算）
- `collect_task_stats()` - store_rootから全runの実行時間を収集
- `auto_timeout()` - 平均 + 2σ
- `multiplied_timeout()` - 平均 × 倍率

#### Step 3: タイムアウト解決ロジック
**ファイル:** `src/main.rs`

- `PlanTaskRunner`に`task_stats`フィールド追加
- `resolve_timeout()`メソッドで動的計算

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `src/plan.rs` | `TimeoutConfig` enum追加、`Task.timeout_sec`の型変更 |
| `src/stats.rs` (新規) | 統計計算ロジック |
| `src/lib.rs` | `pub mod stats;` 追加 |
| `src/main.rs` | `resolve_timeout()`実装 |

### テスト
- `TimeoutConfig`のシリアライズ/デシリアライズ
- 統計計算の正確性
- 履歴なし時のフォールバック

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

### Issue #27
```bash
# 1. 既存のrunがある状態でautoタイムアウトをテスト
cat > test-plan.json << 'EOF'
{
  "version": 1,
  "tasks": [
    { "id": "test", "timeout_sec": "auto", "codex": { "prompt": "echo test" } }
  ]
}
EOF
cargo run -- dry-run test-plan.json
cargo run -- run test-plan.json
```

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
