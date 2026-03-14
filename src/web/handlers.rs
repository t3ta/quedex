//! HTTP request handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse},
};
use serde::Serialize;
use serde_json::json;

use super::state::AppState;

/// Serve index.html
pub async fn index_html() -> Html<&'static str> {
    Html(include_str!("assets/index.html"))
}

/// Serve app.js
pub async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("assets/app.js"),
    )
}

/// Serve style.css
pub async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css")],
        include_str!("assets/style.css"),
    )
}

/// API response wrapper
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }

    pub fn err(msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        })
    }
}

/// Get current state for all runs or a specific run.
pub async fn get_state(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runs_dir = state.store_root.join("runs");

    if !runs_dir.exists() {
        return Ok(Json(json!({ "runs": [] })));
    }

    let mut runs = Vec::new();

    if let Some(ref run_id) = state.run_id {
        // Single run mode
        let state_path = runs_dir.join(run_id).join("state.json");
        if state_path.exists()
            && let Ok(content) = std::fs::read_to_string(&state_path)
            && let Ok(run_state) = serde_json::from_str::<serde_json::Value>(&content)
        {
            runs.push(run_state);
        }
    } else {
        // All runs mode
        if let Ok(entries) = std::fs::read_dir(&runs_dir) {
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
    }

    // Sort by started_at descending
    runs.sort_by(|a, b| {
        let a_time = a.get("started_at").and_then(|v| v.as_str()).unwrap_or("");
        let b_time = b.get("started_at").and_then(|v| v.as_str()).unwrap_or("");
        b_time.cmp(a_time)
    });

    Ok(Json(json!({ "runs": runs })))
}

/// List all run IDs.
pub async fn list_runs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runs_dir = state.store_root.join("runs");

    if !runs_dir.exists() {
        return Ok(Json(json!({ "runs": [] })));
    }

    let mut run_ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && let Some(name) = entry.file_name().to_str()
            {
                run_ids.push(name.to_string());
            }
        }
    }

    Ok(Json(json!({ "runs": run_ids })))
}

/// Get logs for a task.
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Path((run_id, task_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task_dir = state
        .store_root
        .join("runs")
        .join(&run_id)
        .join("tasks")
        .join(&task_id);

    let stdout = std::fs::read_to_string(task_dir.join("stdout.log")).unwrap_or_default();
    let stderr = std::fs::read_to_string(task_dir.join("stderr.log")).unwrap_or_default();

    Ok(Json(json!({
        "run_id": run_id,
        "task_id": task_id,
        "stdout": stdout,
        "stderr": stderr,
    })))
}

/// Retry a failed task.
pub async fn retry_task(
    State(_state): State<Arc<AppState>>,
    Path((run_id, task_id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    // For now, return a placeholder response
    // Full implementation would trigger the retry logic
    Json(json!({
        "success": true,
        "message": format!("Retry requested for task {} in run {}", task_id, run_id),
    }))
}

/// Cancel a running task.
pub async fn cancel_task(
    State(_state): State<Arc<AppState>>,
    Path((run_id, task_id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    // For now, return a placeholder response
    // Full implementation would trigger the cancel logic
    Json(json!({
        "success": true,
        "message": format!("Cancel requested for task {} in run {}", task_id, run_id),
    }))
}
