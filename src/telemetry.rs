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
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

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
        Self {
            enabled: std::env::var("QUEDEX_TELEMETRY_ENABLED")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4317".to_string()),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "quedex".to_string()),
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
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
    Ok(())
}

fn init_with_otel(config: &TelemetryConfig, env_filter: EnvFilter) -> Result<()> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::runtime;
    use opentelemetry_sdk::Resource;

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

    let tracer = provider.tracer("quedex");
    let _ = TRACER_PROVIDER.set(provider);

    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(telemetry_layer)
        .init();

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

    #[test]
    fn test_telemetry_config_default() {
        // Clear env vars for test
        // SAFETY: Single-threaded test execution
        unsafe {
            std::env::remove_var("QUEDEX_TELEMETRY_ENABLED");
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            std::env::remove_var("OTEL_SERVICE_NAME");
        }

        let config = TelemetryConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.otlp_endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "quedex");
    }

    #[test]
    fn test_telemetry_config_from_env() {
        // SAFETY: Single-threaded test execution
        unsafe {
            std::env::set_var("QUEDEX_TELEMETRY_ENABLED", "true");
            std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://jaeger:4317");
            std::env::set_var("OTEL_SERVICE_NAME", "my-quedex");
        }

        let config = TelemetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.otlp_endpoint, "http://jaeger:4317");
        assert_eq!(config.service_name, "my-quedex");

        // Cleanup
        // SAFETY: Single-threaded test execution
        unsafe {
            std::env::remove_var("QUEDEX_TELEMETRY_ENABLED");
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            std::env::remove_var("OTEL_SERVICE_NAME");
        }
    }
}
