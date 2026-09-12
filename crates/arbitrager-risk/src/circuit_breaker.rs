use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use tracing::{error, warn};
use arbitrager_core::types::{dec, Decimal};

/// Configuration parameters for emergency trading halts and circuit breakers.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Maximum allowed daily drawdown in USD before full trading halt
    pub max_daily_drawdown_usd: Decimal,
    /// Maximum allowed daily drawdown percentage
    pub max_daily_drawdown_pct: Decimal,
    /// Max consecutive execution errors before triggering pause
    pub max_consecutive_errors: u32,
    /// Maximum allowed spread divergence from moving average in BPS (to block toxic fills)
    pub max_spread_divergence_bps: Decimal,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_drawdown_usd: dec!(1000.0),
            max_daily_drawdown_pct: dec!(5.0),
            max_consecutive_errors: 3,
            max_spread_divergence_bps: dec!(250.0),
        }
    }
}

/// Thread-safe enterprise circuit breaker.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    is_tripped: AtomicBool,
    trip_reason: Arc<RwLock<Option<String>>>,
    consecutive_errors: AtomicU32,
    day_start_equity: Arc<RwLock<Decimal>>,
    current_equity: Arc<RwLock<Decimal>>,
    peak_equity: Arc<RwLock<Decimal>>,
    tripped_at: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig, initial_equity: Decimal) -> Self {
        Self {
            config,
            is_tripped: AtomicBool::new(false),
            trip_reason: Arc::new(RwLock::new(None)),
            consecutive_errors: AtomicU32::new(0),
            day_start_equity: Arc::new(RwLock::new(initial_equity)),
            current_equity: Arc::new(RwLock::new(initial_equity)),
            peak_equity: Arc::new(RwLock::new(initial_equity)),
            tripped_at: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if trading is allowed right now.
    pub fn is_trading_allowed(&self) -> Result<(), String> {
        if self.is_tripped.load(Ordering::Relaxed) {
            let reason = self.trip_reason.read().clone().unwrap_or_else(|| "Unknown".to_string());
            Err(format!("Circuit breaker TRIPPED: {}", reason))
        } else {
            Ok(())
        }
    }

    /// Trip circuit breaker manually or automatically.
    pub fn trip(&self, reason: impl Into<String>) {
        let r = reason.into();
        error!("🚨 CIRCUIT BREAKER TRIPPED! Reason: {}", r);
        *self.trip_reason.write() = Some(r);
        *self.tripped_at.write() = Some(Utc::now());
        self.is_tripped.store(true, Ordering::SeqCst);
    }

    /// Reset after operator review. The drawdown baseline is re-anchored to
    /// the current equity so prior losses are not carried into the new window.
    pub fn reset(&self) {
        warn!("Circuit breaker manually RESET by operator");
        let current = *self.current_equity.read();
        *self.day_start_equity.write() = current;
        *self.peak_equity.write() = current;
        *self.trip_reason.write() = None;
        *self.tripped_at.write() = None;
        self.consecutive_errors.store(0, Ordering::SeqCst);
        self.is_tripped.store(false, Ordering::SeqCst);
    }

    /// Record an execution error (e.g. unhedged leg, API timeout).
    pub fn record_error(&self, details: &str) {
        let count = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
        warn!("Consecutive execution error #{} / {}: {}", count, self.config.max_consecutive_errors, details);
        if count >= self.config.max_consecutive_errors {
            self.trip(format!(
                "Exceeded maximum consecutive execution errors ({}/{}): {}",
                count, self.config.max_consecutive_errors, details
            ));
        }
    }

    /// Record a successful execution fill on all legs.
    pub fn record_success(&self) {
        self.consecutive_errors.store(0, Ordering::SeqCst);
    }

    /// Update equity and check drawdown limits.
    pub fn update_equity(&self, new_equity: Decimal) {
        *self.current_equity.write() = new_equity;

        let mut peak = self.peak_equity.write();
        if new_equity > *peak {
            *peak = new_equity;
        }

        let start = *self.day_start_equity.read();
        if start > dec!(0) && new_equity < start {
            let dollar_loss = start - new_equity;
            let loss_pct = (dollar_loss / start) * dec!(100);

            if dollar_loss >= self.config.max_daily_drawdown_usd {
                self.trip(format!(
                    "Daily dollar drawdown limit breached: -${} (max ${})",
                    dollar_loss, self.config.max_daily_drawdown_usd
                ));
            } else if loss_pct >= self.config.max_daily_drawdown_pct {
                self.trip(format!(
                    "Daily percentage drawdown limit breached: -{}% (max {}%)",
                    loss_pct, self.config.max_daily_drawdown_pct
                ));
            }
        }
    }

    /// Get current consecutive errors count
    pub fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors.load(Ordering::Relaxed)
    }

    /// Get configured max consecutive errors
    pub fn max_consecutive_errors(&self) -> u32 {
        self.config.max_consecutive_errors
    }

    /// Get configured max daily dollar drawdown
    pub fn max_daily_drawdown_usd(&self) -> Decimal {
        self.config.max_daily_drawdown_usd
    }

    /// Get starting equity for the day
    pub fn day_start_equity(&self) -> Decimal {
        *self.day_start_equity.read()
    }

    /// Get current equity
    pub fn current_equity(&self) -> Decimal {
        *self.current_equity.read()
    }

    /// Get peak equity observed
    pub fn peak_equity(&self) -> Decimal {
        *self.peak_equity.read()
    }

    /// Calculate current dollar drawdown from day start
    pub fn current_drawdown_usd(&self) -> Decimal {
        let start = *self.day_start_equity.read();
        let cur = *self.current_equity.read();
        if cur < start {
            start - cur
        } else {
            dec!(0)
        }
    }

    /// Check if circuit breaker is currently tripped
    pub fn is_tripped(&self) -> bool {
        self.is_tripped.load(Ordering::Relaxed)
    }

    /// Get optional trip reason string
    pub fn trip_reason(&self) -> Option<String> {
        self.trip_reason.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_reanchors_drawdown_baseline() {
        let cb = CircuitBreaker::new(
            CircuitBreakerConfig {
                max_daily_drawdown_usd: dec!(1000.0),
                ..CircuitBreakerConfig::default()
            },
            dec!(100000.0),
        );

        // Drop equity below the drawdown limit and confirm the trip
        cb.update_equity(dec!(98000.0));
        assert!(cb.is_tripped());

        // Operator reset must re-anchor the baseline at current equity so a
        // further small loss does not instantly re-trip on the stale baseline
        cb.reset();
        assert!(!cb.is_tripped());
        assert_eq!(cb.day_start_equity(), dec!(98000.0));
        assert_eq!(cb.peak_equity(), dec!(98000.0));
        assert_eq!(cb.current_drawdown_usd(), dec!(0));

        cb.update_equity(dec!(97500.0));
        assert!(!cb.is_tripped());
        cb.update_equity(dec!(96900.0));
        assert!(cb.is_tripped());
    }
}
