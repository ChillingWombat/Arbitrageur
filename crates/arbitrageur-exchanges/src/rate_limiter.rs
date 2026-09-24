use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, warn};

/// Rate limit backoff and cached request coordinator with path-level request coalescing.
/// Pattern reverse-engineered from high-frequency DEX arbitrage production tools (e.g. PopDEX / Lighter bot).
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
    /// Signed/order traffic gets its own throttle bucket so public-read
    /// pacing never silently delays order submission — while the venue-wide
    /// backoff applies to both.
    last_signed_request_time: Option<Instant>,
    backoff_until: Option<Instant>,
    consecutive_failures: u32,
    cached_data: Option<(Instant, String)>,
    cached_paths: HashMap<String, (Instant, String)>,
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
                last_signed_request_time: None,
                backoff_until: None,
                consecutive_failures: 0,
                cached_data: None,
                cached_paths: HashMap::new(),
            })),
        }
    }

    /// Pre-request check for a specific API path/query:
    /// Returns Some(cached_json) if fresh in cache (request coalescing), or throttles and permits request.
    pub async fn acquire_or_cache_path(
        &self,
        path: &str,
        allow_cache: bool,
    ) -> Result<Option<String>, anyhow::Error> {
        let now = Instant::now();
        let wait_dur;

        {
            let state = self.state.lock();
            // 1. Check active backoff
            if let Some(until) = state.backoff_until {
                if now < until {
                    let backoff_left = until - now;
                    if allow_cache {
                        if let Some((cached_at, ref payload)) = state.cached_paths.get(path) {
                            if now.duration_since(*cached_at) < self.cache_ttl * 3 {
                                debug!(
                                    "[{}] In backoff ({:?} remaining), returning cached path data for {}",
                                    self.venue_name, backoff_left, path
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

            // 2. Check fresh cache for this exact path
            if allow_cache {
                if let Some((cached_at, ref payload)) = state.cached_paths.get(path) {
                    if now.duration_since(*cached_at) < self.cache_ttl {
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

    /// Pre-request check: Wait if needed, or return cached payload if within TTL.
    pub async fn acquire_or_cache(
        &self,
        allow_cache: bool,
    ) -> Result<Option<String>, anyhow::Error> {
        self.acquire_or_cache_path("default", allow_cache).await
    }

    /// Pre-submit gate for SIGNED paths (orders, cancels, private POSTs).
    /// A venue-level backoff (including one written by a failed submit)
    /// blocks order submission — never serves cached data, since a signed
    /// request must always reach the venue. Throttles signed calls at
    /// `min_interval` on a bucket separate from public reads.
    pub async fn acquire_signed(&self) -> Result<(), anyhow::Error> {
        let wait_dur = {
            let state = self.state.lock();
            if let Some(until) = state.backoff_until {
                let now = Instant::now();
                if now < until {
                    anyhow::bail!(
                        "[{}] Rate limit backoff active for another {:?} — signed request refused",
                        self.venue_name,
                        until - now
                    );
                }
            }
            state
                .last_signed_request_time
                .and_then(|last| self.min_interval.checked_sub(last.elapsed()))
        };
        if let Some(wait) = wait_dur {
            sleep(wait).await;
        }
        self.state.lock().last_signed_request_time = Some(Instant::now());
        Ok(())
    }

    /// Record a successful HTTP response for a specific path.
    pub fn record_success_path(&self, path: &str, payload: String) {
        let mut state = self.state.lock();
        state.consecutive_failures = 0;
        state.backoff_until = None;
        state
            .cached_paths
            .insert(path.to_string(), (Instant::now(), payload.clone()));
        state.cached_data = Some((Instant::now(), payload));
    }

    /// Record a successful HTTP response.
    pub fn record_success(&self, payload: String) {
        self.record_success_path("default", payload);
    }

    /// Record an edge challenge (WAF CAPTCHA / IP flag). Unlike transient
    /// failures, retrying a challenge wall only extends the flag — cool down
    /// hard so the venue can recover. Subsequent callers bail at acquire
    /// (no log spam) until the cool-down expires.
    pub fn record_challenge(&self) {
        let mut state = self.state.lock();
        state.consecutive_failures += 1;
        state.backoff_until = Some(Instant::now() + Duration::from_secs(30 * 60));
        warn!(
            "[{}] edge challenge detected (WAF/CAPTCHA) — cooling down for 30m",
            self.venue_name
        );
    }
}

/// True when an HTTP response is an edge challenge wall (AWS WAF CAPTCHA
/// or similar IP flag) rather than a venue API answer. Retrying these is
/// counterproductive — callers must cool down, not spin.
pub fn is_waf_challenge(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body_start: &str,
) -> bool {
    let waf_header = headers
        .get("x-amzn-waf-action")
        .and_then(|v| v.to_str().ok())
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if waf_header {
        return true;
    }
    let htmlish = body_start.contains("<html")
        || body_start.contains("Human Verification")
        || body_start.contains("awsWaf")
        || body_start.contains("gokuProps");
    htmlish && (status == 403 || status == 405)
}

impl AccountRequestCoordinator {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_waf() -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-amzn-waf-action", "captcha".parse().unwrap());
        h
    }

    #[test]
    fn test_waf_challenge_detection() {
        let empty = reqwest::header::HeaderMap::new();
        assert!(is_waf_challenge(405, &headers_with_waf(), ""));
        assert!(is_waf_challenge(
            405,
            &empty,
            "<html><title>Human Verification</title>"
        ));
        assert!(is_waf_challenge(403, &empty, "window.gokuProps = {}"));
        assert!(!is_waf_challenge(429, &empty, ""));
        assert!(!is_waf_challenge(200, &empty, "{\"code\":200}"));
        assert!(!is_waf_challenge(405, &empty, "{\"code\":-4021}"));
    }

    #[test]
    fn test_challenge_cooldown_is_long() {
        let c = AccountRequestCoordinator::new("test", 200, 3000, 500, 10_000);
        c.record_challenge();
        let state = c.state.lock();
        let until = state.backoff_until.expect("cooldown set");
        assert!(until > Instant::now() + Duration::from_secs(29 * 60));
    }
}
