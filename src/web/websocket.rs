//! WebSocket handler for real-time state updates.

use std::sync::Arc;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};

use super::state::AppState;

/// WebSocket upgrade handler.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle a WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to state updates
    let mut rx = state.subscribe();

    // Send initial state
    if let Ok(initial_state) = get_current_state(&state).await {
        let _ = sender.send(Message::Text(initial_state.into())).await;
    }

    // Spawn task to forward broadcast messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Ok(update) = rx.recv().await {
            if sender
                .send(Message::Text(update.state_json.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Handle incoming messages (ping/pong, close)
    let recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_data)) => {
                    // Pong is handled automatically by axum
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}

/// Get current state as JSON string.
async fn get_current_state(state: &AppState) -> Result<String, ()> {
    let runs_dir = state.store_root.join("runs");

    if !runs_dir.exists() {
        return Ok(r#"{"runs":[]}"#.to_string());
    }

    let mut runs = Vec::new();

    if let Some(ref run_id) = state.run_id {
        let state_path = runs_dir.join(run_id).join("state.json");
        if state_path.exists()
            && let Ok(content) = std::fs::read_to_string(&state_path)
            && let Ok(run_state) = serde_json::from_str::<serde_json::Value>(&content)
        {
            runs.push(run_state);
        }
    } else if let Ok(entries) = std::fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let state_path = entry.path().join("state.json");
                if state_path.exists()
                    && let Ok(content) = std::fs::read_to_string(&state_path)
                    && let Ok(run_state) = serde_json::from_str::<serde_json::Value>(&content)
                {
                    runs.push(run_state);
                }
            }
        }
    }

    Ok(serde_json::json!({ "runs": runs }).to_string())
}
