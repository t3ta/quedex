//! Prompt template expansion module.
//!
//! Supports variable expansion in prompts:
//! - `${variable}` - expands to the value of `variable` from plan variables
//! - `${env.VAR}` - expands to the value of environment variable `VAR`
//! - `${output.TASK_ID}` - expands to concatenated output files for task `TASK_ID`
//! - `${output.TASK_ID:file.txt}` - expands to content of a specific output file
//! - `${output.TASK_ID:*.json}` - expands to content of all files matching the glob pattern
//! - `${output.TASK_ID:data.result.value}` - expands to value extracted from JSON using dot notation

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use glob::glob;
use regex::Regex;
use serde_json::Value;

use crate::store::{State, Store, TaskStatus};

/// Compiled regex for variable expansion, created once and reused.
static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([^}]+)\}").expect("invalid regex"));

/// Regex for output references like `output.task-id` or `output.task-id:file.txt`.
static OUTPUT_REF_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^output\.([A-Za-z0-9_-]+)(?::([^}]+))?$").expect("invalid regex"));

/// Represents different types of output references.
#[derive(Debug, Clone)]
enum OutputRef {
    All(String),
    File(String, String),
    Glob(String, String),
    JsonPath(String, String),
}

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
            match std::env::var(env_var) {
                Ok(val) => val,
                Err(_) => {
                    errors.push(format!("undefined environment variable: {env_var}"));
                    continue;
                }
            }
        } else {
            match parse_output_ref(&var_name)? {
                Some(output_ref) => {
                    let cache_key = format!("{:?}", output_ref);
                    if let Some(cached) = output_cache.get(&cache_key) {
                        cached.clone()
                    } else {
                        match read_output_for_task(&output_ref, store, &mut state_cache) {
                            Ok(output) => {
                                output_cache.insert(cache_key.clone(), output.clone());
                                output
                            }
                            Err(err) => {
                                errors.push(err.to_string());
                                continue;
                            }
                        }
                    }
                }
                None => match variables.get(&var_name) {
                    Some(val) => val.clone(),
                    None => {
                        errors.push(format!("undefined variable: {var_name}"));
                        continue;
                    }
                },
            }
        };

        result = result.replace(&full_match, &value);
    }

    if !errors.is_empty() {
        bail!("template expansion failed: {}", errors.join(", "));
    }

    Ok(result)
}

fn parse_output_ref(var_name: &str) -> Result<Option<OutputRef>> {
    if !var_name.starts_with("output.") {
        return Ok(None);
    }
    if let Some(caps) = OUTPUT_REF_REGEX.captures(var_name) {
        let task_id = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        if let Some(path) = caps.get(2).map(|m| m.as_str()) {
            // Check for glob patterns first (wildcards)
            if contains_glob_pattern(path) {
                return Ok(Some(OutputRef::Glob(task_id.to_string(), path.to_string())));
            }
            // Check for valid relative file path (has a recognized file extension)
            if is_valid_relative_path(path) {
                return Ok(Some(OutputRef::File(task_id.to_string(), path.to_string())));
            }
            // Everything else with dots or brackets is treated as JSON path
            if path.contains('.') || path.contains('[') {
                return Ok(Some(OutputRef::JsonPath(task_id.to_string(), path.to_string())));
            }
            bail!("invalid output file path: {path}");
        }
        return Ok(Some(OutputRef::All(task_id.to_string())));
    }
    bail!("invalid output reference: {var_name}");
}

fn contains_glob_pattern(path: &str) -> bool {
    path.contains('*') || path.contains('?')
        || (path.contains('[') && !path.contains(']'))
}

fn is_valid_relative_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    if path.contains("..") {
        return false;
    }
    // Reject paths that look like JSON paths (contain dots but no recognized file extension)
    if path.contains('.') {
        // List of common file extensions to distinguish from JSON paths
        let valid_extensions = [
            "txt", "md", "rs", "py", "js", "ts", "json", "yaml", "yml", "toml", "xml", "html", "css",
            "sh", "bash", "zsh", "fish", "conf", "config", "ini", "cfg", "log", "out", "err", "tmp",
        ];
        let parts: Vec<&str> = path.split('.').collect();
        if let Some(ext) = parts.last() {
            // If it's a recognized file extension, it's a valid file path
            if valid_extensions.contains(ext) {
                return true;
            }
        }
        // Otherwise, treat as a JSON path
        return false;
    }
    true
}

fn read_output_for_task(
    output_ref: &OutputRef,
    store: &dyn Store,
    state_cache: &mut Option<State>,
) -> Result<String> {
    let task_id = match output_ref {
        OutputRef::All(id) => id,
        OutputRef::File(id, _) => id,
        OutputRef::Glob(id, _) => id,
        OutputRef::JsonPath(id, _) => id,
    };

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
        None => bail!("output task {} not found", task_id),
    };

    if task_state.status != TaskStatus::Succeeded {
        bail!("output task {} not completed (status: {:?})", task_id, task_state.status);
    }

    let output_dir = store.output_dir(task_id);

    match output_ref {
        OutputRef::All(_) => {
            if !output_dir.exists() {
                bail!("output files for task {} not found", task_id);
            }

            let mut files = collect_output_files(&output_dir)?;
            files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
            if files.is_empty() {
                bail!("output files for task {} not found", task_id);
            }

            let mut combined = String::new();
            for path in files {
                let content =
                    fs::read_to_string(&path).with_context(|| format!("read output {}", path.display()))?;
                combined.push_str(&content);
            }
            Ok(combined)
        }
        OutputRef::File(_, path) => {
            let file_path = output_dir.join(path);
            if !file_path.exists() {
                bail!("output file {} not found", path);
            }
            fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read output file {}", path))
        }
        OutputRef::Glob(_, pattern) => {
            let full_pattern = output_dir.join(pattern);
            let pattern_str = full_pattern
                .to_str()
                .context("failed to create glob pattern")?;

            let mut matched_files: Vec<PathBuf> = glob(pattern_str)
                .context("failed to parse glob pattern")?
                .filter_map(|entry| entry.ok())
                .filter(|path| path.is_file())
                .collect();

            if matched_files.is_empty() {
                bail!("no files match glob pattern '{}'", pattern);
            }

            matched_files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

            let mut combined = String::new();
            for path in matched_files {
                let content =
                    fs::read_to_string(&path).with_context(|| format!("read output {}", path.display()))?;
                combined.push_str(&content);
            }
            Ok(combined)
        }
        OutputRef::JsonPath(_, json_path) => {
            read_json_path(&output_dir, json_path)
        }
    }
}

fn read_json_path(output_dir: &Path, json_path: &str) -> Result<String> {
    let json_files: Vec<PathBuf> = std::fs::read_dir(output_dir)
        .context("failed to read output directory")?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false))
        .collect();

    if json_files.is_empty() {
        bail!("no JSON files found in output directory");
    }

    let mut found_value: Option<String> = None;

    for json_file in json_files {
        let content = fs::read_to_string(&json_file)
            .with_context(|| format!("failed to read JSON file {}", json_file.display()))?;

        let value: Value = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse JSON file {}", json_file.display()))?;

        if let Ok(extracted) = extract_json_value(&value, json_path) {
            found_value = Some(extracted);
            break;
        }
    }

    found_value.ok_or_else(|| anyhow::anyhow!("JSON path '{}' not found", json_path))
}

fn extract_json_value(value: &Value, path: &str) -> Result<String> {
    let mut current = value;
    let mut pos = 0;
    let mut part_num = 0;

    while pos < path.len() {
        part_num += 1;

        if path.chars().nth(pos) == Some('[') {
            let end = path[pos..].find(']')
                .ok_or_else(|| anyhow!("unclosed bracket in JSON path: {}", path))?;
            let index_str = &path[pos + 1..pos + end];
            let index: usize = index_str
                .parse()
                .with_context(|| format!("failed to parse array index: {}", index_str))?;

            if let Some(arr) = current.as_array() {
                if let Some(item) = arr.get(index) {
                    current = item;
                } else {
                    bail!("array index {} out of range (path part: {})", index, part_num);
                }
            } else {
                bail!("not an array (path part: {})", part_num);
            }

            pos += end + 1;
            if pos < path.len() && path.chars().nth(pos) == Some('.') {
                pos += 1;
            }
        } else {
            let end = path[pos..]
                .find(['.', '['])
                .unwrap_or(path.len() - pos);
            let part = &path[pos..pos + end];

            if part.is_empty() {
                bail!("empty part in JSON path: {}", path);
            }

            if let Some(obj) = current.as_object() {
                if let Some(v) = obj.get(part) {
                    current = v;
                } else {
                    bail!("JSON path '{}' not found (key '{}' not found)", path, part);
                }
            } else {
                bail!("not an object (path part: {})", part_num);
            }

            pos += end;
            if pos < path.len() && path.chars().nth(pos) == Some('.') {
                pos += 1;
            }
        }
    }

    match current {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".to_string()),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(current).context("failed to serialize array or object")
        }
    }
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
        dir.push(format!("quedex-template-{}-{}-{}", label, nanos, std::process::id()));
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
        assert!(err.contains("not found") && err.contains("task-a"));

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
        assert!(err.contains("not found"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_specific_file() {
        let dir = temp_dir("specific-file");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("a.txt"), "first\n").expect("write a.txt");
        fs::write(output_dir.join("b.txt"), "second\n").expect("write b.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:a.txt}", &vars, &store).unwrap();
        assert_eq!(result, "Output: first\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_specific_file_missing_error() {
        let dir = temp_dir("missing-file");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("a.txt"), "first\n").expect("write a.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:missing.txt}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found") && err.contains("missing.txt"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_glob_pattern() {
        let dir = temp_dir("glob-pattern");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("a.json"), "{\"a\": 1}\n").expect("write a.json");
        fs::write(output_dir.join("b.json"), "{\"b\": 2}\n").expect("write b.json");
        fs::write(output_dir.join("c.txt"), "text\n").expect("write c.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:*.json}", &vars, &store).unwrap();
        assert_eq!(result, "Output: {\"a\": 1}\n{\"b\": 2}\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_glob_pattern_no_match_error() {
        let dir = temp_dir("glob-no-match");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("a.txt"), "text\n").expect("write a.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:*.json}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no files match"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_json_path() {
        let dir = temp_dir("json-path");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(
            output_dir.join("data.json"),
            "{\"result\": {\"value\": \"42\"}}",
        )
        .expect("write data.json");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:result.value}", &vars, &store).unwrap();
        assert_eq!(result, "Output: 42");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_json_path_array_index() {
        let dir = temp_dir("json-path-array");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(
            output_dir.join("data.json"),
            "{\"items\": [{\"name\": \"first\"}, {\"name\": \"second\"}]}",
        )
        .expect("write data.json");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:items[0].name}", &vars, &store).unwrap();
        assert_eq!(result, "Output: first");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_json_path_missing_error() {
        let dir = temp_dir("json-path-missing");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("data.json"), "{\"result\": {\"value\": \"42\"}}")
            .expect("write data.json");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:notfound.path}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_json_path_no_json_file_error() {
        let dir = temp_dir("json-path-no-file");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let output_dir = store.output_dir("task-a");
        fs::create_dir_all(&output_dir).expect("create output dir");
        fs::write(output_dir.join("data.txt"), "text").expect("write data.txt");

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:result.value}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no JSON files found"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_output_invalid_path_error() {
        let dir = temp_dir("invalid-path");
        let store = FsStore::new(&dir, "test-run").expect("create store");
        write_state_for(&store, "task-a", TaskStatus::Succeeded);

        let vars = HashMap::new();
        let result = expand_prompt("Output: ${output.task-a:/absolute/path}", &vars, &store);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid output file path"));

        let _ = fs::remove_dir_all(dir);
    }
}