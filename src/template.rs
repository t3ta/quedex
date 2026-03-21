use std::collections::HashMap;

use tera::{Context, Tera};

/// Template engine for expanding Tera template syntax in task prompts.
pub struct TemplateEngine {
    variables: HashMap<String, String>,
}

impl TemplateEngine {
    /// Create a new template engine with optional custom variables from config.
    pub fn new(config_vars: Option<&HashMap<String, String>>) -> Self {
        let variables = config_vars.cloned().unwrap_or_default();
        Self { variables }
    }

    /// Render a prompt string by expanding Tera template syntax.
    ///
    /// Returns the original prompt unchanged if:
    /// - The prompt contains no template syntax (`{{` or `{%`)
    /// - Template expansion fails (with a warning log)
    #[allow(clippy::too_many_arguments)]
    pub fn render_prompt(
        &self,
        prompt: &str,
        task_id: &str,
        task_title: Option<&str>,
        task_mode: &str,
        run_name: &str,
        attempt: u32,
        env: &HashMap<String, String>,
    ) -> String {
        // Skip expansion if no template syntax is present (performance optimization)
        if !prompt.contains("{{") && !prompt.contains("{%") {
            return prompt.to_owned();
        }

        let mut ctx = Context::new();
        ctx.insert("attempt", &attempt);

        // task.* variables
        let task_map = serde_json::json!({
            "id": task_id,
            "title": task_title.unwrap_or(""),
            "mode": task_mode,
        });
        ctx.insert("task", &task_map);

        // run.* variables
        let run_map = serde_json::json!({
            "name": run_name,
        });
        ctx.insert("run", &run_map);

        // env.* variables
        ctx.insert("env", env);

        // Custom variables from [templates.variables]
        for (key, value) in &self.variables {
            ctx.insert(key, value);
        }

        match Tera::one_off(prompt, &ctx, false) {
            Ok(rendered) => rendered,
            Err(e) => {
                tracing::warn!(
                    template_error = %e,
                    "prompt template expansion failed, using original"
                );
                prompt.to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_passthrough() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let result = engine.render_prompt(
            "Please implement the feature.",
            "task-1",
            Some("Build"),
            "implement",
            "my-run",
            1,
            &env,
        );
        assert_eq!(result, "Please implement the feature.");
    }

    #[test]
    fn test_template_expansion() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let result = engine.render_prompt(
            "Task {{ task.id }}: attempt {{ attempt }}",
            "build-frontend",
            Some("Build Frontend"),
            "implement",
            "my-run",
            2,
            &env,
        );
        assert_eq!(result, "Task build-frontend: attempt 2");
    }

    #[test]
    fn test_task_variables() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let result = engine.render_prompt(
            "{{ task.title }} ({{ task.mode }})",
            "t1",
            Some("My Task"),
            "research",
            "run-1",
            1,
            &env,
        );
        assert_eq!(result, "My Task (research)");
    }

    #[test]
    fn test_run_name() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let result = engine.render_prompt(
            "Run: {{ run.name }}",
            "t1",
            None,
            "implement",
            "my-run",
            1,
            &env,
        );
        assert_eq!(result, "Run: my-run");
    }

    #[test]
    fn test_env_variables() {
        let engine = TemplateEngine::new(None);
        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "production".to_string());
        let result = engine.render_prompt(
            "Env: {{ env.NODE_ENV }}",
            "t1",
            None,
            "implement",
            "run",
            1,
            &env,
        );
        assert_eq!(result, "Env: production");
    }

    #[test]
    fn test_custom_variables() {
        let mut vars = HashMap::new();
        vars.insert("project".to_string(), "quedex".to_string());
        let engine = TemplateEngine::new(Some(&vars));
        let env = HashMap::new();
        let result = engine.render_prompt(
            "Project: {{ project }}",
            "t1",
            None,
            "implement",
            "run",
            1,
            &env,
        );
        assert_eq!(result, "Project: quedex");
    }

    #[test]
    fn test_syntax_error_fallback() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let bad_template = "Hello {{ not_closed";
        let result = engine.render_prompt(bad_template, "t1", None, "implement", "run", 1, &env);
        assert_eq!(result, bad_template);
    }

    #[test]
    fn test_conditional_retry() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let prompt = "Do the task.{% if attempt > 1 %} Retry #{{ attempt }}.{% endif %}";
        let r1 = engine.render_prompt(prompt, "t1", None, "implement", "run", 1, &env);
        assert_eq!(r1, "Do the task.");
        let r2 = engine.render_prompt(prompt, "t1", None, "implement", "run", 2, &env);
        assert_eq!(r2, "Do the task. Retry #2.");
    }

    #[test]
    fn test_code_in_prompt_not_escaped() {
        let engine = TemplateEngine::new(None);
        let env = HashMap::new();
        let prompt = "Fix: x = a < b && c > d";
        let result = engine.render_prompt(prompt, "t1", None, "implement", "run", 1, &env);
        assert_eq!(result, "Fix: x = a < b && c > d");
    }
}
