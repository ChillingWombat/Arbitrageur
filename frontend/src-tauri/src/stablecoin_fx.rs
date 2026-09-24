//! USD-stablecoin FX table — the single source of truth for converting
//! between USD-family quote currencies (USD, USDT, USDC, USDG, …).
//!
//! Three pairing policies (`EngineSettingsDto.usd_quote_mode`):
//! - `separate`: exact BASE-QUOTE identity — BTC-USDT and BTC-USDC are
//!   distinct instruments and never merge (no conversion needed).
//! - `parity`: every USD-family quote is assumed 1:1 with the dollar —
//!   the monitor's historical default.
//! - `fx_adjusted`: quotes merge, but prices are converted to real USD
//!   through this table before comparison. Stablecoin depeg risk is
//!   monitored continuously; a breached quote blocks pairs quoted in it
//!   (stablecoin-vs-USD books stay tradeable — that IS the depeg arb).
//!
//! Rate sources, in precedence order:
//! 1. CoinGecko `/simple/price` — global cross-venue truth (~90s cadence).
//! 2. Uniswap V3 QuoterV2 on Ethereum mainnet (via the `uniswap` EVM-AMM
//!    spec's public RPC) — relative-rate backoff when CoinGecko is down,
//!    anchored on USDC.
//! 3. Venue-local stable/stable markets — used for single-venue paths
//!    (triangle cycles, venue equity) where the venue's own FX is the
//!    honest rate.
//! 4. Parity fallback: never-observed quotes read 1.0; a disconnected feed
//!    keeps its last observation and converges toward 1.0 exponentially.
//!
//! Convergence half-life: 24h. Historical fiat-backed depegs recovered in
//! <24h (USDT 2022-05), ~1 day (FDUSD 2025-04) and ~3 days (USDC SVB
//! 2023-03); flash dislocations like USDe 2026-09 snap back in minutes.
//! A 24h half-life keeps a real multi-day depeg visible for its typical
//! duration while a dead feed's residual fades ~0.6%/day-of-deviation —
//! "very very slow" as specified, never crossing below parity.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::symbol::is_usd_family;
use parking_lot::Mutex;
use serde::Serialize;

pub const USD_QUOTE_MODE_SEPARATE: &str = "separate";
pub const USD_QUOTE_MODE_PARITY: &str = "parity";
pub const USD_QUOTE_MODE_FX_ADJUSTED: &str = "fx_adjusted";

/// Normalize a stored/requested mode string. Unknown values fall back to
/// the configured default (fx_adjusted) — save-side validation rejects
/// anything outside the three spellings before this ever sees it.
pub fn normalize_usd_quote_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        USD_QUOTE_MODE_SEPARATE => USD_QUOTE_MODE_SEPARATE,
        USD_QUOTE_MODE_PARITY => USD_QUOTE_MODE_PARITY,
        _ => USD_QUOTE_MODE_FX_ADJUSTED,
    }
}

/// One observed USD rate for a stablecoin quote.
#[derive(Clone, Debug)]
pub struct FxObservation {
    /// USD per 1 unit of the quote coin at observation time.
    pub rate: f64,
    pub at_ms: i64,
    pub source: String,
}

/// IPC row for one tracked quote (System Settings → Execution display).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StablecoinQuoteDto {
    pub quote: String,
    /// Effective USD rate right now (post stale-convergence).
    pub effective_rate: f64,
    /// Signed deviation from parity, bps. Negative = below peg.
    pub deviation_bps: f64,
    /// "ok" | "warn" | "halted" | "unobserved"
    pub status: String,
    pub last_update_ms: Option<i64>,
    pub source: Option<String>,
    /// True once the observation is older than the freshness window and
    /// the rate is converging back toward 1.0.
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StablecoinHealthDto {
    pub mode: String,
    pub quotes: Vec<StablecoinQuoteDto>,
    /// Quotes past the halt threshold — pairs quoted in these are blocked
    /// (except the stablecoin-vs-USD books that constitute the depeg arb).
    pub halted_quotes: Vec<String>,
    /// True while every external source is unreachable — rates are the
    /// converging-to-parity fallback, not live truth.
    pub feed_stale: bool,
    pub generated_ts_ms: i64,
}

/// CoinGecko id per tracked quote. Wrong/absent ids simply never resolve —
/// the quote keeps its prior observation or the parity baseline.
const COINGECKO_IDS: &[(&str, &str)] = &[
    ("USDT", "tether"),
    ("USDC", "usd-coin"),
    ("USDG", "global-dollar"),
    ("DAI", "dai"),
    ("USDE", "ethena-usde"),
    ("FDUSD", "first-digital-usd"),
    ("TUSD", "true-usd"),
    ("PYUSD", "paypal-usd"),
    ("USDS", "usds"),
];

/// Quotes always shown in the health table even before observation.
const DISPLAY_QUOTES: &[&str] = &["USD", "USDT", "USDC", "USDG"];

/// Observation sanity band: outside this a feed print is corrupt, not a
/// depeg (the 2023 SVB print bottomed at 0.87; even UST took days to leave
/// the band). Rejected prints keep the prior observation.
const RATE_MIN: f64 = 0.10;
const RATE_MAX: f64 = 10.0;

pub struct StablecoinFx {
    /// Global cross-venue observations, keyed by uppercase quote ticker.
    global: Mutex<HashMap<String, FxObservation>>,
    /// Venue-local rates: venue -> quote -> observation. Single-venue
    /// paths (triangle cycles, venue equity) prefer these — the venue's
    /// own stable/stable markets are the honest conversion.
    venue: Mutex<HashMap<String, HashMap<String, FxObservation>>>,
    http: reqwest::Client,
    /// Exponential half-life of the drift back to parity once an
    /// observation ages past `stale_after_ms`. 24h — see module doc.
    convergence_half_life_ms: i64,
    /// Freshness window: at/under this age the recorded rate is used
    /// verbatim; past it the rate converges toward 1.0.
    stale_after_ms: i64,
    /// Set while the global refresh is failing — health surface flag.
    feed_down: Mutex<bool>,
}

impl StablecoinFx {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            global: Mutex::new(HashMap::new()),
            venue: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            convergence_half_life_ms: 86_400_000,
            stale_after_ms: 300_000,
            feed_down: Mutex::new(false),
        })
    }

    /// Test seam: custom convergence parameters.
    #[cfg(test)]
    pub fn with_timing(half_life_ms: i64, stale_after_ms: i64) -> Arc<Self> {
        let fx = Self::new();
        // Fresh Arc — no other owners exist yet.
        let inner = Arc::into_inner(fx).expect("fresh Arc has no other owners");
        Arc::new(Self {
            convergence_half_life_ms: half_life_ms,
            stale_after_ms,
            ..inner
        })
    }

    fn key(quote: &str) -> String {
        quote.trim().to_uppercase()
    }

    /// Record a global (cross-venue) observation. Out-of-band prints are
    /// rejected — a corrupt feed must never move the table.
    pub fn record_global(&self, quote: &str, rate: f64, source: &str, at_ms: i64) -> bool {
        let q = Self::key(quote);
        if q == "USD"
            || !is_usd_family(&q)
            || !rate.is_finite()
            || !(RATE_MIN..=RATE_MAX).contains(&rate)
        {
            return false;
        }
        self.global.lock().insert(
            q,
            FxObservation {
                rate,
                at_ms,
                source: source.to_string(),
            },
        );
        true
    }

    /// Record a venue-local observation derived from that venue's own
    /// stable/stable market prices.
    pub fn record_venue(&self, venue: &str, quote: &str, rate: f64, at_ms: i64) -> bool {
        let q = Self::key(quote);
        if q == "USD"
            || !is_usd_family(&q)
            || !rate.is_finite()
            || !(RATE_MIN..=RATE_MAX).contains(&rate)
        {
            return false;
        }
        self.venue
            .lock()
            .entry(venue.trim().to_lowercase())
            .or_default()
            .insert(
                q,
                FxObservation {
                    rate,
                    at_ms,
                    source: "venue".to_string(),
                },
            );
        true
    }

    /// Effective rate of one observation at `now_ms`: verbatim inside the
    /// freshness window, then exponential decay toward parity.
    fn effective(&self, obs: &FxObservation, now_ms: i64) -> f64 {
        let stale_ms = (now_ms - obs.at_ms - self.stale_after_ms).max(0);
        if stale_ms == 0 || obs.rate == 1.0 {
            return obs.rate;
        }
        let half_lives = stale_ms as f64 / self.convergence_half_life_ms as f64;
        1.0 + (obs.rate - 1.0) * 0.5f64.powf(half_lives)
    }

    fn effective_obs<'a>(
        &self,
        map: &'a HashMap<String, FxObservation>,
        quote: &str,
        now_ms: i64,
    ) -> Option<(f64, &'a FxObservation)> {
        map.get(quote).map(|o| (self.effective(o, now_ms), o))
    }

    /// Cross-venue USD rate for `quote`. USD is always 1.0; USD-family
    /// quotes never observed read the parity baseline 1.0; non-family
    /// quotes return `None` (callers must not merge them at all).
    pub fn usd_rate(&self, quote: &str, now_ms: i64) -> Option<f64> {
        let q = Self::key(quote);
        if q == "USD" {
            return Some(1.0);
        }
        if !is_usd_family(&q) {
            return None;
        }
        Some(
            self.effective_obs(&self.global.lock(), &q, now_ms)
                .map(|(r, _)| r)
                .unwrap_or(1.0),
        )
    }

    /// Single-venue USD rate: the venue's own stable-market observation
    /// wins while fresh; falls back to the global table.
    pub fn venue_usd_rate(&self, venue: &str, quote: &str, now_ms: i64) -> Option<f64> {
        let q = Self::key(quote);
        if q == "USD" {
            return Some(1.0);
        }
        if !is_usd_family(&q) {
            return None;
        }
        let v = self.venue.lock();
        if let Some(map) = v.get(&venue.trim().to_lowercase()) {
            if let Some((r, _)) = self.effective_obs(map, &q, now_ms) {
                return Some(r);
            }
        }
        drop(v);
        self.usd_rate(&q, now_ms)
    }

    /// Rate used for price conversion in fx_adjusted mode: USD-family
    /// quotes get the effective rate, everything else (including unknown /
    /// non-family) reads parity — a missing observation is a 1:1 baseline,
    /// never a fabricated divergence.
    pub fn fx_factor(&self, venue: Option<&str>, quote: &str, now_ms: i64) -> f64 {
        match venue {
            Some(v) => self.venue_usd_rate(v, quote, now_ms),
            None => self.usd_rate(quote, now_ms),
        }
        .unwrap_or(1.0)
    }

    /// Signed deviation of the effective rate from parity, bps.
    pub fn deviation_bps(&self, quote: &str, now_ms: i64) -> Option<f64> {
        self.usd_rate(quote, now_ms).map(|r| (r - 1.0) * 10_000.0)
    }

    /// Depeg gate: the quote's effective deviation breaches `halt_bps`.
    /// USD itself and non-family quotes never halt — non-family books
    /// are never merged into USD pairs so there is nothing to protect.
    pub fn is_halted(&self, quote: &str, now_ms: i64, halt_bps: f64) -> bool {
        self.deviation_bps(quote, now_ms)
            .map(|d| d.abs() >= halt_bps)
            .unwrap_or(false)
    }

    /// One health row per display/tracked quote for the settings UI.
    pub fn health(
        &self,
        mode: &str,
        now_ms: i64,
        warn_bps: f64,
        halt_bps: f64,
    ) -> StablecoinHealthDto {
        let global = self.global.lock();
        let mut quotes: Vec<StablecoinQuoteDto> = Vec::new();
        let mut seen: Vec<String> = DISPLAY_QUOTES.iter().map(|s| s.to_string()).collect();
        for k in global.keys() {
            if !seen.contains(k) {
                seen.push(k.clone());
            }
        }
        for q in &seen {
            if q == "USD" {
                quotes.push(StablecoinQuoteDto {
                    quote: q.clone(),
                    effective_rate: 1.0,
                    deviation_bps: 0.0,
                    status: "ok".into(),
                    last_update_ms: None,
                    source: Some("fiat anchor".into()),
                    stale: false,
                });
                continue;
            }
            match global.get(q) {
                Some(obs) => {
                    let eff = self.effective(obs, now_ms);
                    let dev = (eff - 1.0) * 10_000.0;
                    let stale = now_ms - obs.at_ms > self.stale_after_ms;
                    let status = if dev.abs() >= halt_bps {
                        "halted"
                    } else if dev.abs() >= warn_bps {
                        "warn"
                    } else {
                        "ok"
                    };
                    quotes.push(StablecoinQuoteDto {
                        quote: q.clone(),
                        effective_rate: eff,
                        deviation_bps: dev,
                        status: status.into(),
                        last_update_ms: Some(obs.at_ms),
                        source: Some(obs.source.clone()),
                        stale,
                    });
                }
                None => quotes.push(StablecoinQuoteDto {
                    quote: q.clone(),
                    effective_rate: 1.0,
                    deviation_bps: 0.0,
                    status: "unobserved".into(),
                    last_update_ms: None,
                    source: None,
                    stale: true,
                }),
            }
        }
        let halted_quotes = quotes
            .iter()
            .filter(|q| q.status == "halted")
            .map(|q| q.quote.clone())
            .collect();
        StablecoinHealthDto {
            mode: mode.to_string(),
            quotes,
            halted_quotes,
            feed_stale: *self.feed_down.lock(),
            generated_ts_ms: now_ms,
        }
    }

    /// One refresh cycle: CoinGecko primary; when the global table goes
    /// stale the Uniswap backoff refills the big two (USDT/USDC/DAI)
    /// from on-chain quotes.
    pub async fn refresh_global(&self) {
        let cg = self.refresh_coingecko().await;
        let backoff_n = match &cg {
            Ok(n) if *n > 0 => 0,
            _ => self.refresh_uniswap_backoff().await,
        };
        self.finish_global_refresh(cg, backoff_n);
    }

    /// Update `feed_down` after a refresh attempt. The flag clears when
    /// any source landed rows; it sets only when the primary produced
    /// nothing usable AND the on-chain backoff produced nothing either —
    /// "down" means no external truth is flowing, not "CG specifically
    /// failed" (a healthy backoff must not read as feed-stale).
    fn finish_global_refresh(&self, cg: anyhow::Result<usize>, backoff_n: usize) {
        match cg {
            Ok(n) if n > 0 => {
                *self.feed_down.lock() = false;
                return;
            }
            Ok(_) => {
                tracing::warn!("stablecoin fx: CoinGecko returned no usable prices");
            }
            Err(e) => {
                tracing::warn!("stablecoin fx: CoinGecko refresh failed: {e}");
            }
        }
        *self.feed_down.lock() = backoff_n == 0;
    }

    async fn refresh_coingecko(&self) -> anyhow::Result<usize> {
        let ids: Vec<&str> = COINGECKO_IDS.iter().map(|(_, id)| *id).collect();
        let url = format!(
            "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
            ids.join(",")
        );
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("CoinGecko HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut n = 0;
        for (quote, id) in COINGECKO_IDS {
            let rate = v
                .get(*id)
                .and_then(|c| c.get("usd"))
                .and_then(|p| p.as_f64());
            if let Some(r) = rate {
                if self.record_global(quote, r, "coingecko", now_ms) {
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// On-chain backoff: Uniswap V3 QuoterV2 via the `uniswap` spec's
    /// public RPC. Stable/stable books are relative — anchored on USDC's
    /// current effective rate (parity when unobserved), documented in the
    /// observation source label. Returns the count of rates recorded.
    async fn refresh_uniswap_backoff(&self) -> usize {
        let Some(adapter) =
            arbitrageur_exchanges::evm_amm::EvmAmmAdapter::for_venue("uniswap", None)
        else {
            return 0;
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        let usdc_anchor = self.usd_rate("USDC", now_ms).unwrap_or(1.0);
        let mut n = 0;
        // USDC-USDT book: 1 USDC = price USDT  →  rate_usdt = usdc/price.
        if let Ok(t) = adapter.fetch_ticker("USDC-USDT").await {
            let bid = t.bid.to_string().parse::<f64>().ok();
            let ask = t.ask.to_string().parse::<f64>().ok();
            if let (Some(b), Some(a)) = (bid, ask) {
                let mid = (b + a) / 2.0;
                if mid.is_finite() && mid > 0.0 {
                    n += self.record_global("USDT", usdc_anchor / mid, "uniswap", now_ms) as usize;
                }
            }
        }
        // DAI-USDC book: 1 DAI = price USDC → rate_dai = price * usdc.
        if let Ok(t) = adapter.fetch_ticker("DAI-USDC").await {
            let bid = t.bid.to_string().parse::<f64>().ok();
            let ask = t.ask.to_string().parse::<f64>().ok();
            if let (Some(b), Some(a)) = (bid, ask) {
                let mid = (b + a) / 2.0;
                if mid.is_finite() && mid > 0.0 {
                    n += self.record_global("DAI", mid * usdc_anchor, "uniswap", now_ms) as usize;
                }
            }
        }
        // USDC itself has no on-chain USD leg — its last/parity value
        // stands; a genuine USDC depeg still surfaces via CG or venues.
        n
    }

    /// Refresh one venue's local rate table from its stable/stable market
    /// prices. `pairs` carries (base_quote, quote_quote, mid) triples where
    /// both sides are USD-family — e.g. ("USDC", "USDT", 1.0004).
    /// Anchored on literal-USD legs (rate 1.0) and the global table.
    pub fn refresh_venue_rates(
        &self,
        venue: &str,
        pairs: &[(String, String, f64)],
        now_ms: i64,
    ) -> usize {
        // Iterative solver: rate[base] = mid * rate[quote]. Seeds come from
        // literal USD and the current global table; two passes cover the
        // longest stable→stable→USD chain venues actually list.
        let mut rates: HashMap<String, f64> = HashMap::new();
        rates.insert("USD".into(), 1.0);
        for (b, q, mid) in pairs {
            if *mid > 0.0 && mid.is_finite() {
                if let Some(gr) = self.usd_rate(q, now_ms) {
                    rates.entry(Self::key(b)).or_insert(*mid * gr);
                }
                if let Some(gr) = self.usd_rate(b, now_ms) {
                    rates.entry(Self::key(q)).or_insert(gr / *mid);
                }
            }
        }
        for _ in 0..3 {
            let mut grew = false;
            for (b, q, mid) in pairs {
                if !(*mid > 0.0 && mid.is_finite()) {
                    continue;
                }
                if let Some(qr) = rates.get(&Self::key(q)).copied() {
                    grew |= rates.insert(Self::key(b), *mid * qr).is_none();
                }
                if let Some(br) = rates.get(&Self::key(b)).copied() {
                    grew |= rates.insert(Self::key(q), br / *mid).is_none();
                }
            }
            if !grew {
                break;
            }
        }
        let mut n = 0;
        for (q, r) in rates {
            if q != "USD" && self.record_venue(venue, &q, r, now_ms) {
                n += 1;
            }
        }
        n
    }

    /// Drop venue-local observations (environment transitions): testnet
    /// stable/stable prices are not mainnet truth. The global table is
    /// venue-independent and stays.
    pub fn clear_venue(&self) {
        self.venue.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> Arc<StablecoinFx> {
        StablecoinFx::new()
    }

    #[test]
    fn usd_is_always_parity() {
        let f = fx();
        assert_eq!(f.usd_rate("USD", 0), Some(1.0));
        assert_eq!(f.usd_rate("usd", 123), Some(1.0));
    }

    #[test]
    fn unobserved_stable_reads_parity_baseline() {
        let f = fx();
        // Per policy: before any feed lands the baseline is 1:1.
        assert_eq!(f.usd_rate("USDT", 0), Some(1.0));
        assert_eq!(f.deviation_bps("USDC", 0), Some(0.0));
    }

    #[test]
    fn non_family_quotes_never_merge() {
        let f = fx();
        assert_eq!(f.usd_rate("EUR", 0), None);
        assert_eq!(f.usd_rate("BTC", 0), None);
        assert!(!f.is_halted("EUR", 0, 100.0));
    }

    #[test]
    fn fresh_observation_reads_verbatim() {
        let f = fx();
        assert!(f.record_global("USDT", 0.9990, "test", 1_000_000));
        assert_eq!(f.usd_rate("USDT", 1_000_000), Some(0.9990));
        let dev = f.deviation_bps("USDT", 1_000_000).unwrap();
        assert!((dev - -10.0).abs() < 1e-9, "expected ~-10bps got {dev}");
    }

    #[test]
    fn corrupt_prints_are_rejected() {
        let f = fx();
        assert!(f.record_global("USDT", 0.999, "test", 1000));
        // A wild misprint keeps the prior observation.
        assert!(!f.record_global("USDT", 0.001, "test", 2000));
        assert!(!f.record_global("USDT", f64::NAN, "test", 2000));
        assert_eq!(f.usd_rate("USDT", 3000), Some(0.999));
    }

    #[test]
    fn stale_rates_converge_to_parity_slowly() {
        let f = StablecoinFx::with_timing(86_400_000, 300_000);
        f.record_global("USDT", 0.96, "test", 0); // 4% below peg
                                                  // Fresh: verbatim.
        let fresh = f.usd_rate("USDT", 200_000).unwrap();
        assert_eq!(fresh, 0.96);
        // One half-life of staleness: deviation halves (0.96 → 0.98).
        let one_hl = f.usd_rate("USDT", 300_000 + 86_400_000).unwrap();
        assert!((one_hl - 0.98).abs() < 1e-9, "expected ~0.98 got {one_hl}");
        // Four half-lives: deviation shrinks to 1/16 (−400bps → −25bps) —
        // within 25bps of parity, still below it.
        let four = f.usd_rate("USDT", 300_000 + 4 * 86_400_000).unwrap();
        assert!((four - 0.9975).abs() < 1e-9 && four < 1.0, "got {four}");
    }

    #[test]
    fn halt_threshold_gates_on_deviation() {
        let f = fx();
        f.record_global("USDE", 0.985, "test", 1000); // -150bps
        assert!(f.is_halted("USDE", 2000, 100.0));
        assert!(!f.is_halted("USDE", 2000, 200.0));
        assert!(!f.is_halted("USDT", 2000, 100.0));
        assert!(!f.is_halted("USD", 2000, 100.0));
    }

    #[test]
    fn venue_rates_prefer_local_then_global() {
        let f = fx();
        f.record_global("USDT", 0.999, "coingecko", 1000);
        f.record_venue("binance", "USDT", 0.998, 1500);
        assert_eq!(f.venue_usd_rate("binance", "USDT", 2000), Some(0.998));
        assert_eq!(f.venue_usd_rate("kraken", "USDT", 2000), Some(0.999));
        assert_eq!(f.venue_usd_rate("binance", "USDC", 2000), Some(1.0));
    }

    #[test]
    fn venue_solver_chains_through_stable_pairs() {
        let f = fx();
        // USDC/USD leg pins USDC; USDT/USDC pins USDT through it.
        let n = f.refresh_venue_rates(
            "kraken",
            &[
                ("USDC".into(), "USD".into(), 0.9995),
                ("USDT".into(), "USDC".into(), 1.0002),
            ],
            1000,
        );
        assert_eq!(n, 2);
        let usdt = f.venue_usd_rate("kraken", "USDT", 2000).unwrap();
        assert!((usdt - 0.9995 * 1.0002).abs() < 1e-9);
    }

    #[test]
    fn health_marks_halted_and_unobserved() {
        let f = fx();
        f.record_global("USDT", 0.98, "test", 1000);
        let h = f.health(USD_QUOTE_MODE_FX_ADJUSTED, 2000, 50.0, 100.0);
        let usdt = h.quotes.iter().find(|q| q.quote == "USDT").unwrap();
        assert_eq!(usdt.status, "halted");
        assert_eq!(h.halted_quotes, vec!["USDT".to_string()]);
        let usdc = h.quotes.iter().find(|q| q.quote == "USDC").unwrap();
        assert_eq!(usdc.status, "unobserved");
    }

    #[test]
    fn clear_venue_drops_local_keeps_global() {
        let f = fx();
        // Testnet-local and global prints diverge; an environment switch
        // must not carry the demo price into live reads.
        f.record_global("USDT", 0.999, "coingecko", 1000);
        f.record_venue("binance", "USDT", 0.50, 1500);
        assert_eq!(f.venue_usd_rate("binance", "USDT", 2000), Some(0.50));
        f.clear_venue();
        // Local table empty → venue reads fall back to the global rate.
        assert_eq!(f.venue_usd_rate("binance", "USDT", 2000), Some(0.999));
        assert_eq!(f.usd_rate("USDT", 2000), Some(0.999));
    }

    #[test]
    fn successful_backoff_clears_feed_down() {
        let f = fx();
        let down = |f: &Arc<StablecoinFx>| {
            f.health(USD_QUOTE_MODE_FX_ADJUSTED, 0, 50.0, 100.0)
                .feed_stale
        };
        // Primary lands: feed healthy.
        f.finish_global_refresh(Ok(9), 0);
        assert!(!down(&f));
        // Primary fails, backoff also dead: genuinely down.
        f.finish_global_refresh(Err(anyhow::anyhow!("cg 429")), 0);
        assert!(down(&f));
        // Primary still failing but on-chain backoff lands rates:
        // truth is flowing, so the surface must not read "feed down".
        f.finish_global_refresh(Err(anyhow::anyhow!("cg 429")), 2);
        assert!(!down(&f));
        // Both dead again → down returns.
        f.finish_global_refresh(Ok(0), 0);
        assert!(down(&f));
    }
}
