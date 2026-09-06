use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use tokio::time::sleep;
use tracing::{warn, debug};

/// Rate limit backoff and cached request coordinator.
/// Pattern reverse-engineered from high-frequency DEX arbitrage production tools.
#[derive(Debug, Clone)]
pub struct AccountRequestCoordinator {
    venue_name: String,
    min_interval: Duration,
    cache_ttl: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
    state: Arc<Mutex<CoordinatorState>>,
}

#[derive(Debug)]
struct CoordinatorState {
    last_request_time: Option<Instant>,
    backoff_until: Option<Instant>,
    consecutive_failures: u32,
    cached_data: Option<(Instant, String)>,
}

impl AccountRequestCoordinator {
    pub fn new(
        venue_name: impl Into<String>,
        min_interval_ms: u64,
        cache_ttl_ms: u64,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
    ) -> Self {
        Self {
            venue_name: venue_name.into(),
            min_interval: Duration::from_millis(min_interval_ms),
            cache_ttl: Duration::from_millis(cache_ttl_ms),
            backoff_base: Duration::from_millis(backoff_base_ms),
            backoff_max: Duration::from_millis(backoff_max_ms),
            state: Arc::new(Mutex::new(CoordinatorState {
                last_request_time: None,
                backoff_until: None,
                consecutive_failures: 0,
                cached_data: None,
            })),
        }
    }

    /// Pre-request check: Wait if needed, or return cached payload if within TTL.
    pub async fn acquire_or_cache(&self, allow_cache: bool) -> Result<Option<String>, anyhow::Error> {
        let now = Instant::now();
        let wait_dur;

        {
            let state = self.state.lock();
            // 1. Check active backoff
            if let Some(until) = state.backoff_until {
                if now < until {
                    let backoff_left = until - now;
                    if allow_cache {
                        if let Some((cached_at, ref payload)) = state.cached_data {
                            if now.duration_since(cached_at) < self.cache_ttl * 2 {
                                debug!(
                                    "[{}] In backoff ({:?} remaining), returning cached data",
                                    self.venue_name, backoff_left
                                );
                                return Ok(Some(payload.clone()));
                            }
                        }
                    }
                    anyhow::bail!(
                        "[{}] Rate limit backoff active for another {:?}",
                        self.venue_name,
                        backoff_left
                    );
                }
            }

            // 2. Check fresh cache
            if allow_cache {
                if let Some((cached_at, ref payload)) = state.cached_data {
                    if now.duration_since(cached_at) < self.cache_ttl {
                        return Ok(Some(payload.clone()));
                    }
                }
            }

            // 3. Throttle to min_interval
            wait_dur = if let Some(last) = state.last_request_time {
                let elapsed = now.duration_since(last);
                if elapsed < self.min_interval {
                    Some(self.min_interval - elapsed)
                } else {
                    None
                }
            } else {
                None
            };
        }

        if let Some(wait) = wait_dur {
            sleep(wait).await;
        }

        {
            let mut state = self.state.lock();
            state.last_request_time = Some(Instant::now());
        }

        Ok(None)
    }

    /// Record a successful HTTP response.
    pub fn record_success(&self, payload: String) {
        let mut state = self.state.lock();
        state.consecutive_failures = 0;
        state.backoff_until = None;
        state.cached_data = Some((Instant::now(), payload));
    }

    /// Record an error or 429 Rate Limit.
    pub fn record_failure(&self, is_rate_limit: bool) {
        let mut state = self.state.lock();
        state.consecutive_failures += 1;

        let exponent = state.consecutive_failures.min(6);
        let multiplier = 2u64.pow(exponent);
        let backoff = if is_rate_limit {
            (self.backoff_base * multiplier as u32).min(self.backoff_max)
        } else {
            self.backoff_base
        };

        let until = Instant::now() + backoff;
        state.backoff_until = Some(until);

        warn!(
            "[{}] API request failure #{} (rate_limit={}), backing off for {:?}",
            self.venue_name, state.consecutive_failures, is_rate_limit, backoff
        );
    }
}
