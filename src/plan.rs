use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use petgraph::algo::is_cyclic_directed;
use petgraph::graphmap::DiGraphMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Research,
    Implement,
    Verify,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct WorktreeRunConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_dir: Option<PathBuf>,
    #[serde(default)]
    pub shallow_depth: Option<u32>,
}

/// Configuration for webhook notifications.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct NotificationConfig {
    /// Webhook URL (supports Slack/Discord incoming webhooks)
    pub url: String,
    /// Events to notify on. Valid values: "on_start", "on_task_complete", "on_complete", "on_failure"
    #[serde(default)]
    pub events: Vec<String>,
    /// Custom username for the notification (optional)
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct RunConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub worktree: Option<WorktreeRunConfig>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    #[serde(default)]
    pub default_timeout_sec: Option<u64>,
    /// Webhook notification configuration
    #[serde(default)]
    pub notifications: Option<NotificationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub version: u32,
    #[serde(default)]
    pub run: RunConfig,
    /// Template variables for prompt expansion.
    /// Use ${variable} to reference these in prompts.
    /// Use ${env.VAR} to reference environment variables.
    #[serde(default)]
    pub variables: HashMap<String, String>,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CodexConfig {
    pub prompt: String,
    #[serde(default)]
    pub output_last_message: Option<PathBuf>,
    #[serde(default = "default_verify_after")]
    #[schemars(default = "default_verify_after")]
    pub verify_after: bool,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub ask_for_approval: Option<String>,
    #[serde(default = "default_json")]
    #[schemars(default = "default_json")]
    pub json: bool,
}

fn default_json() -> bool {
    true
}

fn default_verify_after() -> bool {
    true
}

/// Condition for conditional task execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TaskCondition {
    /// Environment variable condition
    Env(EnvCondition),
    /// Previous task result condition
    TaskResult(TaskResultCondition),
}

/// Environment variable based condition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvCondition {
    /// Name of the environment variable to check
    pub env: String,
    /// Value that the environment variable must equal
    #[serde(default)]
    pub equals: Option<String>,
    /// Value that the environment variable must not equal
    #[serde(default)]
    pub not_equals: Option<String>,
    /// Whether the environment variable must exist (true) or must not exist (false)
    #[serde(default)]
    pub exists: Option<bool>,
}

/// Previous task result based condition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskResultCondition {
    /// Task ID to check the result of
    pub task: String,
    /// Expected status of the referenced task
    pub status: ConditionStatus,
}

/// Status values for task result conditions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClaudeCodeConfig {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_json")]
    #[schemars(default = "default_json")]
    pub json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpencodeConfig {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_json")]
    #[schemars(default = "default_json")]
    pub json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub mode: TaskMode,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub locks: Vec<String>,
    #[serde(default)]
    pub timeout_sec: Option<u64>,
    #[serde(default)]
    pub no_worktree: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub codex: Option<CodexConfig>,
    #[serde(default)]
    pub claude_code: Option<ClaudeCodeConfig>,
    #[serde(default)]
    pub opencode: Option<OpencodeConfig>,
    /// Number of retry attempts on failure (0 = no retry)
    #[serde(default)]
    pub retry_count: u32,
    /// Delay in seconds between retry attempts
    #[serde(default)]
    pub retry_delay_sec: u64,
    /// Condition for conditional execution. If the condition is not met, the task is skipped.
    #[serde(default)]
    pub condition: Option<TaskCondition>,
}

#[derive(Debug, Clone, Copy)]
pub enum PlanFormat {
    Json,
    Yaml,
}

impl Plan {
    pub fn parse_str(input: &str, format: PlanFormat) -> Result<Self> {
        match format {
            PlanFormat::Json => serde_json::from_str(input).context("parse plan json"),
            PlanFormat::Yaml => serde_yaml::from_str(input).context("parse plan yaml"),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported plan version {}", self.version);
        }
        if self.tasks.is_empty() {
            bail!("plan has no tasks");
        }

        let mut seen = HashSet::new();
        for task in &self.tasks {
            if task.id.trim().is_empty() {
                bail!("task id is empty");
            }
            // Validate task ID contains only safe characters
            if !task.id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                bail!(
                    "task id '{}' contains invalid characters (only alphanumeric, underscore, and hyphen are allowed)",
                    task.id
                );
            }
            if !seen.insert(task.id.clone()) {
                bail!("duplicate task id {}", task.id);
            }
            let runner_count = [task.codex.is_some(), task.claude_code.is_some(), task.opencode.is_some()]
                .iter()
                .filter(|&&x| x)
                .count();
            if runner_count > 1 {
                bail!(
                    "task {} has multiple runner configs (only one of codex, claude_code, or opencode allowed)",
                    task.id
                );
            }
            if runner_count == 0 {
                bail!("task {} missing runner config", task.id);
            }
            if let Some(kind) = task.kind.as_deref() {
                match kind {
                    "codex" => {
                        if task.codex.is_none() {
                            bail!("task {} kind=codex without codex config", task.id);
                        }
                    }
                    "claude_code" => {
                        if task.claude_code.is_none() {
                            bail!("task {} kind=claude_code without claude_code config", task.id);
                        }
                    }
                    "opencode" => {
                        if task.opencode.is_none() {
                            bail!("task {} kind=opencode without opencode config", task.id);
                        }
                    }
                    _ => bail!("task {} has unknown kind {}", task.id, kind),
                }
            }
            if let Some(codex) = task.codex.as_ref() {
                if codex.prompt.trim().is_empty() {
                    bail!("task {} codex.prompt is empty", task.id);
                }
                if codex.output_last_message.is_some() && task.mode != TaskMode::Research {
                    bail!(
                        "task {} output_last_message only allowed for research mode",
                        task.id
                    );
                }
            }
            if let Some(claude_code) = task.claude_code.as_ref() {
                if claude_code.prompt.trim().is_empty() {
                    bail!("task {} claude_code.prompt is empty", task.id);
                }
            }
            if let Some(opencode) = task.opencode.as_ref() {
                if opencode.prompt.trim().is_empty() {
                    bail!("task {} opencode.prompt is empty", task.id);
                }
            }
            for dep in &task.deps {
                if dep == &task.id {
                    bail!("task {} depends on itself", task.id);
                }
            }
        }

        let ids: HashSet<_> = self.tasks.iter().map(|task| task.id.as_str()).collect();
        for task in &self.tasks {
            for dep in &task.deps {
                if !ids.contains(dep.as_str()) {
                    bail!("task {} has missing dep {}", task.id, dep);
                }
            }
            // Validate condition references
            if let Some(TaskCondition::TaskResult(cond)) = &task.condition {
                if !ids.contains(cond.task.as_str()) {
                    bail!(
                        "task {} condition references non-existent task {}",
                        task.id,
                        cond.task
                    );
                }
                if cond.task == task.id {
                    bail!(
                        "task {} condition cannot reference itself",
                        task.id
                    );
                }
            }
        }

        let mut graph = DiGraphMap::<&str, ()>::new();
        for task in &self.tasks {
            graph.add_node(task.id.as_str());
        }
        for task in &self.tasks {
            for dep in &task.deps {
                graph.add_edge(dep.as_str(), task.id.as_str(), ());
            }
        }
        if is_cyclic_directed(&graph) {
            bail!("task dependency cycle detected");
        }

        Ok(())
    }
}

/// Generate JSON Schema for Plan
pub fn plan_json_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Plan)
}
