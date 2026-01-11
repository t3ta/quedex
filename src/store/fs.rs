use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

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
}
