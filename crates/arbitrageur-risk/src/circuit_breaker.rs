use arbitrageur_core::types::{dec, Decimal};
use chrono::{DateTime, NaiveDate, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{error, warn};

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

/// Durable snapshot of the breaker's state — persisted to kv on every
/// durable transition so a restart cannot clear a daily-loss halt or
/// silently reset the drawdown baseline mid-day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerSnapshot {
    pub tripped: bool,
    pub trip_reason: Option<String>,
    pub tripped_at: Option<DateTime<Utc>>,
    pub day_start_equity: Decimal,
    pub current_equity: Decimal,
    pub peak_equity: Decimal,
    /// UTC day the baseline belongs to. A baseline from an earlier day is
    /// stale — the next equity update re-anchors it.
    pub day_key: Option<NaiveDate>,
    pub consecutive_errors: u32,
}

/// Durable-transition sink — fired after every trip/reset/re-anchor/day
/// roll so the host layer can persist the snapshot.
pub type PersistHook = Arc<dyn Fn(&CircuitBreakerSnapshot) + Send + Sync>;

/// Operator-facing trip notifier — fired ONCE on the false→true
/// transition with the trip reason, so an out-of-band alert channel
/// (webhook, pager) sees exactly one event per halt instead of one per
/// subsequent re-trip while already halted.
pub type TripHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Thread-safe enterprise circuit breaker.
pub struct CircuitBreaker {
    config: RwLock<CircuitBreakerConfig>,
    is_tripped: AtomicBool,
    trip_reason: Arc<RwLock<Option<String>>>,
    consecutive_errors: AtomicU32,
    day_start_equity: Arc<RwLock<Decimal>>,
    current_equity: Arc<RwLock<Decimal>>,
    peak_equity: Arc<RwLock<Decimal>>,
    tripped_at: Arc<RwLock<Option<DateTime<Utc>>>>,
    day_key: RwLock<Option<NaiveDate>>,
    /// Optional sink fired after every durable transition (trip, reset,
    /// re-anchor, day roll). The host layer writes the snapshot to disk.
    persist_hook: Mutex<Option<PersistHook>>,
    /// Optional out-of-band notifier fired once on the false→true trip
    /// transition. The host layer pages the operator.
    trip_hook: Mutex<Option<TripHook>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig, initial_equity: Decimal) -> Self {
        Self {
            config: RwLock::new(config),
            is_tripped: AtomicBool::new(false),
            trip_reason: Arc::new(RwLock::new(None)),
            consecutive_errors: AtomicU32::new(0),
            day_start_equity: Arc::new(RwLock::new(initial_equity)),
            current_equity: Arc::new(RwLock::new(initial_equity)),
            peak_equity: Arc::new(RwLock::new(initial_equity)),
            tripped_at: Arc::new(RwLock::new(None)),
            // Seed the day anchor at construction: `None` made the FIRST
            // update_equity call roll and re-anchor day_start to the
            // incoming equity, silently discarding `initial_equity` as
            // the drawdown baseline — the breaker could never trip on a
            // fresh process's first update.
            day_key: RwLock::new(Some(Utc::now().date_naive())),
            persist_hook: Mutex::new(None),
            trip_hook: Mutex::new(None),
        }
    }

    /// Register the durability sink. Fires on every durable transition —
    /// never on `restore` (restoring is not a transition).
    pub fn set_persist_hook(&self, hook: PersistHook) {
        *self.persist_hook.lock() = Some(hook);
    }

    /// Register the out-of-band trip notifier. Fires once on the
    /// false→true transition only — re-trips while already halted do not
    /// re-notify (an operator ack of the first page must not spawn more).
    pub fn set_trip_hook(&self, hook: TripHook) {
        *self.trip_hook.lock() = Some(hook);
    }

    /// Serialize the current state for the durability sink.
    pub fn snapshot(&self) -> CircuitBreakerSnapshot {
        CircuitBreakerSnapshot {
            tripped: self.is_tripped.load(Ordering::SeqCst),
            trip_reason: self.trip_reason.read().clone(),
            tripped_at: *self.tripped_at.read(),
            day_start_equity: *self.day_start_equity.read(),
            current_equity: *self.current_equity.read(),
            peak_equity: *self.peak_equity.read(),
            day_key: *self.day_key.read(),
            consecutive_errors: self.consecutive_errors.load(Ordering::SeqCst),
        }
    }

    /// Restore a persisted snapshot at startup. Does not fire the sink —
    /// a restore is not a transition.
    pub fn restore(&self, snap: &CircuitBreakerSnapshot) {
        *self.day_start_equity.write() = snap.day_start_equity;
        *self.current_equity.write() = snap.current_equity;
        *self.peak_equity.write() = snap.peak_equity;
        // A snapshot predating day_key carries no anchor — keep the
        // restored baseline for today rather than re-anchoring on the
        // first update and discarding it.
        *self.day_key.write() = snap.day_key.or(Some(Utc::now().date_naive()));
        *self.trip_reason.write() = snap.trip_reason.clone();
        *self.tripped_at.write() = snap.tripped_at;
        self.consecutive_errors
            .store(snap.consecutive_errors, Ordering::SeqCst);
        self.is_tripped.store(snap.tripped, Ordering::SeqCst);
    }

    fn persist(&self) {
        let hook = self.persist_hook.lock().clone();
        if let Some(h) = hook {
            h(&self.snapshot());
        }
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("is_tripped", &self.is_tripped())
            .field("trip_reason", &self.trip_reason())
            .field("consecutive_errors", &self.consecutive_errors())
            .field("day_start_equity", &self.day_start_equity())
            .field("current_equity", &self.current_equity())
            .field("peak_equity", &self.peak_equity())
            .finish()
    }
}

impl CircuitBreaker {
    /// Check if trading is allowed right now.
    pub fn is_trading_allowed(&self) -> Result<(), String> {
        if self.is_tripped.load(Ordering::Relaxed) {
            let reason = self
                .trip_reason
                .read()
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            Err(format!("Circuit breaker TRIPPED: {}", reason))
        } else {
            Ok(())
        }
    }

    /// Trip circuit breaker manually or automatically.
    pub fn trip(&self, reason: impl Into<String>) {
        let r = reason.into();
        error!("🚨 CIRCUIT BREAKER TRIPPED! Reason: {}", r);
        let was_tripped;
        {
            *self.trip_reason.write() = Some(r.clone());
            *self.tripped_at.write() = Some(Utc::now());
            was_tripped = self.is_tripped.swap(true, Ordering::SeqCst);
        }
        self.persist();
        // Out-of-band alert fires only on the transition into halted —
        // `record_error` keeps re-tripping past the threshold and each
        // re-trip must not re-page.
        if !was_tripped {
            let hook = self.trip_hook.lock().clone();
            if let Some(h) = hook {
                h(&r);
            }
        }
    }

    /// Re-anchor the baseline to a new equity universe (e.g. paper<->live
    /// mode switch or paper-balance reset). Switching account universes is
    /// not a trading loss, so it must never trip: baseline, current, and
    /// peak all move together. The consecutive-error count resets with the
    /// universe, but a TRIP is sticky — only operator `reset()` clears it.
    /// Otherwise a live-mode drawdown halt could be dodged by flipping
    /// environments, and a restored trip would evaporate on the first
    /// equity feed after restart.
    pub fn reanchor_baseline(&self, equity: Decimal) {
        {
            *self.day_start_equity.write() = equity;
            *self.current_equity.write() = equity;
            *self.peak_equity.write() = equity;
            *self.day_key.write() = Some(Utc::now().date_naive());
            self.consecutive_errors.store(0, Ordering::SeqCst);
        }
        self.persist();
    }

    /// Reset after operator review. The drawdown baseline is re-anchored to
    /// the current equity so prior losses are not carried into the new window.
    pub fn reset(&self) {
        warn!("Circuit breaker manually RESET by operator");
        {
            let current = *self.current_equity.read();
            *self.day_start_equity.write() = current;
            *self.peak_equity.write() = current;
            *self.day_key.write() = Some(Utc::now().date_naive());
            *self.trip_reason.write() = None;
            *self.tripped_at.write() = None;
            self.consecutive_errors.store(0, Ordering::SeqCst);
            self.is_tripped.store(false, Ordering::SeqCst);
        }
        self.persist();
    }

    /// Record an execution error (e.g. unhedged leg, API timeout).
    pub fn record_error(&self, details: &str) {
        let count = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
        warn!(
            "Consecutive execution error #{} / {}: {}",
            count,
            self.config.read().max_consecutive_errors,
            details
        );
        if count >= self.config.read().max_consecutive_errors {
            self.trip(format!(
                "Exceeded maximum consecutive execution errors ({}/{}): {}",
                count,
                self.config.read().max_consecutive_errors,
                details
            ));
        }
    }

    /// Record a successful execution fill on all legs.
    pub fn record_success(&self) {
        self.consecutive_errors.store(0, Ordering::SeqCst);
    }

    /// Update equity and check drawdown limits. The baseline is anchored
    /// to the UTC day: the first update of a new day re-anchors
    /// `day_start_equity` so "daily drawdown" is actually a daily window
    /// rather than a since-process-start one.
    pub fn update_equity(&self, new_equity: Decimal) {
        let today = Utc::now().date_naive();
        let rolled = {
            *self.current_equity.write() = new_equity;
            let mut key = self.day_key.write();
            if *key != Some(today) {
                *key = Some(today);
                *self.day_start_equity.write() = new_equity;
                true
            } else {
                false
            }
        };

        {
            let mut peak = self.peak_equity.write();
            if new_equity > *peak {
                *peak = new_equity;
            }
        }

        let start = *self.day_start_equity.read();
        if start > dec!(0) && new_equity < start {
            let dollar_loss = start - new_equity;
            let loss_pct = (dollar_loss / start) * dec!(100);

            if dollar_loss >= self.config.read().max_daily_drawdown_usd {
                self.trip(format!(
                    "Daily dollar drawdown limit breached: -${} (max ${})",
                    dollar_loss,
                    self.config.read().max_daily_drawdown_usd
                ));
            } else if loss_pct >= self.config.read().max_daily_drawdown_pct {
                self.trip(format!(
                    "Daily percentage drawdown limit breached: -{}% (max {}%)",
                    loss_pct,
                    self.config.read().max_daily_drawdown_pct
                ));
            }
        }

        if rolled && !self.is_tripped() {
            // A trip already persisted itself; otherwise persist the new
            // baseline so a mid-day restart keeps the day's anchor.
            self.persist();
        }
    }

    /// Get current consecutive errors count
    pub fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors.load(Ordering::Relaxed)
    }

    /// Get configured max consecutive errors
    pub fn max_consecutive_errors(&self) -> u32 {
        self.config.read().max_consecutive_errors
    }

    /// Get configured max daily dollar drawdown
    pub fn max_daily_drawdown_usd(&self) -> Decimal {
        self.config.read().max_daily_drawdown_usd
    }

    /// Snapshot the live circuit-breaker limits (for settings UI).
    pub fn get_config(&self) -> CircuitBreakerConfig {
        self.config.read().clone()
    }

    /// Live-update the circuit-breaker limits without restarting.
    /// Tightening never trips by itself; the next equity update enforces it.
    pub fn update_config(&self, cfg: CircuitBreakerConfig) {
        *self.config.write() = cfg;
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

    #[test]
    fn test_trip_hook_fires_once_on_transition_only() {
        use std::sync::atomic::AtomicUsize;
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(100000.0));
        let calls = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(std::sync::Mutex::new(String::new()));
        {
            let calls = calls.clone();
            let last = last.clone();
            cb.set_trip_hook(Arc::new(move |reason| {
                calls.fetch_add(1, Ordering::SeqCst);
                *last.lock().unwrap() = reason.to_string();
            }));
        }

        cb.trip("first halt");
        cb.trip("re-trip while halted");
        cb.record_error("error one");
        cb.record_error("error two");
        cb.record_error("error three trips again");
        // One page for the whole halt — re-trips must not re-notify.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(last.lock().unwrap().as_str(), "first halt");

        // Operator reset re-arms the transition: the next trip notifies.
        cb.reset();
        cb.trip("second halt");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(last.lock().unwrap().as_str(), "second halt");
    }
}
