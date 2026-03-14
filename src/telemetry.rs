//! OpenTelemetry tracing infrastructure for quedex.
//!
//! This module provides distributed tracing capabilities using OpenTelemetry.
//! Traces can be exported via OTLP to any compatible backend (Jaeger, Zipkin,
//! Grafana Tempo, etc.).
//!
//! # Environment Variables
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint URL (default: http://localhost:4317)
//! - `OTEL_SERVICE_NAME`: Service name for traces (default: quedex)
//! - `QUEDEX_TELEMETRY_ENABLED`: Set to "1" or "true" to enable telemetry

use anyhow::Result;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::TracerProvider;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Configuration for telemetry.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled
    pub enabled: bool,
    /// OTLP endpoint URL
    pub otlp_endpoint: String,
    /// Service name for traces
    pub service_name: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self::from_env(|key| std::env::var(key))
    }
}

impl TelemetryConfig {
    /// Create a TelemetryConfig by reading from the given env reader function.
    /// This allows injecting a custom env reader for testing without mutating
    /// process-wide environment variables.
    pub fn from_env<F>(env_reader: F) -> Self
    where
        F: for<'a> Fn(&'a str) -> Result<String, std::env::VarError>,
    {
        Self {
            enabled: env_reader("QUEDEX_TELEMETRY_ENABLED")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            otlp_endpoint: env_reader("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4317".to_string()),
            service_name: env_reader("OTEL_SERVICE_NAME").unwrap_or_else(|_| "quedex".to_string()),
        }
    }
}

/// Global tracer provider handle for shutdown
static TRACER_PROVIDER: std::sync::OnceLock<TracerProvider> = std::sync::OnceLock::new();

/// Initialize telemetry with the given configuration.
///
/// This sets up:
/// - Console logging with configurable log levels via RUST_LOG
/// - OpenTelemetry tracing with OTLP export (if enabled)
///
/// # Errors
///
/// Returns an error if the OTLP exporter fails to initialize.
pub fn init(config: &TelemetryConfig) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if config.enabled {
        init_with_otel(config, env_filter)
    } else {
        init_console_only(env_filter)
    }
}

fn init_console_only(env_filter: EnvFilter) -> Result<()> {
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    Ok(())
}

fn init_with_otel(config: &TelemetryConfig, env_filter: EnvFilter) -> Result<()> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::runtime;

    let resource = Resource::new(vec![KeyValue::new(
        "service.name",
        config.service_name.clone(),
    )]);

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()?;

    let provider = TracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter, runtime::Tokio)
        .build();

    let tracer = match TRACER_PROVIDER.set(provider) {
        Ok(()) => TRACER_PROVIDER.get().unwrap().tracer("quedex"),
        Err(_) => {
            // Provider already set; reuse existing provider's tracer
            TRACER_PROVIDER.get().unwrap().tracer("quedex")
        }
    };

    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(telemetry_layer)
        .try_init();

    Ok(())
}

/// Shutdown telemetry and flush any pending spans.
///
/// This should be called before the application exits to ensure
/// all traces are properly exported.
pub fn shutdown() {
    if let Some(provider) = TRACER_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            eprintln!("Failed to shutdown telemetry provider: {e}");
        }
    }
}

/// Macro for creating a span with task context.
///
/// This creates a span with standard quedex attributes:
/// - task_id
/// - task_mode
/// - task_runner
#[macro_export]
macro_rules! task_span {
    ($task_id:expr, $mode:expr, $runner:expr) => {
        tracing::info_span!(
            "task_execute",
            task_id = %$task_id,
            task_mode = %$mode,
            task_runner = %$runner,
            otel_kind = "INTERNAL"
        )
    };
}

/// Record task completion metrics.
#[inline]
pub fn record_task_completion(
    task_id: &str,
    exit_code: Option<i32>,
    duration_ms: u64,
    retry_count: u32,
) {
    tracing::info!(
        target: "quedex::metrics",
        task_id = %task_id,
        task_exit_code = ?exit_code,
        task_duration_ms = duration_ms,
        task_retry_count = retry_count,
        "task completed"
    );
}

/// Record retry attempt.
#[inline]
pub fn record_retry(task_id: &str, attempt: u32, delay_ms: u64, reason: &str) {
    tracing::warn!(
        target: "quedex::metrics",
        task_id = %task_id,
        retry_attempt = attempt,
        retry_delay_ms = delay_ms,
        retry_reason = %reason,
        "task retry scheduled"
    );
}

/// Record circuit breaker state change.
#[inline]
pub fn record_circuit_breaker_state(task_id: &str, from_state: &str, to_state: &str) {
    tracing::info!(
        target: "quedex::metrics",
        task_id = %task_id,
        circuit_breaker_from = %from_state,
        circuit_breaker_to = %to_state,
        "circuit breaker state changed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mock_env(vars: HashMap<&str, &str>) -> impl Fn(&str) -> Result<String, std::env::VarError> {
        let owned: HashMap<String, String> = vars
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| {
            owned
                .get(key)
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    #[test]
    fn test_telemetry_config_default() {
        let config = TelemetryConfig::from_env(mock_env(HashMap::new()));
        assert!(!config.enabled);
        assert_eq!(config.otlp_endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "quedex");
    }

    #[test]
    fn test_telemetry_config_from_env() {
        let vars = HashMap::from([
            ("QUEDEX_TELEMETRY_ENABLED", "true"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://jaeger:4317"),
            ("OTEL_SERVICE_NAME", "my-quedex"),
        ]);
        let config = TelemetryConfig::from_env(mock_env(vars));
        assert!(config.enabled);
        assert_eq!(config.otlp_endpoint, "http://jaeger:4317");
        assert_eq!(config.service_name, "my-quedex");
    }
}
