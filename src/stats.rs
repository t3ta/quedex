//! Statistics module for dynamic timeout calculation.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::plan::DynamicTimeout;

/// Statistics for a single task's execution times.
#[derive(Debug, Clone, Default)]
pub struct TaskStats {
    /// Execution times in seconds
    pub durations: Vec<f64>,
}

impl TaskStats {
    /// Add a duration to the statistics.
    pub fn add(&mut self, duration_secs: f64) {
        self.durations.push(duration_secs);
    }

    /// Calculate the mean execution time.
    pub fn mean(&self) -> Option<f64> {
        if self.durations.is_empty() {
            return None;
        }
        Some(self.durations.iter().sum::<f64>() / self.durations.len() as f64)
    }

    /// Calculate the standard deviation.
    pub fn std_dev(&self) -> Option<f64> {
        let mean = self.mean()?;
        if self.durations.len() < 2 {
            return None;
        }
        let variance = self.durations.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>() / (self.durations.len() - 1) as f64;
        Some(variance.sqrt())
    }

    /// Calculate auto timeout (mean + 2σ).
    /// Falls back to mean * 1.5 if not enough data for std_dev.
    pub fn auto_timeout(&self) -> Option<u64> {
        let mean = self.mean()?;
        let timeout = if let Some(std_dev) = self.std_dev() {
            mean + 2.0 * std_dev
        } else {
            // Not enough data for std_dev, use 1.5x mean
            mean * 1.5
        };
        Some(timeout.ceil() as u64)
    }

    /// Calculate multiplied timeout (mean * multiplier).
    pub fn multiplied_timeout(&self, multiplier: f64) -> Option<u64> {
        let mean = self.mean()?;
        Some((mean * multiplier).ceil() as u64)
    }

    /// Resolve dynamic timeout based on type.
    pub fn resolve_dynamic(&self, dynamic: &DynamicTimeout) -> Option<u64> {
        match dynamic {
            DynamicTimeout::Auto => self.auto_timeout(),
            DynamicTimeout::TwoXAverage => self.multiplied_timeout(2.0),
        }
    }
}

/// Collection of statistics for all tasks.
#[derive(Debug, Clone, Default)]
pub struct StatsCollector {
    /// Statistics per task ID
    pub tasks: HashMap<String, TaskStats>,
}

// Internal struct for deserializing state.json
#[derive(Deserialize)]
struct TaskState {
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct State {
    tasks: HashMap<String, TaskState>,
}

impl StatsCollector {
    /// Create a new empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Collect statistics from all runs in the store root.
    pub fn collect_from_store(store_root: &Path) -> Result<Self> {
        let mut collector = Self::new();
        let runs_dir = store_root.join("runs");

        if !runs_dir.exists() {
            return Ok(collector);
        }

        for entry in std::fs::read_dir(&runs_dir)
            .with_context(|| format!("read runs dir: {}", runs_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let state_path = entry.path().join("state.json");
            if !state_path.exists() {
                continue;
            }

            if let Ok(state) = Self::read_state(&state_path) {
                for (task_id, task_state) in state.tasks {
                    if let (Some(started), Some(completed)) = 
                        (task_state.started_at, task_state.completed_at) 
                    {
                        let duration = (completed - started).num_seconds() as f64;
                        if duration > 0.0 {
                            collector.tasks
                                .entry(task_id)
                                .or_default()
                                .add(duration);
                        }
                    }
                }
            }
        }

        Ok(collector)
    }

    fn read_state(path: &Path) -> Result<State> {
        let file = std::fs::File::open(path)?;
        let state: State = serde_json::from_reader(file)?;
        Ok(state)
    }

    /// Get statistics for a specific task.
    pub fn get(&self, task_id: &str) -> Option<&TaskStats> {
        self.tasks.get(task_id)
    }

    /// Resolve a dynamic timeout for a task.
    /// Returns None if no history exists.
    pub fn resolve_timeout(&self, task_id: &str, dynamic: &DynamicTimeout) -> Option<u64> {
        self.get(task_id)?.resolve_dynamic(dynamic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_stats_empty() {
        let stats = TaskStats::default();
        assert!(stats.mean().is_none());
        assert!(stats.std_dev().is_none());
        assert!(stats.auto_timeout().is_none());
    }

    #[test]
    fn test_task_stats_single() {
        let mut stats = TaskStats::default();
        stats.add(100.0);
        assert_eq!(stats.mean(), Some(100.0));
        assert!(stats.std_dev().is_none()); // Need 2+ samples
        // Falls back to mean * 1.5
        assert_eq!(stats.auto_timeout(), Some(150));
    }

    #[test]
    fn test_task_stats_multiple() {
        let mut stats = TaskStats::default();
        stats.add(100.0);
        stats.add(110.0);
        stats.add(90.0);
        
        let mean = stats.mean().unwrap();
        assert!((mean - 100.0).abs() < 0.01);
        
        let std_dev = stats.std_dev().unwrap();
        // std_dev should be ~10
        assert!((std_dev - 10.0).abs() < 0.1);
        
        // auto_timeout = mean + 2σ ≈ 120
        let auto = stats.auto_timeout().unwrap();
        assert!(auto >= 119 && auto <= 121);
    }

    #[test]
    fn test_multiplied_timeout() {
        let mut stats = TaskStats::default();
        stats.add(100.0);
        stats.add(100.0);
        
        assert_eq!(stats.multiplied_timeout(2.0), Some(200));
        assert_eq!(stats.multiplied_timeout(1.5), Some(150));
    }

    #[test]
    fn test_resolve_dynamic() {
        let mut stats = TaskStats::default();
        stats.add(100.0);
        stats.add(100.0);
        
        assert_eq!(stats.resolve_dynamic(&DynamicTimeout::TwoXAverage), Some(200));
    }
}
