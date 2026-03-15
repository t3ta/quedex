use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use petgraph::algo::is_cyclic_directed;
use petgraph::graphmap::DiGraphMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Research,
    #[default]
    Implement,
    Verify,
}

impl fmt::Display for TaskMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match self {
            TaskMode::Research => "research",
            TaskMode::Implement => "implement",
            TaskMode::Verify => "verify",
        };
        f.write_str(mode)
    }
}

/// Configuration for git worktree isolation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct WorktreeRunConfig {
    /// Enable worktree isolation for parallel task execution.
    #[serde(default)]
    pub enabled: bool,
    /// Base directory for worktrees (default: `.quedex/worktrees`).
    #[serde(default)]
    pub base_dir: Option<PathBuf>,
    /// DEPRECATED: This field has no effect.
    /// Git worktree does not support shallow clone (--depth option).
    /// Shallow clone is only available with `git clone`, not `git worktree add`.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    /// Per-mode concurrency limits. Keys are mode names ("research", "implement", "verify").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency_by_mode: Option<HashMap<String, usize>>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    /// Default stall timeout in seconds for all tasks.
    /// If a task produces no output for this duration, it is killed.
    /// Set to 0 to disable. Individual tasks can override with their own stall_timeout_sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_sec: Option<u64>,
    #[serde(
        rename = "default_timeout_sec",
        default,
        deserialize_with = "reject_default_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _default_timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    #[serde(
        rename = "timeout_sec",
        default,
        deserialize_with = "reject_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    /// Webhook notification configuration
    #[serde(default)]
    pub notifications: Option<NotificationConfig>,
    /// System prompt to prepend to all task prompts (overrides quedex.toml)
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Default completion gates applied to implement and verify mode tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_gates: Option<Vec<CompletionGate>>,
}

/// Agent profile for role-based task specialization.
/// Profiles define system_prompt and model overrides that can be shared across tasks.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentProfile {
    /// System prompt for this profile (merged with run-level system_prompt)
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Model override for this profile
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub version: u32,
    #[serde(default)]
    pub run: RunConfig,
    /// Agent role profiles for task specialization.
    /// Maps profile name to AgentProfile configuration.
    #[serde(default)]
    pub profiles: HashMap<String, AgentProfile>,
    /// Task groups for logical organization.
    /// Maps group name to list of task IDs belonging to that group.
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
    pub tasks: Vec<Task>,
    #[serde(
        rename = "timeout_sec",
        default,
        deserialize_with = "reject_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    #[serde(
        rename = "default_timeout_sec",
        default,
        deserialize_with = "reject_default_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _default_timeout_sec_rejected: Option<serde::de::IgnoredAny>,
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

fn reject_timeout_sec<'de, D>(deserializer: D) -> Result<Option<serde::de::IgnoredAny>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "timeout_sec は削除済みです。Codex CLI のタイムアウト設定を使用してください。",
    ))
}

fn reject_default_timeout_sec<'de, D>(
    deserializer: D,
) -> Result<Option<serde::de::IgnoredAny>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "default_timeout_sec は削除済みです。Codex CLI のタイムアウト設定を使用してください。",
    ))
}

/// Configuration for shared context between tasks.
/// Allows tasks to publish data to a key-value store and inject upstream context into prompts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextConfig {
    /// Publish context after task completion
    #[serde(default)]
    pub publish: Option<PublishConfig>,
    /// Inject context from upstream tasks before task starts
    #[serde(default)]
    pub inject: Option<Vec<InjectConfig>>,
}

/// Configuration for publishing context data after task completion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PublishConfig {
    /// Key to publish the context under
    pub key: String,
    /// Source file to read the context from (relative path)
    pub source: String,
}

/// Configuration for injecting context data into a task's prompt.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InjectConfig {
    /// Key of the context to inject (published by an upstream task)
    pub from: String,
    /// Label to display in the injected context section
    #[serde(default)]
    pub r#as: Option<String>,
}

/// Classification of failure types for retry decision making.
///
/// This helps determine whether a failure is worth retrying:
/// - Transient failures (network issues, rate limits) should be retried
/// - Permanent failures (invalid config, missing files) should not be retried
/// - Unknown failures are retried by default
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureType {
    /// Temporary failure that may succeed on retry (e.g., network timeout, rate limit)
    Transient,
    /// Permanent failure that will not succeed on retry (e.g., invalid config, auth failure)
    Permanent,
    /// Unknown failure type - retry by default
    #[default]
    Unknown,
}

impl FailureType {
    /// Classify a failure based on exit code using common Unix conventions.
    ///
    /// Exit code heuristics:
    /// - 0: Success (shouldn't be called for failures)
    /// - 1: General error - Unknown
    /// - 2: Misuse of shell command - Permanent
    /// - 126: Permission problem - Permanent
    /// - 127: Command not found - Permanent
    /// - 128+: Signal-based termination (128+N where N is signal number) - Transient
    ///   - 130 (SIGINT): User interrupt - Permanent
    ///   - 137 (SIGKILL): Force killed (OOM, etc.) - Transient
    ///   - 143 (SIGTERM): Graceful termination - Transient
    pub fn from_exit_code(exit_code: Option<i32>) -> Self {
        match exit_code {
            None => FailureType::Unknown,        // Process didn't exit normally
            Some(0) => FailureType::Unknown,     // Success, shouldn't happen
            Some(1) => FailureType::Unknown,     // Generic error
            Some(2) => FailureType::Permanent,   // Misuse of command
            Some(126) => FailureType::Permanent, // Permission denied
            Some(127) => FailureType::Permanent, // Command not found
            Some(128) => FailureType::Unknown,   // Invalid exit argument
            Some(130) => FailureType::Permanent, // SIGINT (Ctrl+C)
            Some(137) => FailureType::Transient, // SIGKILL (OOM or force kill)
            Some(143) => FailureType::Transient, // SIGTERM (graceful shutdown)
            Some(code) if code > 128 => FailureType::Transient, // Other signals
            Some(_) => FailureType::Unknown,
        }
    }

    /// Classify a failure based on common error patterns in stderr.
    ///
    /// Pattern matching for common transient vs permanent errors.
    pub fn from_stderr_patterns(stderr: &str) -> Self {
        let stderr_lower = stderr.to_lowercase();

        // Permanent failure patterns
        let permanent_patterns = [
            "permission denied",
            "access denied",
            "authentication failed",
            "invalid credentials",
            "unauthorized",
            "forbidden",
            "not found",
            "does not exist",
            "no such file or directory",
            "invalid configuration",
            "syntax error",
            "parse error",
            "compilation failed",
            "type error",
            "undefined reference",
        ];

        // Transient failure patterns
        let transient_patterns = [
            "timeout",
            "timed out",
            "connection refused",
            "connection reset",
            "connection closed",
            "network unreachable",
            "host unreachable",
            "temporary failure",
            "too many requests",
            "rate limit",
            "quota exceeded",
            "service unavailable",
            "503",
            "502",
            "504",
            "out of memory",
            "oom",
            "killed",
            "retry",
            "try again",
            "temporarily unavailable",
        ];

        for pattern in permanent_patterns {
            if stderr_lower.contains(pattern) {
                return FailureType::Permanent;
            }
        }

        for pattern in transient_patterns {
            if stderr_lower.contains(pattern) {
                return FailureType::Transient;
            }
        }

        FailureType::Unknown
    }

    /// Combine exit code and stderr analysis to classify failure.
    ///
    /// Stderr patterns take precedence over exit codes for more accurate classification.
    pub fn classify(exit_code: Option<i32>, stderr: &str) -> Self {
        // First check stderr patterns (more specific)
        let from_stderr = Self::from_stderr_patterns(stderr);
        if from_stderr != FailureType::Unknown {
            return from_stderr;
        }

        // Fall back to exit code classification
        Self::from_exit_code(exit_code)
    }

    /// Returns true if this failure type should be retried.
    pub fn should_retry(&self) -> bool {
        match self {
            FailureType::Transient => true,
            FailureType::Permanent => false,
            FailureType::Unknown => true, // Retry by default for unknown failures
        }
    }
}

/// A completion gate that runs after a task exits successfully.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompletionGate {
    /// Human-readable name for this gate
    pub name: String,
    /// Command to execute (run via sh -c)
    pub command: String,
    /// Timeout in seconds for this gate (default: 300)
    #[serde(default = "default_gate_timeout_sec")]
    #[schemars(default = "default_gate_timeout_sec")]
    pub timeout_sec: u64,
}

fn default_gate_timeout_sec() -> u64 {
    300
}

/// Type of backoff strategy for retry delays.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackoffType {
    /// Fixed delay between retries (default)
    #[default]
    Fixed,
    /// Exponential backoff: delay = base * 2^(attempt-1)
    Exponential,
    /// Linear backoff: delay = base * attempt
    Linear,
}

/// Strategy for adaptive retry behavior.
/// When configured, retry attempts can inject error context from previous failures
/// and optionally escalate to a more capable model.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetryStrategy {
    /// Inject stderr from the previous failed attempt into the retry prompt
    #[serde(default)]
    pub inject_error_context: bool,
    /// Model to escalate to on retry (e.g., "opus" for a more capable model)
    #[serde(default)]
    pub escalate_model: Option<String>,
    /// Maximum number of stderr lines to include in error context injection (default: 50)
    #[serde(default = "default_max_stderr_lines")]
    #[schemars(default = "default_max_stderr_lines")]
    pub max_stderr_lines: usize,
    /// Type of backoff strategy for retry delays (default: fixed)
    #[serde(default)]
    pub backoff_type: BackoffType,
    /// Maximum delay in seconds (caps exponential/linear growth). Default: 300 (5 minutes)
    #[serde(default = "default_max_delay_sec")]
    #[schemars(default = "default_max_delay_sec")]
    pub max_delay_sec: u64,
    /// Jitter percentage (0-100) to add randomness to delays for thundering herd prevention.
    /// Default: 0 (no jitter)
    #[serde(default)]
    pub jitter_percent: u8,
    /// Skip retries for failures classified as permanent (e.g., auth errors, missing files).
    /// When enabled, uses exit code and stderr pattern analysis to identify permanent failures.
    /// Default: false (retry all failures)
    #[serde(default)]
    pub skip_permanent_failures: bool,
}

fn default_max_stderr_lines() -> usize {
    50
}

fn default_max_delay_sec() -> u64 {
    300 // 5 minutes
}

impl RetryStrategy {
    /// Calculate the delay for a retry attempt with backoff and jitter.
    ///
    /// # Arguments
    /// * `base_delay_sec` - Base delay in seconds (from Task.retry_delay_sec)
    /// * `attempt` - Current retry attempt number (1-indexed)
    ///
    /// # Returns
    /// The calculated delay in seconds with backoff and optional jitter applied.
    pub fn calculate_delay(&self, base_delay_sec: u64, attempt: u32) -> u64 {
        if base_delay_sec == 0 {
            return 0;
        }

        // Calculate base delay based on backoff type
        let delay = match self.backoff_type {
            BackoffType::Fixed => base_delay_sec,
            BackoffType::Exponential => {
                // delay = base * 2^(attempt-1), capped at max_delay_sec
                let exponent = attempt.saturating_sub(1);
                base_delay_sec.saturating_mul(2u64.saturating_pow(exponent))
            }
            BackoffType::Linear => {
                // delay = base * attempt, capped at max_delay_sec
                base_delay_sec.saturating_mul(attempt as u64)
            }
        };

        // Cap at max_delay_sec
        let delay = delay.min(self.max_delay_sec);

        // Apply jitter if configured
        if self.jitter_percent > 0 && delay > 0 {
            let jitter_percent = self.jitter_percent.min(100) as u64;
            let jitter_range = delay.saturating_mul(jitter_percent) / 100;
            if jitter_range > 0 {
                // Generate random jitter using simple thread-local RNG
                use std::collections::hash_map::RandomState;
                use std::hash::{BuildHasher, Hasher};
                let random = RandomState::new().build_hasher().finish();
                // Compute modulus safely: range is [0, 2*jitter_range]
                let modulus = jitter_range.saturating_mul(2).saturating_add(1);
                let random_offset = random % modulus;
                // Apply jitter: delay + random_offset - jitter_range
                // Ensure delay doesn't go below 1 second
                let jittered = delay
                    .saturating_add(random_offset)
                    .saturating_sub(jitter_range);
                return jittered.max(1);
            }
        }

        delay
    }
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

/// Per-task lifecycle hooks configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TaskHooksConfig {
    /// Command to run before this task starts
    pub before_task: Option<String>,
    /// Command to run after this task completes
    pub after_task: Option<String>,
    /// Command to run when this task fails
    pub on_failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub mode: TaskMode,
    /// Optional agent profile for role-based specialization.
    /// If specified, must match a key in Plan.profiles.
    #[serde(default)]
    pub profile: Option<String>,
    /// Optional group this task belongs to.
    /// If specified, should match a key in Plan.groups.
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub locks: Vec<String>,
    #[serde(
        rename = "timeout_sec",
        default,
        deserialize_with = "reject_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    #[serde(
        rename = "default_timeout_sec",
        default,
        deserialize_with = "reject_default_timeout_sec",
        skip_serializing
    )]
    #[schemars(skip)]
    pub _default_timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub no_worktree: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_files: Option<Vec<String>>,
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
    /// Adaptive retry strategy for smarter retry behavior
    #[serde(default)]
    pub retry_strategy: Option<RetryStrategy>,
    /// Shared context configuration for publishing and injecting data between tasks
    #[serde(default)]
    pub context: Option<ContextConfig>,
    /// Condition for conditional execution. If the condition is not met, the task is skipped.
    #[serde(default)]
    pub condition: Option<TaskCondition>,
    /// Stall timeout in seconds for this task.
    /// If the task produces no output for this duration, it is killed.
    /// Set to 0 to disable. None falls back to run.stall_timeout_sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_sec: Option<u64>,
    /// Completion gates to run after this task exits successfully.
    /// Overrides default_gates if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_gates: Option<Vec<CompletionGate>>,
    /// Skip completion gates for this task.
    #[serde(default)]
    pub skip_gates: bool,
    /// Whether to create a git commit after this task succeeds (default: true)
    /// Only applicable for Implement and Verify modes, ignored for Research mode
    #[serde(default = "default_auto_commit")]
    #[schemars(default = "default_auto_commit")]
    pub auto_commit: bool,
    /// Whether this task should squash all previous commits into one
    /// Used for final integration/review tasks
    #[serde(default)]
    pub squash: bool,
    /// Per-task lifecycle hooks (overrides run-level hooks)
    #[serde(default)]
    pub hooks: Option<TaskHooksConfig>,
}

fn default_auto_commit() -> bool {
    true
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

        // Validate cwd is absolute path if specified
        if let Some(cwd) = self.run.cwd.as_ref() {
            if cwd.is_relative() {
                bail!("run.cwd must be an absolute path, got: {}", cwd.display());
            }
        }

        if let Some(max_concurrency_by_mode) = self.run.max_concurrency_by_mode.as_ref() {
            for (mode, limit) in max_concurrency_by_mode {
                if *limit == 0 {
                    bail!("run.max_concurrency_by_mode.{mode} must be greater than 0");
                }
                match mode.as_str() {
                    "research" | "implement" | "verify" => {}
                    _ => bail!(
                        "run.max_concurrency_by_mode contains unknown mode '{mode}' (expected one of: research, implement, verify)"
                    ),
                }
            }
        }

        // Warn if env block is explicitly set but empty (likely a mistake)
        if let Some(env) = &self.run.env {
            if env.is_empty() {
                eprintln!(
                    "warning: run.env is empty; if you don't need custom env vars, remove the env block entirely"
                );
            }
        }

        if let Some(gates) = self.run.default_gates.as_ref() {
            validate_completion_gates("run.default_gates", gates)?;
        }

        let mut seen = HashSet::new();
        for task in &self.tasks {
            if task.id.trim().is_empty() {
                bail!("task id is empty");
            }
            // Validate task ID contains only safe characters
            if !task
                .id
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                bail!(
                    "task id '{}' contains invalid characters (only alphanumeric, underscore, and hyphen are allowed)",
                    task.id
                );
            }
            if !seen.insert(task.id.clone()) {
                bail!("duplicate task id {}", task.id);
            }
            let runner_count = [
                task.codex.is_some(),
                task.claude_code.is_some(),
                task.opencode.is_some(),
            ]
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
                            bail!(
                                "task {} kind=claude_code without claude_code config",
                                task.id
                            );
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
            if let Some(output_files) = task.output_files.as_ref() {
                if output_files.is_empty() {
                    bail!("task {} output_files is empty", task.id);
                }
                for path in output_files {
                    if path.trim().is_empty() {
                        bail!("task {} output_files contains empty path", task.id);
                    }
                    // Reject absolute paths
                    if path.starts_with('/') || path.starts_with('\\') {
                        bail!(
                            "task {} output_files contains absolute path: {}",
                            task.id,
                            path
                        );
                    }
                    // Reject parent directory references
                    if path.contains("..") {
                        bail!("task {} output_files contains '..': {}", task.id, path);
                    }
                }
            }
            if let Some(gates) = task.completion_gates.as_ref() {
                validate_completion_gates(&format!("task {} completion_gates", task.id), gates)?;
            }
            // Validate context.publish.source path
            if let Some(ref ctx) = task.context {
                if let Some(ref publish) = ctx.publish {
                    let path = &publish.source;
                    if path.trim().is_empty() {
                        bail!("task {} context.publish.source is empty", task.id);
                    }
                    if path.starts_with('/') || path.starts_with('\\') {
                        bail!(
                            "task {} context.publish.source contains absolute path: {}",
                            task.id,
                            path
                        );
                    }
                    if path.contains("..") {
                        bail!(
                            "task {} context.publish.source contains '..': {}",
                            task.id,
                            path
                        );
                    }
                    // Validate publish key contains only safe characters
                    if !publish
                        .key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                    {
                        bail!(
                            "task {} context.publish.key '{}' contains invalid characters",
                            task.id,
                            publish.key
                        );
                    }
                }
                // Validate inject keys contain only safe characters
                if let Some(ref injections) = ctx.inject {
                    for inject in injections {
                        if !inject
                            .from
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        {
                            bail!(
                                "task {} context.inject.from '{}' contains invalid characters",
                                task.id,
                                inject.from
                            );
                        }
                    }
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

        // Validate profile references
        for task in &self.tasks {
            if let Some(ref profile_name) = task.profile {
                if !self.profiles.contains_key(profile_name) {
                    bail!(
                        "task {} references non-existent profile '{}'",
                        task.id,
                        profile_name
                    );
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
                    bail!("task {} condition cannot reference itself", task.id);
                }
                // Condition-referenced task must be declared as a dependency
                if !task.deps.contains(&cond.task) {
                    bail!(
                        "task {} condition references task {} but does not declare it as a dependency",
                        task.id,
                        cond.task
                    );
                }
            }
        }

        // Validate group names contain only safe characters
        for group_name in self.groups.keys() {
            if !group_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                bail!(
                    "group name '{group_name}' contains invalid characters (only alphanumeric, underscore, and hyphen allowed)"
                );
            }
        }

        // Validate groups reference existing tasks
        for (group_name, task_list) in &self.groups {
            for task_id in task_list {
                if !ids.contains(task_id.as_str()) {
                    bail!("group '{group_name}' references non-existent task '{task_id}'");
                }
            }
        }

        // Validate no task is in multiple groups (via Plan.groups)
        let mut task_to_groups: HashMap<&str, Vec<&str>> = HashMap::new();
        for (group_name, task_list) in &self.groups {
            for task_id in task_list {
                task_to_groups
                    .entry(task_id.as_str())
                    .or_default()
                    .push(group_name.as_str());
            }
        }
        for (task_id, groups) in &task_to_groups {
            if groups.len() > 1 {
                bail!("task '{task_id}' is listed in multiple groups: {groups:?}");
            }
        }

        // Validate no conflict between Plan.groups and Task.group
        for task in &self.tasks {
            if let Some(ref task_group) = task.group {
                if let Some(plan_groups) = task_to_groups.get(task.id.as_str()) {
                    if !plan_groups.contains(&task_group.as_str()) {
                        bail!(
                            "task '{}' has group field '{}' but is listed in Plan.groups under '{}'",
                            task.id,
                            task_group,
                            plan_groups[0]
                        );
                    }
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

    /// Returns task IDs belonging to a specific group (from Plan.groups only).
    pub fn get_group_tasks(&self, group: &str) -> Vec<&str> {
        self.groups
            .get(group)
            .map(|ids| ids.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Returns the group name for a task ID (from Task.group field).
    pub fn get_task_group(&self, task_id: &str) -> Option<&str> {
        self.tasks
            .iter()
            .find(|t| t.id == task_id)
            .and_then(|t| t.group.as_deref())
    }

    /// Resolves all group memberships from both Plan.groups and Task.group fields.
    /// Returns a HashMap mapping group names to their member task IDs.
    pub fn resolve_groups(&self) -> HashMap<String, Vec<String>> {
        let mut resolved: HashMap<String, Vec<String>> = HashMap::new();

        // Add tasks from Plan.groups
        for (group_name, task_ids) in &self.groups {
            resolved
                .entry(group_name.clone())
                .or_default()
                .extend(task_ids.clone());
        }

        // Add tasks from Task.group fields
        for task in &self.tasks {
            if let Some(ref group) = task.group {
                let entry = resolved.entry(group.clone()).or_default();
                if !entry.contains(&task.id) {
                    entry.push(task.id.clone());
                }
            }
        }

        resolved
    }
}

fn validate_completion_gates(context: &str, gates: &[CompletionGate]) -> Result<()> {
    let mut gate_names = HashSet::new();
    for gate in gates {
        if gate.name.trim().is_empty() {
            bail!("{context} contains gate with empty name");
        }
        if gate.command.trim().is_empty() {
            bail!("{context} gate '{}' has empty command", gate.name);
        }
        if gate.timeout_sec == 0 {
            bail!(
                "{context} gate '{}' has timeout_sec of 0 (must be > 0)",
                gate.name
            );
        }
        if !gate_names.insert(gate.name.clone()) {
            bail!("{context} has duplicate gate name '{}'", gate.name);
        }
    }
    Ok(())
}

/// Generate JSON Schema for Plan
pub fn plan_json_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_delay_fixed_backoff() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Fixed,
            max_delay_sec: 300,
            jitter_percent: 0,
            skip_permanent_failures: false,
        };

        // Fixed delay should remain constant regardless of attempt
        assert_eq!(strategy.calculate_delay(10, 1), 10);
        assert_eq!(strategy.calculate_delay(10, 2), 10);
        assert_eq!(strategy.calculate_delay(10, 5), 10);
    }

    #[test]
    fn test_calculate_delay_exponential_backoff() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Exponential,
            max_delay_sec: 300,
            jitter_percent: 0,
            skip_permanent_failures: false,
        };

        // Exponential: base * 2^(attempt-1)
        assert_eq!(strategy.calculate_delay(5, 1), 5); // 5 * 2^0 = 5
        assert_eq!(strategy.calculate_delay(5, 2), 10); // 5 * 2^1 = 10
        assert_eq!(strategy.calculate_delay(5, 3), 20); // 5 * 2^2 = 20
        assert_eq!(strategy.calculate_delay(5, 4), 40); // 5 * 2^3 = 40
    }

    #[test]
    fn test_calculate_delay_linear_backoff() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Linear,
            max_delay_sec: 300,
            jitter_percent: 0,
            skip_permanent_failures: false,
        };

        // Linear: base * attempt
        assert_eq!(strategy.calculate_delay(5, 1), 5); // 5 * 1 = 5
        assert_eq!(strategy.calculate_delay(5, 2), 10); // 5 * 2 = 10
        assert_eq!(strategy.calculate_delay(5, 3), 15); // 5 * 3 = 15
        assert_eq!(strategy.calculate_delay(5, 4), 20); // 5 * 4 = 20
    }

    #[test]
    fn test_calculate_delay_max_cap() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Exponential,
            max_delay_sec: 60, // Cap at 60 seconds
            jitter_percent: 0,
            skip_permanent_failures: false,
        };

        // Should cap at max_delay_sec
        assert_eq!(strategy.calculate_delay(10, 1), 10); // 10 * 2^0 = 10
        assert_eq!(strategy.calculate_delay(10, 2), 20); // 10 * 2^1 = 20
        assert_eq!(strategy.calculate_delay(10, 3), 40); // 10 * 2^2 = 40
        assert_eq!(strategy.calculate_delay(10, 4), 60); // 10 * 2^3 = 80, capped to 60
        assert_eq!(strategy.calculate_delay(10, 5), 60); // 10 * 2^4 = 160, capped to 60
    }

    #[test]
    fn test_calculate_delay_zero_base() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Exponential,
            max_delay_sec: 300,
            jitter_percent: 25,
            skip_permanent_failures: false,
        };

        // Zero base delay should return 0 regardless of jitter
        assert_eq!(strategy.calculate_delay(0, 1), 0);
        assert_eq!(strategy.calculate_delay(0, 5), 0);
    }

    #[test]
    fn test_calculate_delay_with_jitter() {
        let strategy = RetryStrategy {
            inject_error_context: false,
            escalate_model: None,
            max_stderr_lines: 50,
            backoff_type: BackoffType::Fixed,
            max_delay_sec: 300,
            jitter_percent: 50, // 50% jitter
            skip_permanent_failures: false,
        };

        // With jitter, delay should be within ±50% of base
        let base = 100u64;
        let delay = strategy.calculate_delay(base, 1);
        // Delay should be at least 1 (minimum) and within reasonable range
        assert!(delay >= 1);
        assert!((50..=150).contains(&delay)); // 100 ± 50%
    }

    #[test]
    fn test_backoff_type_default() {
        // Default should be Fixed
        assert_eq!(BackoffType::default(), BackoffType::Fixed);
    }

    // ==================== FailureType tests ====================

    #[test]
    fn test_failure_type_from_exit_code_permanent() {
        assert_eq!(FailureType::from_exit_code(Some(2)), FailureType::Permanent);
        assert_eq!(
            FailureType::from_exit_code(Some(126)),
            FailureType::Permanent
        );
        assert_eq!(
            FailureType::from_exit_code(Some(127)),
            FailureType::Permanent
        );
        assert_eq!(
            FailureType::from_exit_code(Some(130)),
            FailureType::Permanent
        );
    }

    #[test]
    fn test_failure_type_from_exit_code_transient() {
        assert_eq!(
            FailureType::from_exit_code(Some(137)),
            FailureType::Transient
        );
        assert_eq!(
            FailureType::from_exit_code(Some(143)),
            FailureType::Transient
        );
        assert_eq!(
            FailureType::from_exit_code(Some(141)),
            FailureType::Transient
        ); // SIGPIPE
    }

    #[test]
    fn test_failure_type_from_exit_code_unknown() {
        assert_eq!(FailureType::from_exit_code(None), FailureType::Unknown);
        assert_eq!(FailureType::from_exit_code(Some(0)), FailureType::Unknown);
        assert_eq!(FailureType::from_exit_code(Some(1)), FailureType::Unknown);
        assert_eq!(FailureType::from_exit_code(Some(42)), FailureType::Unknown);
    }

    #[test]
    fn test_failure_type_from_stderr_permanent() {
        assert_eq!(
            FailureType::from_stderr_patterns("Permission denied: /etc/passwd"),
            FailureType::Permanent
        );
        assert_eq!(
            FailureType::from_stderr_patterns("No such file or directory"),
            FailureType::Permanent
        );
        assert_eq!(
            FailureType::from_stderr_patterns("Compilation failed with errors"),
            FailureType::Permanent
        );
        assert_eq!(
            FailureType::from_stderr_patterns("type error: undefined is not a function"),
            FailureType::Permanent
        );
    }

    #[test]
    fn test_failure_type_from_stderr_transient() {
        assert_eq!(
            FailureType::from_stderr_patterns("Connection refused"),
            FailureType::Transient
        );
        assert_eq!(
            FailureType::from_stderr_patterns("Request timeout after 30s"),
            FailureType::Transient
        );
        assert_eq!(
            FailureType::from_stderr_patterns("Error 503 Service Unavailable"),
            FailureType::Transient
        );
        assert_eq!(
            FailureType::from_stderr_patterns("Rate limit exceeded"),
            FailureType::Transient
        );
    }

    #[test]
    fn test_failure_type_from_stderr_unknown() {
        assert_eq!(
            FailureType::from_stderr_patterns("Something went wrong"),
            FailureType::Unknown
        );
        assert_eq!(FailureType::from_stderr_patterns(""), FailureType::Unknown);
    }

    #[test]
    fn test_failure_type_classify() {
        // Stderr takes precedence
        assert_eq!(
            FailureType::classify(Some(1), "Connection timeout"),
            FailureType::Transient
        );
        // Exit code fallback
        assert_eq!(
            FailureType::classify(Some(127), "Something happened"),
            FailureType::Permanent
        );
        // Both unknown
        assert_eq!(
            FailureType::classify(Some(1), "Some error"),
            FailureType::Unknown
        );
    }

    #[test]
    fn test_failure_type_should_retry() {
        assert!(FailureType::Transient.should_retry());
        assert!(!FailureType::Permanent.should_retry());
        assert!(FailureType::Unknown.should_retry());
    }

    #[test]
    fn test_failure_type_default() {
        assert_eq!(FailureType::default(), FailureType::Unknown);
    }

    #[test]
    fn validate_rejects_zero_max_concurrency_by_mode() {
        let plan = Plan {
            version: 1,
            run: RunConfig {
                max_concurrency_by_mode: Some(HashMap::from([("research".to_string(), 0)])),
                ..RunConfig::default()
            },
            profiles: HashMap::new(),
            groups: HashMap::new(),
            tasks: vec![Task {
                id: "task1".to_string(),
                title: None,
                deps: Vec::new(),
                mode: TaskMode::Implement,
                kind: None,
                profile: None,
                group: None,
                locks: Vec::new(),
                _timeout_sec_rejected: None,
                _default_timeout_sec_rejected: None,
                no_worktree: false,
                output_files: None,
                codex: Some(CodexConfig {
                    prompt: "do it".to_string(),
                    output_last_message: None,
                    verify_after: true,
                    sandbox: None,
                    ask_for_approval: None,
                    json: true,
                }),
                claude_code: None,
                opencode: None,
                retry_count: 0,
                retry_delay_sec: 0,
                retry_strategy: None,
                context: None,
                condition: None,
                stall_timeout_sec: None,
                completion_gates: None,
                skip_gates: false,
                auto_commit: true,
                squash: false,
                hooks: None,
            }],
            _timeout_sec_rejected: None,
            _default_timeout_sec_rejected: None,
        };

        let err = plan.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("run.max_concurrency_by_mode.research")
        );
    }

    #[test]
    fn validate_rejects_unknown_max_concurrency_by_mode_key() {
        let plan = Plan {
            version: 1,
            run: RunConfig {
                max_concurrency_by_mode: Some(HashMap::from([("unknown".to_string(), 1)])),
                ..RunConfig::default()
            },
            profiles: HashMap::new(),
            groups: HashMap::new(),
            tasks: vec![Task {
                id: "task1".to_string(),
                title: None,
                deps: Vec::new(),
                mode: TaskMode::Implement,
                kind: None,
                profile: None,
                group: None,
                locks: Vec::new(),
                _timeout_sec_rejected: None,
                _default_timeout_sec_rejected: None,
                no_worktree: false,
                output_files: None,
                codex: Some(CodexConfig {
                    prompt: "do it".to_string(),
                    output_last_message: None,
                    verify_after: true,
                    sandbox: None,
                    ask_for_approval: None,
                    json: true,
                }),
                claude_code: None,
                opencode: None,
                retry_count: 0,
                retry_delay_sec: 0,
                retry_strategy: None,
                context: None,
                condition: None,
                stall_timeout_sec: None,
                completion_gates: None,
                skip_gates: false,
                auto_commit: true,
                squash: false,
                hooks: None,
            }],
            _timeout_sec_rejected: None,
            _default_timeout_sec_rejected: None,
        };

        let err = plan.validate().unwrap_err();
        assert!(err.to_string().contains("unknown mode 'unknown'"));
    }
}
