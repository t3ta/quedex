use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use petgraph::algo::is_cyclic_directed;
use petgraph::graphmap::DiGraphMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Research,
    Implement,
    Verify,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    #[serde(default)]
    pub default_timeout_sec: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    #[serde(default)]
    pub run: RunConfig,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfig {
    pub prompt: String,
    #[serde(default)]
    pub output_last_message: Option<PathBuf>,
    #[serde(default)]
    pub verify_after: bool,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub ask_for_approval: Option<String>,
    #[serde(default = "default_json")]
    pub json: bool,
}

fn default_json() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub kind: Option<String>,
    #[serde(default)]
    pub codex: Option<CodexConfig>,
    #[serde(default)]
    pub shell: Option<ShellConfig>,
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
            if task.codex.is_some() && task.shell.is_some() {
                bail!("task {} has both codex and shell configs", task.id);
            }
            if task.codex.is_none() && task.shell.is_none() {
                bail!("task {} missing runner config", task.id);
            }
            if let Some(kind) = task.kind.as_deref() {
                match kind {
                    "codex" => {
                        if task.codex.is_none() {
                            bail!("task {} kind=codex without codex config", task.id);
                        }
                    }
                    "shell" => {
                        if task.shell.is_none() {
                            bail!("task {} kind=shell without shell config", task.id);
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
