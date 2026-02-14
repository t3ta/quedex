use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::{Event, LogStream, State, Store};

pub struct FsStore {
    root: PathBuf,
    run_id: String,
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>, run_id: impl Into<String>) -> Result<Self> {
        let store = Self {
            root: root.into(),
            run_id: run_id.into(),
        };
        store.ensure_dirs()?;
        Ok(store)
    }

    fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.tasks_dir()).context("create store directories")?;
        Ok(())
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("runs").join(&self.run_id)
    }

    fn tasks_dir(&self) -> PathBuf {
        self.run_dir().join("tasks")
    }

    fn events_path(&self) -> PathBuf {
        self.run_dir().join("events.jsonl")
    }

    fn state_path(&self) -> PathBuf {
        self.run_dir().join("state.json")
    }

    fn outputs_dir(&self) -> PathBuf {
        self.run_dir().join("outputs")
    }

    fn output_task_dir(&self, task_id: &str) -> PathBuf {
        self.outputs_dir().join(task_id)
    }

    fn context_dir(&self) -> PathBuf {
        self.run_dir().join("context")
    }

    fn output_path(&self, task_id: &str, filename: &str) -> Result<PathBuf> {
        if filename.trim().is_empty() {
            bail!("output filename is empty");
        }
        let path = Path::new(filename);
        if path.is_absolute() {
            bail!("output filename must be relative: {filename}");
        }
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    bail!("output filename must not contain '..': {filename}");
                }
                Component::RootDir | Component::Prefix(_) => {
                    bail!("output filename must be relative: {filename}");
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        Ok(self.output_task_dir(task_id).join(path))
    }
}

impl Store for FsStore {
    fn append_event(&self, event: Event) -> Result<()> {
        self.ensure_dirs()?;
        let path = self.events_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open events log {}", path.display()))?;
        let line = serde_json::to_string(&event).context("serialize event")?;
        file.write_all(line.as_bytes())
            .context("write event line")?;
        file.write_all(b"\n").context("write event newline")?;
        Ok(())
    }

    fn write_state(&self, state: State) -> Result<()> {
        self.ensure_dirs()?;
        let state_path = self.state_path();
        let tmp_path = self.run_dir().join("state.json.tmp");
        {
            let mut file =
                File::create(&tmp_path).with_context(|| format!("open {}", tmp_path.display()))?;
            serde_json::to_writer_pretty(&mut file, &state).context("serialize state")?;
            file.write_all(b"\n").context("write state newline")?;
            file.sync_all().context("sync state file")?;
        }
        fs::rename(&tmp_path, &state_path)
            .with_context(|| format!("rename {} -> {}", tmp_path.display(), state_path.display()))?;
        Ok(())
    }

    fn read_state(&self) -> Result<State> {
        let path = self.state_path();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let state = serde_json::from_reader(file).context("deserialize state")?;
        Ok(state)
    }

    fn open_log(&self, task_id: &str, stream: LogStream) -> Result<File> {
        let task_dir = self.tasks_dir().join(task_id);
        fs::create_dir_all(&task_dir)
            .with_context(|| format!("create task dir {}", task_dir.display()))?;
        let path = task_dir.join(stream.file_name());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open log {}", path.display()))?;
        Ok(file)
    }

    fn log_path(&self, task_id: &str, stream: LogStream) -> PathBuf {
        self.tasks_dir().join(task_id).join(stream.file_name())
    }

    fn output_dir(&self, task_id: &str) -> PathBuf {
        self.output_task_dir(task_id)
    }

    fn save_output(&self, task_id: &str, filename: &str, content: &[u8]) -> Result<PathBuf> {
        let output_path = self.output_path(task_id, filename)?;
        let parent = output_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("output path missing parent for {filename}"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create output dir {}", parent.display()))?;
        let tmp_path = output_path.with_extension("tmp");
        {
            let mut file = File::create(&tmp_path)
                .with_context(|| format!("open {}", tmp_path.display()))?;
            file.write_all(content)
                .with_context(|| format!("write output {}", output_path.display()))?;
            file.sync_all()
                .with_context(|| format!("sync output {}", output_path.display()))?;
        }
        fs::rename(&tmp_path, &output_path).with_context(|| {
            format!(
                "rename {} -> {}",
                tmp_path.display(),
                output_path.display()
            )
        })?;
        Ok(output_path)
    }

    fn get_output(&self, task_id: &str, filename: &str) -> Result<Vec<u8>> {
        let output_path = self.output_path(task_id, filename)?;
        fs::read(&output_path).with_context(|| format!("read output {}", output_path.display()))
    }

    fn list_outputs(&self, task_id: &str) -> Result<Vec<String>> {
        let output_dir = self.output_dir(task_id);
        if !output_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        let mut stack = vec![output_dir.clone()];
        while let Some(current) = stack.pop() {
            for entry in fs::read_dir(&current)
                .with_context(|| format!("read {}", current.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if file_type.is_file() {
                    files.push(path);
                    continue;
                }
                if file_type.is_symlink() {
                    if let Ok(metadata) = fs::metadata(&path) {
                        if metadata.is_file() {
                            files.push(path);
                        }
                    }
                }
            }
        }

        let mut outputs = Vec::new();
        for path in files {
            let relative = path
                .strip_prefix(&output_dir)
                .with_context(|| format!("strip prefix {}", output_dir.display()))?;
            outputs.push(relative.to_string_lossy().to_string());
        }
        outputs.sort();
        Ok(outputs)
    }

    fn save_context(&self, key: &str, content: &[u8]) -> Result<()> {
        if key.trim().is_empty() {
            bail!("context key is empty");
        }
        // Validate key contains only safe characters
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            bail!("context key '{}' contains invalid characters", key);
        }
        let context_dir = self.context_dir();
        fs::create_dir_all(&context_dir)
            .with_context(|| format!("create context dir {}", context_dir.display()))?;
        let context_path = context_dir.join(key);
        let tmp_path = context_path.with_extension("tmp");
        {
            let mut file = File::create(&tmp_path)
                .with_context(|| format!("create context file {}", tmp_path.display()))?;
            file.write_all(content)
                .with_context(|| format!("write context {}", context_path.display()))?;
            file.sync_all()
                .with_context(|| format!("sync context {}", context_path.display()))?;
        }
        fs::rename(&tmp_path, &context_path).with_context(|| {
            format!(
                "rename {} -> {}",
                tmp_path.display(),
                context_path.display()
            )
        })?;
        Ok(())
    }

    fn get_context(&self, key: &str) -> Result<Vec<u8>> {
        if key.trim().is_empty() {
            bail!("context key is empty");
        }
        // Validate key contains only safe characters (same as save_context)
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            bail!("context key '{}' contains invalid characters", key);
        }
        let context_path = self.context_dir().join(key);
        fs::read(&context_path)
            .with_context(|| format!("read context '{}' at {}", key, context_path.display()))
    }

    fn save_context_versioned(
        &self,
        key: &str,
        content: &[u8],
        updated_by: &str,
        expected_version: Option<u64>,
    ) -> Result<super::ContextMetadata> {
        use super::ContextMetadata;

        if key.trim().is_empty() {
            bail!("context key is empty");
        }
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            bail!("context key '{}' contains invalid characters", key);
        }

        let context_dir = self.context_dir();
        fs::create_dir_all(&context_dir)?;

        let meta_path = context_dir.join(format!("{}.meta", key));

        // Load existing metadata if present
        let existing_meta: Option<ContextMetadata> = if meta_path.exists() {
            let meta_content = fs::read_to_string(&meta_path)?;
            Some(serde_json::from_str(&meta_content)?)
        } else {
            None
        };

        // Optimistic locking check
        if let Some(expected) = expected_version {
            let current_version = existing_meta.as_ref().map(|m| m.version).unwrap_or(0);
            if current_version != expected {
                bail!(
                    "context '{}' version conflict: expected {}, got {}",
                    key,
                    expected,
                    current_version
                );
            }
        }

        // Create new metadata
        let new_meta = if let Some(ref old_meta) = existing_meta {
            old_meta.next_version(updated_by.to_string())
        } else {
            ContextMetadata::new(updated_by.to_string())
        };

        // Save content (atomic write)
        let content_path = context_dir.join(key);
        let tmp_content_path = context_dir.join(format!("{}.tmp", key));
        {
            let mut file = File::create(&tmp_content_path)?;
            file.write_all(content)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_content_path, &content_path)?;

        // Save metadata (atomic write)
        let tmp_meta_path = context_dir.join(format!("{}.meta.tmp", key));
        {
            let meta_json = serde_json::to_string_pretty(&new_meta)?;
            let mut file = File::create(&tmp_meta_path)?;
            file.write_all(meta_json.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&tmp_meta_path, &meta_path)?;

        Ok(new_meta)
    }

    fn get_context_metadata(&self, key: &str) -> Result<Option<super::ContextMetadata>> {
        use super::ContextMetadata;

        if key.trim().is_empty() {
            bail!("context key is empty");
        }
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            bail!("context key '{}' contains invalid characters", key);
        }

        let meta_path = self.context_dir().join(format!("{}.meta", key));
        if !meta_path.exists() {
            return Ok(None);
        }

        let meta_content = fs::read_to_string(&meta_path)
            .with_context(|| format!("read context metadata '{}' at {}", key, meta_path.display()))?;
        let meta: ContextMetadata = serde_json::from_str(&meta_content)
            .with_context(|| format!("parse context metadata '{}'", key))?;
        Ok(Some(meta))
    }
}
