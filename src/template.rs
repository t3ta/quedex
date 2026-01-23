//! Prompt template expansion module.
//!
//! Supports variable expansion in prompts:
//! - `${variable}` - expands to the value of `variable` from plan variables
//! - `${env.VAR}` - expands to the value of environment variable `VAR`
//! - `${output.TASK_ID}` - expands to concatenated output files for task `TASK_ID`

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::store::{State, Store, TaskStatus};

/// Compiled regex for variable expansion, created once and reused.
static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([^}]+)\}").expect("invalid regex"));

/// Regex for output references like `output.task-id`.
static OUTPUT_REF_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^output\.([A-Za-z0-9_-]+)$").expect("invalid regex"));

/// Expand template variables in a prompt string.
///
/// # Arguments
/// * `prompt` - The prompt string containing `${variable}` placeholders
/// * `variables` - Map of variable names to their values
/// * `store` - Store for resolving task outputs
///
/// # Returns
/// * `Ok(String)` - The expanded prompt with all variables substituted
/// * `Err` - If any referenced variable is undefined
///
/// # Examples
/// ```no_run
/// use std::collections::HashMap;
/// use std::env;
///
/// use quedex::store::fs::FsStore;
/// use quedex::template::expand_prompt;
///
/// let mut vars = HashMap::new();
/// vars.insert("name".to_string(), "test".to_string());
///
/// let temp_dir = env::temp_dir().join("quedex-doc-test");
/// let store = FsStore::new(&temp_dir, "doc-test").unwrap();
/// let result = expand_prompt("Hello ${name}!", &vars, &store).unwrap();
/// assert_eq!(result, "Hello test!");
/// ```
pub fn expand_prompt(
    prompt: &str,
    variables: &HashMap<String, String>,
    store: &dyn Store,
) -> Result<String> {
    let re = &*TEMPLATE_REGEX;

    let mut result = prompt.to_string();
    let mut errors: Vec<String> = Vec::new();
    let mut state_cache: Option<State> = None;
    let mut output_cache: HashMap<String, String> = HashMap::new();

    // Find all matches first to avoid borrowing issues
    let matches: Vec<(String, String)> = re
        .captures_iter(prompt)
        .map(|cap| {
            let full_match = cap.get(0).unwrap().as_str().to_string();
            let var_name = cap.get(1).unwrap().as_str().to_string();
            (full_match, var_name)
        })
        .collect();

    for (full_match, var_name) in matches {
        let value = if let Some(env_var) = var_name.strip_prefix("env.") {
            // Environment variable reference
            match std::env::var(env_var) {
                Ok(val) => val,
                Err(_) => {
                    errors.push(format!("undefined environment variable: {env_var}"));
                    continue;
                }
            }
        } else {
            match parse_output_ref(&var_name) {
                Ok(Some(task_id)) => {
                    if let Some(cached) = output_cache.get(task_id) {
                        cached.clone()
                    } else {
                        match read_output_for_task(task_id, store, &mut state_cache) {
                            Ok(output) => {
                                output_cache.insert(task_id.to_string(), output.clone());
                                output
                            }
                            Err(err) => {
                                errors.push(err.to_string());
                                continue;
                            }
                        }
                    }
                }
                Ok(None) => {
                    // Regular variable reference
                    match variables.get(&var_name) {
                        Some(val) => val.clone(),
                        None => {
                            errors.push(format!("undefined variable: {var_name}"));
                            continue;
                        }
                    }
                }
                Err(err) => {
                    errors.push(err.to_string());
                    continue;
                }
            }
        };

        result = result.replace(&full_match, &value);
    }

    if !errors.is_empty() {
        bail!("template expansion failed: {}", errors.join(", "));
    }

    Ok(result)
}

fn parse_output_ref(var_name: &str) -> Result<Option<&str>> {
    if !var_name.starts_with("output.") {
        return Ok(None);
    }
    if let Some(caps) = OUTPUT_REF_REGEX.captures(var_name) {
        let task_id = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        return Ok(Some(task_id));
    }
    bail!("invalid output reference: {var_name}");
}

fn read_output_for_task(
    task_id: &str,
    store: &dyn Store,
    state_cache: &mut Option<State>,
) -> Result<String> {
    let state = match state_cache.as_ref() {
        Some(state) => state,
        None => {
            let state = store.read_state().context("read store state")?;
            *state_cache = Some(state);
            state_cache.as_ref().expect("state cache populated")
        }
    };

    let task_state = match state.tasks.get(task_id) {
        Some(task_state) => task_state,
        None => bail!("output task {task_id} not found"),
    };

    if task_state.status != TaskStatus::Succeeded {
        bail!(
            "output task {task_id} not completed (status: {:?})",
            task_state.status
        );
    }

    let output_dir = store.output_dir(task_id);
    if !output_dir.exists() {
        bail!("output files for task {task_id} not found");
    }

    let mut files = collect_output_files(&output_dir)?;
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    if files.is_empty() {
        bail!("output files for task {task_id} not found");
    }

    let mut combined = String::new();
    for path in files {
        let content =
            fs::read_to_string(&path).with_context(|| format!("read output {}", path.display()))?;
        combined.push_str(&content);
    }

    Ok(combined)
}

fn collect_output_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }

    let mut stack = vec![dir.to_path_buf()];
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
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::store::fs::FsStore;
    use crate::store::{Event, LogStream, RunStatus, TaskState};

    struct NoopStore;

    impl Store for NoopStore {
        fn append_event(&self, _event: Event) -> Result<()> {
            Err(anyhow!("noop store"))
        }

        fn write_state(&self, _state: State) -> Result<()> {
            Err(anyhow!("noop store"))
        }

        fn read_state(&self) -> Result<State> {
            Err(anyhow!("noop store"))
        }

        fn open_log(&self, _task_id: &str, _stream: LogStream) -> Result<std::fs::File> {
            Err(anyhow!("noop store"))
        }

        fn log_path(&self, _task_id: &str, _stream: LogStream) -> PathBuf {
            panic!("noop store")
        }

        fn output_dir(&self, _task_id: &str) -> PathBuf {
            panic!("noop store")
        }

        fn save_output(&self, _task_id: &str, _filename: &str, _content: &[u8]) -> Result<PathBuf> {
            Err(anyhow!("noop store"))
        }

        fn get_output(&self, _task_id: &str, _filename: &str) -> Result<Vec<u8>> {
            Err(anyhow!("noop store"))
        }

        fn list_outputs(&self, _task_id: &str) -> Result<Vec<String>> {
            Err(anyhow!("noop store"))
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut dir = std::env::temp_dir();
        dir.push(format!("quedex-template-{label}-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_state_for(store: &FsStore, task_id: &str, status: TaskStatus) {
        let mut tasks = HashMap::new();
        tasks.insert(
            task_id.to_string(),
            TaskState {
                status,
                exit_code: Some(0),
                stderr_tail: None,
                started_at: None,
                completed_at: None,
                output_files: None,
                pid: None,
                skip_reason: None,
            },
        );
        let state = State {
            run_id: "test-run".to_string(),
            run_name: "test-run".to_string(),
            status: RunStatus::Running,
            tasks,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };
        store.write_state(state).expect("write state");
    }

    #[test]
    fn test_expand_simple_variable() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());

        let store = NoopStore;
        let result = expand_prompt("Hello ${name}!", &vars, &store).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_expand_multiple_variables() {
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), "Hello".to_string());
        vars.insert("name".to_string(), "world".to_string());

        let store = NoopStore;
        let result = expand_prompt("${greeting} ${name}!", &vars, &store).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_expand_same_variable_multiple_times() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), "Y".to_string());

        let store = NoopStore;
        let result = expand_prompt("${x}${x}${x}", &vars, &store).unwrap();
        assert_eq!(result, "YYY");
    }

    #[test]
    fn test_expand_env_variable() {
        // SAFETY: Using unique env var name to minimize conflicts with parallel tests.
        // This is inherently racy but acceptable for testing purposes.
        unsafe {
            std::env::set_var("TEST_QUEDEX_EXPAND_VAR_1", "test_value");
        }

        let vars = HashMap::new();
        let store = NoopStore;
        let result =
            expand_prompt("Value: ${env.TEST_QUEDEX_EXPAND_VAR_1}", &vars, &store).unwrap();
        assert_eq!(result, "Value: test_value");

        unsafe {
            std::env::remove_var("TEST_QUEDEX_EXPAND_VAR_1");
        }
    }

    #[test]
    fn test_undefined_variable_error() {
        let vars = HashMap::new();
        let store = NoopStore;
        let result = expand_prompt("Hello ${undefined}!", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined variable: undefined"));
    }

    #[test]
    fn test_undefined_env_variable_error() {
        let vars = HashMap::new();
        // Use a variable name that is very unlikely to exist
        let store = NoopStore;
        let result = expand_prompt("Value: ${env.QUEDEX_NONEXISTENT_12345}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined environment variable"));
    }

    #[test]
    fn test_multiple_errors() {
        let vars = HashMap::new();
        let store = NoopStore;
        let result = expand_prompt("${a} ${b} ${c}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined variable: a"));
        assert!(err.contains("undefined variable: b"));
        assert!(err.contains("undefined variable: c"));
    }

    #[test]
    fn test_no_variables() {
        let vars = HashMap::new();
        let store = NoopStore;
        let result = expand_prompt("No variables here", &vars, &store).unwrap();
        assert_eq!(result, "No variables here");
    }

    #[test]
    fn test_empty_prompt() {
        let vars = HashMap::new();
        let store = NoopStore;
        let result = expand_prompt("", &vars, &store).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_dollar_without_braces() {
        let vars = HashMap::new();
        let store = NoopStore;
        let result = expand_prompt("Price: $100", &vars, &store).unwrap();
        assert_eq!(result, "Price: $100");
    }

    #[test]
    fn test_mixed_variables_and_env() {
        // SAFETY: Using unique env var name to minimize conflicts with parallel tests.
        // This is inherently racy but acceptable for testing purposes.
        unsafe {
            std::env::set_var("TEST_QUEDEX_MIXED_VAR_2", "env_value");
        }

        let mut vars = HashMap::new();
        vars.insert("local".to_string(), "local_value".to_string());

        let store = NoopStore;
        let result = expand_prompt(
            "Local: ${local}, Env: ${env.TEST_QUEDEX_MIXED_VAR_2}",
            &vars,
            &store,
        )
        .unwrap();
        assert_eq!(result, "Local: local_value, Env: env_value");

        unsafe {
            std::env::remove_var("TEST_QUEDEX_MIXED_VAR_2");
        }
    }

    #[test]
    fn test_expand_output_reference_concatenates_files() {
        let dir = temp_dir("concat");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("a.txt"), "first\n").expect("write a.txt");
        fs::write(output_dir.join("b.txt"), "second\n").expect("write b.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output:\n${output.task-a}", &vars, &store).unwrap();
        assert_eq!(result, "Output:\nfirst\nsecond\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_task_missing_error() {
        let dir = temp_dir("missing-task");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "other-task", TaskStatus::Succeeded);

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("output task task-a not found"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_task_not_completed_error() {
        let dir = temp_dir("not-completed");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Running);

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not completed"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_missing_files_error() {
        let dir = temp_dir("missing-files");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("output files for task task-a not found"));

        let _ = fs::remove_dir_all(dir);
    }
}
