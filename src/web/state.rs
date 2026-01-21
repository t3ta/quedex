//! Shared application state for the web server.

use std::path::PathBuf;
use tokio::sync::broadcast;

/// Shared state for the web application.
pub struct AppState {
    /// Store root directory.
    pub store_root: PathBuf,
    /// Optional specific run ID to monitor.
    pub run_id: Option<String>,
    /// Broadcast channel for state updates.
    pub tx: broadcast::Sender<StateUpdate>,
}

/// State update message for WebSocket clients.
#[derive(Debug, Clone)]
pub struct StateUpdate {
    pub run_id: String,
    pub state_json: String,
}

impl AppState {
    /// Create new application state.
    pub fn new(store_root: PathBuf, run_id: Option<String>) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            store_root,
            run_id,
            tx,
        }
    }

    /// Subscribe to state updates.
    pub fn subscribe(&self) -> broadcast::Receiver<StateUpdate> {
        self.tx.subscribe()
    }
}
