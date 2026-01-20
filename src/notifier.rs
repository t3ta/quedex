//! Webhook notification module for sending Slack/Discord compatible notifications.

use std::sync::Arc;

use serde::Serialize;

use crate::plan::NotificationConfig;
use crate::store::TaskStatus;

/// Event types that can trigger notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationEvent {
    /// Run started
    OnStart,
    /// Individual task completed (success or failure)
    OnTaskComplete,
    /// All tasks completed successfully
    OnComplete,
    /// Run failed (at least one task failed)
    OnFailure,
}

impl NotificationEvent {
    fn as_str(&self) -> &'static str {
        match self {
            NotificationEvent::OnStart => "on_start",
            NotificationEvent::OnTaskComplete => "on_task_complete",
            NotificationEvent::OnComplete => "on_complete",
            NotificationEvent::OnFailure => "on_failure",
        }
    }
}

/// Slack/Discord compatible webhook payload.
#[derive(Debug, Serialize)]
struct WebhookPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>, // Discord
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>, // Slack
}

/// Notifier for sending webhook notifications asynchronously.
#[derive(Clone)]
pub struct Notifier {
    config: Arc<NotificationConfig>,
    client: reqwest::Client,
    run_id: String,
    run_name: String,
}

impl Notifier {
    /// Creates a new Notifier if notification config is provided.
    pub fn new(
        config: Option<NotificationConfig>,
        run_id: String,
        run_name: String,
    ) -> Option<Self> {
        let config = config?;
        if config.url.is_empty() {
            return None;
        }
        Some(Self {
            config: Arc::new(config),
            client: reqwest::Client::new(),
            run_id,
            run_name,
        })
    }

    /// Checks if the given event type is enabled in the configuration.
    fn is_event_enabled(&self, event: NotificationEvent) -> bool {
        if self.config.events.is_empty() {
            // Default: enable all events
            return true;
        }
        self.config.events.iter().any(|e| e == event.as_str())
    }

    /// Sends a notification asynchronously (fire-and-forget).
    /// This method spawns a tokio task and does not block.
    pub fn send(&self, event: NotificationEvent, message: &str) {
        if !self.is_event_enabled(event) {
            return;
        }

        let url = self.config.url.clone();
        let username = self.config.username.clone();
        let client = self.client.clone();
        let message = message.to_string();

        tokio::spawn(async move {
            let payload = WebhookPayload {
                username,
                content: Some(message.clone()), // Discord
                text: Some(message),            // Slack
            };

            if let Err(e) = client.post(&url).json(&payload).send().await {
                eprintln!("notification error: {e}");
            }
        });
    }

    /// Notifies that a run has started.
    pub fn notify_start(&self) {
        let message = format!("🚀 **quedex run started**\nRun: `{}`\nID: `{}`", self.run_name, self.run_id);
        self.send(NotificationEvent::OnStart, &message);
    }

    /// Notifies that a task has completed.
    pub fn notify_task_complete(&self, task_id: &str, status: TaskStatus, exit_code: Option<i32>) {
        let status_emoji = match status {
            TaskStatus::Succeeded => "✅",
            TaskStatus::Failed => "❌",
            TaskStatus::Canceled => "🚫",
            TaskStatus::Skipped => "⏭️",
            _ => "❓",
        };
        let exit_info = exit_code
            .map(|c| format!(" (exit code: {c})"))
            .unwrap_or_default();
        let message = format!(
            "{} **Task completed**\nRun: `{}`\nTask: `{}`\nStatus: {:?}{}",
            status_emoji, self.run_name, task_id, status, exit_info
        );
        self.send(NotificationEvent::OnTaskComplete, &message);
    }

    /// Notifies that the run has completed successfully.
    pub fn notify_complete(&self, total_tasks: usize, succeeded: usize) {
        let message = format!(
            "✅ **quedex run completed**\nRun: `{}`\nID: `{}`\nTasks: {}/{} succeeded",
            self.run_name, self.run_id, succeeded, total_tasks
        );
        self.send(NotificationEvent::OnComplete, &message);
    }

    /// Notifies that the run has failed.
    pub fn notify_failure(&self, total_tasks: usize, failed: usize, succeeded: usize) {
        let message = format!(
            "❌ **quedex run failed**\nRun: `{}`\nID: `{}`\nFailed: {}, Succeeded: {}, Total: {}",
            self.run_name, self.run_id, failed, succeeded, total_tasks
        );
        self.send(NotificationEvent::OnFailure, &message);
    }
}
