//! Cached state store with RwLock for improved read parallelism.
//!
//! This module provides a wrapper around any `Store` implementation that:
//! - Caches the State in memory using `tokio::sync::RwLock`
//! - Provides fast, non-blocking reads from cache
//! - Writes through to the underlying Store
//! - Allows multiple concurrent readers

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use super::{ContextMetadata, Event, LogStream, State, Store};

/// A cached wrapper around a Store that uses RwLock for state access.
///
/// This provides better read parallelism when multiple tasks need to
/// check state concurrently.
pub struct CachedStore<S: Store> {
    inner: S,
    state_cache: Arc<RwLock<Option<State>>>,
}

impl<S: Store> CachedStore<S> {
    /// Create a new cached store wrapping the given store.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            state_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Get a reference to the inner store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Read state asynchronously, using cache if available.
    pub async fn read_state_async(&self) -> Result<State> {
        // Try to read from cache first
        {
            let cache = self.state_cache.read().await;
            if let Some(ref state) = *cache {
                return Ok(state.clone());
            }
        }

        // Cache miss - read from underlying store and populate cache
        let state = self.inner.read_state()?;
        {
            let mut cache = self.state_cache.write().await;
            *cache = Some(state.clone());
        }
        Ok(state)
    }

    /// Write state asynchronously, updating cache and persisting.
    pub async fn write_state_async(&self, state: State) -> Result<()> {
        // Write to underlying store first
        self.inner.write_state(state.clone())?;

        // Then update cache
        {
            let mut cache = self.state_cache.write().await;
            *cache = Some(state);
        }
        Ok(())
    }

    /// Invalidate the cache, forcing next read to go to disk.
    pub async fn invalidate_cache(&self) {
        let mut cache = self.state_cache.write().await;
        *cache = None;
    }

    /// Check if cache is populated.
    pub async fn is_cached(&self) -> bool {
        let cache = self.state_cache.read().await;
        cache.is_some()
    }
}

// Implement Store trait by delegating to inner store.
// Note: The sync methods bypass the cache for simplicity.
// Use the async methods for cached access.
impl<S: Store> Store for CachedStore<S> {
    fn append_event(&self, event: Event) -> Result<()> {
        self.inner.append_event(event)
    }

    fn write_state(&self, state: State) -> Result<()> {
        // Sync write - just delegate to inner
        // The cache will be out of sync until next async read
        self.inner.write_state(state)
    }

    fn read_state(&self) -> Result<State> {
        // Sync read - bypass cache
        self.inner.read_state()
    }

    fn open_log(&self, task_id: &str, stream: LogStream) -> Result<File> {
        self.inner.open_log(task_id, stream)
    }

    fn log_path(&self, task_id: &str, stream: LogStream) -> PathBuf {
        self.inner.log_path(task_id, stream)
    }

    fn output_dir(&self, task_id: &str) -> PathBuf {
        self.inner.output_dir(task_id)
    }

    fn save_output(&self, task_id: &str, filename: &str, content: &[u8]) -> Result<PathBuf> {
        self.inner.save_output(task_id, filename, content)
    }

    fn get_output(&self, task_id: &str, filename: &str) -> Result<Vec<u8>> {
        self.inner.get_output(task_id, filename)
    }

    fn list_outputs(&self, task_id: &str) -> Result<Vec<String>> {
        self.inner.list_outputs(task_id)
    }

    fn save_context(&self, key: &str, content: &[u8]) -> Result<()> {
        self.inner.save_context(key, content)
    }

    fn get_context(&self, key: &str) -> Result<Vec<u8>> {
        self.inner.get_context(key)
    }

    fn save_context_versioned(
        &self,
        key: &str,
        content: &[u8],
        updated_by: &str,
        expected_version: Option<u64>,
    ) -> Result<ContextMetadata> {
        self.inner
            .save_context_versioned(key, content, updated_by, expected_version)
    }

    fn get_context_metadata(&self, key: &str) -> Result<Option<ContextMetadata>> {
        self.inner.get_context_metadata(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fs::FsStore;
    use crate::store::{RunStatus, TaskState, TaskStatus};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_test_state(run_id: &str) -> State {
        let mut tasks = HashMap::new();
        tasks.insert(
            "task1".to_string(),
            TaskState {
                status: TaskStatus::Pending,
                exit_code: None,
                stderr_tail: None,
                started_at: None,
                completed_at: None,
                output_files: None,
                pid: None,
                skip_reason: None,
            },
        );
        State {
            run_id: run_id.to_string(),
            run_name: "test".to_string(),
            status: RunStatus::Running,
            tasks,
            started_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn test_cached_store_basic() {
        let tmp = TempDir::new().unwrap();
        let inner = FsStore::new(tmp.path(), "test-run").unwrap();
        let cached = CachedStore::new(inner);

        // Write state
        let state = make_test_state("test-run");
        cached.write_state_async(state.clone()).await.unwrap();

        // Read should hit cache
        assert!(cached.is_cached().await);
        let read_state = cached.read_state_async().await.unwrap();
        assert_eq!(read_state.run_id, "test-run");
    }

    #[tokio::test]
    async fn test_cached_store_invalidate() {
        let tmp = TempDir::new().unwrap();
        let inner = FsStore::new(tmp.path(), "test-run").unwrap();
        let cached = CachedStore::new(inner);

        // Write state
        let state = make_test_state("test-run");
        cached.write_state_async(state).await.unwrap();
        assert!(cached.is_cached().await);

        // Invalidate
        cached.invalidate_cache().await;
        assert!(!cached.is_cached().await);

        // Read should repopulate cache
        let _ = cached.read_state_async().await.unwrap();
        assert!(cached.is_cached().await);
    }

    #[tokio::test]
    async fn test_cached_store_concurrent_reads() {
        let tmp = TempDir::new().unwrap();
        let inner = FsStore::new(tmp.path(), "test-run").unwrap();
        let cached = Arc::new(CachedStore::new(inner));

        // Write initial state
        let state = make_test_state("test-run");
        cached.write_state_async(state).await.unwrap();

        // Spawn multiple concurrent reads
        let mut handles = vec![];
        for _ in 0..10 {
            let cached_clone = Arc::clone(&cached);
            handles.push(tokio::spawn(async move {
                cached_clone.read_state_async().await.unwrap()
            }));
        }

        // All reads should succeed
        for handle in handles {
            let state = handle.await.unwrap();
            assert_eq!(state.run_id, "test-run");
        }
    }

    #[tokio::test]
    async fn test_cached_store_sync_bypasses_cache() {
        let tmp = TempDir::new().unwrap();
        let inner = FsStore::new(tmp.path(), "test-run").unwrap();
        let cached = CachedStore::new(inner);

        // Write using sync method
        let state = make_test_state("test-run");
        cached.write_state(state).unwrap();

        // Cache should not be populated (sync bypasses cache)
        assert!(!cached.is_cached().await);

        // Sync read should also work
        let read_state = cached.read_state().unwrap();
        assert_eq!(read_state.run_id, "test-run");

        // Cache still not populated
        assert!(!cached.is_cached().await);
    }
}
