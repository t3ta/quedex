//! Prompt template expansion module.
//!
//! Supports variable expansion in prompts:
//! - `${variable}` - expands to the value of `variable` from plan variables
//! - `${env.VAR}` - expands to the value of environment variable `VAR`

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{bail, Result};
use regex::Regex;

/// Compiled regex for variable expansion, created once and reused.
static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([^}]+)\}").expect("invalid regex"));

/// Expand template variables in a prompt string.
///
/// # Arguments
/// * `prompt` - The prompt string containing `${variable}` placeholders
/// * `variables` - Map of variable names to their values
///
/// # Returns
/// * `Ok(String)` - The expanded prompt with all variables substituted
/// * `Err` - If any referenced variable is undefined
///
/// # Examples
/// ```
/// use std::collections::HashMap;
/// use quedex::template::expand_prompt;
///
/// let mut vars = HashMap::new();
/// vars.insert("name".to_string(), "test".to_string());
///
/// let result = expand_prompt("Hello ${name}!", &vars).unwrap();
/// assert_eq!(result, "Hello test!");
/// ```
pub fn expand_prompt(prompt: &str, variables: &HashMap<String, String>) -> Result<String> {
    let re = &*TEMPLATE_REGEX;

    let mut result = prompt.to_string();
    let mut errors: Vec<String> = Vec::new();

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
            // Regular variable reference
            match variables.get(&var_name) {
                Some(val) => val.clone(),
                None => {
                    errors.push(format!("undefined variable: {var_name}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_simple_variable() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());

        let result = expand_prompt("Hello ${name}!", &vars).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_expand_multiple_variables() {
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), "Hello".to_string());
        vars.insert("name".to_string(), "world".to_string());

        let result = expand_prompt("${greeting} ${name}!", &vars).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_expand_same_variable_multiple_times() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), "Y".to_string());

        let result = expand_prompt("${x}${x}${x}", &vars).unwrap();
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
        let result = expand_prompt("Value: ${env.TEST_QUEDEX_EXPAND_VAR_1}", &vars).unwrap();
        assert_eq!(result, "Value: test_value");

        unsafe {
            std::env::remove_var("TEST_QUEDEX_EXPAND_VAR_1");
        }
    }

    #[test]
    fn test_undefined_variable_error() {
        let vars = HashMap::new();
        let result = expand_prompt("Hello ${undefined}!", &vars);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined variable: undefined"));
    }

    #[test]
    fn test_undefined_env_variable_error() {
        let vars = HashMap::new();
        // Use a variable name that is very unlikely to exist
        let result = expand_prompt("Value: ${env.QUEDEX_NONEXISTENT_12345}", &vars);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined environment variable"));
    }

    #[test]
    fn test_multiple_errors() {
        let vars = HashMap::new();
        let result = expand_prompt("${a} ${b} ${c}", &vars);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined variable: a"));
        assert!(err.contains("undefined variable: b"));
        assert!(err.contains("undefined variable: c"));
    }

    #[test]
    fn test_no_variables() {
        let vars = HashMap::new();
        let result = expand_prompt("No variables here", &vars).unwrap();
        assert_eq!(result, "No variables here");
    }

    #[test]
    fn test_empty_prompt() {
        let vars = HashMap::new();
        let result = expand_prompt("", &vars).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_dollar_without_braces() {
        let vars = HashMap::new();
        let result = expand_prompt("Price: $100", &vars).unwrap();
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

        let result = expand_prompt("Local: ${local}, Env: ${env.TEST_QUEDEX_MIXED_VAR_2}", &vars).unwrap();
        assert_eq!(result, "Local: local_value, Env: env_value");

        unsafe {
            std::env::remove_var("TEST_QUEDEX_MIXED_VAR_2");
        }
    }
}
