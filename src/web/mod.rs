//! Web dashboard module for quedex.
//!
//! Provides a browser-based UI for monitoring task execution,
//! viewing logs, and managing runs.

pub mod handlers;
pub mod state;
pub mod watcher;
pub mod websocket;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use self::state::AppState;

/// Configuration for the web server.
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub store_root: PathBuf,
    pub run_id: Option<String>,
}

/// Build the application router.
pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Static assets (index.html served at root)
        .route("/", get(handlers::index_html))
        .route("/app.js", get(handlers::app_js))
        .route("/style.css", get(handlers::style_css))
        // API routes
        .route("/api/state", get(handlers::get_state))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/logs/{run_id}/{task_id}", get(handlers::get_logs))
        .route("/api/retry/{run_id}/{task_id}", post(handlers::retry_task))
        .route("/api/cancel/{run_id}/{task_id}", post(handlers::cancel_task))
        // WebSocket
        .route("/ws", get(websocket::ws_handler))
        .layer(cors)
        .with_state(state)
}

/// Start the web server with file watching.
pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let state = Arc::new(AppState::new(config.store_root.clone(), config.run_id));
    
    // Start file watcher
    watcher::start_watcher(Arc::clone(&state))?;
    
    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    println!("Starting web dashboard at http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
