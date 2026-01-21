//! File watcher for monitoring state changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use tokio::sync::mpsc;

use super::state::{AppState, StateUpdate};

/// Start watching the store directory for state changes.
pub fn start_watcher(state: Arc<AppState>) -> anyhow::Result<()> {
    let store_root = state.store_root.clone();
    let tx = state.tx.clone();
    let run_id_filter = state.run_id.clone();

    // Use async channel for file events
    let (file_tx, mut file_rx) = mpsc::channel::<PathBuf>(100);

    // Create synchronous watcher
    let file_tx_clone = file_tx.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    for path in event.paths {
                        if path.file_name().map(|n| n == "state.json").unwrap_or(false) {
                            let _ = file_tx_clone.blocking_send(path);
                        }
                    }
                }
            }
        },
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )?;

    // Watch the runs directory
    let runs_dir = store_root.join("runs");
    if runs_dir.exists() {
        watcher.watch(&runs_dir, RecursiveMode::Recursive)?;
    }

    // Spawn task to process file events
    tokio::spawn(async move {
        // Keep watcher alive
        let _watcher = watcher;

        while let Some(path) = file_rx.recv().await {
            // Extract run_id from path
            if let Some(run_id) = extract_run_id(&path, &store_root) {
                // Skip if we're filtering for a specific run
                if let Some(ref filter) = run_id_filter {
                    if &run_id != filter {
                        continue;
                    }
                }

                // Read the state file
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let update = StateUpdate {
                        run_id: run_id.clone(),
                        state_json: content,
                    };
                    let _ = tx.send(update);
                }
            }
        }
    });

    Ok(())
}

/// Extract run_id from a state.json path.
fn extract_run_id(path: &Path, store_root: &Path) -> Option<String> {
    // Path format: {store_root}/runs/{run_id}/state.json
    let runs_dir = store_root.join("runs");
    let relative = path.strip_prefix(&runs_dir).ok()?;
    let components: Vec<_> = relative.components().collect();
    if components.len() >= 2 {
        components[0].as_os_str().to_str().map(String::from)
    } else {
        None
    }
}
