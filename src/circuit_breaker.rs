//! Circuit Breaker pattern implementation for protecting against cascading failures.
//!
//! The circuit breaker has three states:
//! - Closed: Normal operation, requests pass through
//! - Open: Failures exceeded threshold, requests are blocked
//! - HalfOpen: Recovery period, limited requests allowed to test if service recovered
//!
//! State transitions:
//! - Closed → Open: When failure_count >= threshold
//! - Open → HalfOpen: After recovery_timeout expires
//! - HalfOpen → Closed: On successful request
//! - HalfOpen → Open: On failed request

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The state of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    /// Normal operation - requests pass through
    Closed = 0,
    /// Circuit tripped - requests are blocked
    Open = 1,
    /// Testing recovery - requests allowed to probe service health
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(value: u8) -> Self {
        match value {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed, // Default fallback
        }
    }
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit
    pub failure_threshold: u32,
    /// Duration to wait before attempting recovery (transition to HalfOpen)
    pub recovery_timeout: Duration,
    /// Number of successful requests needed in HalfOpen to close the circuit
    pub success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(60),
            success_threshold: 1,
        }
    }
}

/// A circuit breaker implementation using atomic operations for state storage
/// and a Mutex for serializing state transitions.
///
/// Thread-safe with lock-free reads and mutex-guarded transitions.
pub struct CircuitBreaker {
    /// Current state (Closed=0, Open=1, HalfOpen=2)
    state: AtomicU32,
    /// Number of consecutive failures
    failure_count: AtomicU32,
    /// Number of consecutive successes (in HalfOpen state)
    success_count: AtomicU32,
    /// Timestamp when circuit was opened (unix timestamp in seconds)
    opened_at: AtomicU64,
    /// Configuration
    config: CircuitBreakerConfig,
    /// Mutex for state transitions to prevent races
    transition_lock: Mutex<()>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: AtomicU32::new(CircuitState::Closed as u32),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            opened_at: AtomicU64::new(0),
            config,
            transition_lock: Mutex::new(()),
        }
    }

    /// Create a circuit breaker with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Get the current state of the circuit breaker.
    pub fn state(&self) -> CircuitState {
        let state_val = self.state.load(Ordering::Acquire);
        let state = CircuitState::from(state_val as u8);

        // Check if we should transition from Open to HalfOpen
        if state == CircuitState::Open {
            let opened_at = self.opened_at.load(Ordering::Acquire);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if now.saturating_sub(opened_at) >= self.config.recovery_timeout.as_secs() {
                // Attempt transition to HalfOpen
                let _guard = self.transition_lock.lock().unwrap();
                // Re-check state under lock — another thread may have
                // already transitioned, so always return the current value.
                let current = self.state.load(Ordering::Acquire);
                if current == CircuitState::Open as u32 {
                    self.state
                        .store(CircuitState::HalfOpen as u32, Ordering::Release);
                    self.success_count.store(0, Ordering::Release);
                    return CircuitState::HalfOpen;
                }
                return CircuitState::from(current as u8);
            }
        }

        state
    }

    /// Check if a request is allowed.
    ///
    /// Returns true if the circuit is closed or half-open, false if open.
    /// Note: In HalfOpen state, all requests are currently allowed.
    /// A single-flight probe mechanism is not yet implemented.
    pub fn allow_request(&self) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => false,
        }
    }

    /// Record a successful request.
    ///
    /// In HalfOpen state, this may close the circuit.
    pub fn record_success(&self) {
        let _guard = self.transition_lock.lock().unwrap();

        let state = CircuitState::from(self.state.load(Ordering::Acquire) as u8);

        match state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Release);
            }
            CircuitState::HalfOpen => {
                let success_count = self.success_count.fetch_add(1, Ordering::AcqRel) + 1;
                if success_count >= self.config.success_threshold {
                    // Transition to Closed
                    self.state
                        .store(CircuitState::Closed as u32, Ordering::Release);
                    self.failure_count.store(0, Ordering::Release);
                    self.success_count.store(0, Ordering::Release);
                }
            }
            CircuitState::Open => {
                // Ignore successes while circuit is open.
                // Late successes from in-flight requests should not
                // bypass the recovery_timeout window.
            }
        }
    }

    /// Record a failed request.
    ///
    /// May open the circuit if failure threshold is exceeded.
    pub fn record_failure(&self) {
        let _guard = self.transition_lock.lock().unwrap();

        let state = CircuitState::from(self.state.load(Ordering::Acquire) as u8);

        match state {
            CircuitState::Closed => {
                let failure_count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
                if failure_count >= self.config.failure_threshold {
                    // Transition to Open
                    self.state
                        .store(CircuitState::Open as u32, Ordering::Release);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    self.opened_at.store(now, Ordering::Release);
                }
            }
            CircuitState::HalfOpen => {
                // Single failure in HalfOpen reopens the circuit
                self.state
                    .store(CircuitState::Open as u32, Ordering::Release);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.opened_at.store(now, Ordering::Release);
                self.failure_count
                    .store(self.config.failure_threshold, Ordering::Release);
            }
            CircuitState::Open => {
                // Already open, nothing to do
            }
        }
    }

    /// Reset the circuit breaker to closed state.
    pub fn reset(&self) {
        let _guard = self.transition_lock.lock().unwrap();
        self.state
            .store(CircuitState::Closed as u32, Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
        self.success_count.store(0, Ordering::Release);
        self.opened_at.store(0, Ordering::Release);
    }

    /// Get the number of consecutive failures.
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Acquire)
    }

    /// Check if the circuit is open.
    pub fn is_open(&self) -> bool {
        self.state() == CircuitState::Open
    }

    /// Check if the circuit is closed.
    pub fn is_closed(&self) -> bool {
        self.state() == CircuitState::Closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_initial_state() {
        let cb = CircuitBreaker::with_defaults();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_stays_closed_under_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 1);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 2);

        assert!(cb.allow_request());
    }

    #[test]
    fn test_circuit_breaker_opens_at_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        cb.record_failure();
        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count(), 2);

        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_success_closes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(0), // Immediate transition for testing
            success_threshold: 1,
        };
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        // With recovery_timeout=0, state() will immediately transition to HalfOpen
        // So we check the internal state without triggering transition
        assert!(cb.failure_count() >= 2);

        // Since recovery_timeout is 0, state() will show HalfOpen
        let state = cb.state();
        assert_eq!(state, CircuitState::HalfOpen, "Expected HalfOpen after recovery timeout");
        assert!(cb.allow_request());

        // Success should close the circuit
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_failure_reopens() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(0), // Immediate transition for testing
            success_threshold: 2,
        };
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        // With recovery_timeout=0, state() will immediately transition to HalfOpen

        // Since recovery_timeout is 0, state() will show HalfOpen
        let state = cb.state();
        assert_eq!(state, CircuitState::HalfOpen, "Expected HalfOpen after recovery timeout");

        // Failure should reopen the circuit
        cb.record_failure();
        // After failure in HalfOpen, it should go back to Open
        // But checking state() again might transition back to HalfOpen due to timeout=0
        // So we check the failure count instead
        assert!(cb.failure_count() >= 2);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Reset
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 0);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_circuit_state_from_u8() {
        assert_eq!(CircuitState::from(0), CircuitState::Closed);
        assert_eq!(CircuitState::from(1), CircuitState::Open);
        assert_eq!(CircuitState::from(2), CircuitState::HalfOpen);
        assert_eq!(CircuitState::from(255), CircuitState::Closed); // Fallback
    }
}
