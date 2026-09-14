use crate::state::{
    AppEngineState, EngineSettingsDto, ExecutionLogEntry, TickerStrategyConfigDto, VenueBalanceInfo,
};
use crate::state_store::{AccountProfile, StrategyAccountRoute};
use crate::vault::{CredentialPresence, ExchangeCredentials, VaultStatus};
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::quote::OrderBook;
use arbitrager_core::types::{dec, Decimal, Side, VenueId};
use arbitrager_execution::coordinator::ArbitrageExecutionResult;
use arbitrager_execution::smart_routing::SmartOrderRouter;
use arbitrager_execution::virtual_ledger::StrategyMetrics;
use arbitrager_math::calibration::WfoOptimizationResult;
use arbitrager_math::dislocation::{compute_deviation_series, BaselineMode, MidSample};
use arbitrager_math::funding_carry::evaluate_funding_carry;
use arbitrager_math::optimal_thresholds::compute_bertram_thresholds;
use arbitrager_math::ou_process::OuModelParams;
use arbitrager_math::spatial_spread::evaluate_spatial_spread;
use arbitrager_risk::margin_monitor::{MarginHealthStatus, MarginMonitor, MarginMonitorConfig};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

const LIVE_EXECUTION_DISABLED: &str = "Live execution is disabled until an authenticated adapter can sign, submit, and reconcile orders";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SystemStatusDto {
    pub is_live_mode: bool,
    pub is_paused: bool,
    pub total_realized_pnl: f64,
    pub total_trades_count: usize,
    pub successful_trades_count: usize,
    pub circuit_breaker_status: String,
    pub circuit_breaker_tripped: bool,
    pub current_drawdown_usd: f64,
    pub max_drawdown_usd: f64,
    pub consecutive_failures: u32,
    pub active_venues: Vec<VenueStatusDto>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueStatusDto {
    pub id: String,
    pub name: String,
    pub market_type: String,
    pub status: String,
    /// Measured round-trip to the venue's public market-metadata endpoint.
    /// `None` when the venue is not admitted or the probe failed — never
    /// fabricated.
    pub latency_ms: Option<u64>,
    pub fee_bps: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueVolumesDto {
    pub venue_id: String,
    /// 24h notional volume in quote currency (~USD), keyed by the venue's
    /// raw asset name (e.g. "BTC", "kPEPE"). Only assets the venue actually
    /// reports are included.
    pub volumes: HashMap<String, f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueOrderBookDto {
    pub venue_id: String,
    pub venue_name: String,
    pub symbol: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub spread: f64,
    pub spread_bps: f64,
    pub mid_price: f64,
    pub total_bid_depth: f64,
    pub total_ask_depth: f64,
    pub vwap_bid: f64,
    pub vwap_ask: f64,
    pub funding_rate_8h_pct: f64,
    pub bids: Vec<[f64; 2]>,
    pub asks: Vec<[f64; 2]>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpatialOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub gross_spread: f64,
    pub gross_spread_bps: f64,
    pub fee_bps: f64,
    pub net_spread_bps: f64,
    pub order_qty: f64,
    pub estimated_profit_usd: f64,
    pub is_executable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FundingOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub long_venue: String,
    pub short_venue: String,
    pub net_rate_8h_pct: f64,
    pub net_apr_pct: f64,
    pub net_apy_pct: f64,
    pub round_trip_fee_bps: f64,
    pub break_even_hours: f64,
    pub estimated_daily_income_usd: f64,
    pub trade_size_usd: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpotOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub gross_spread_bps: f64,
    pub buy_fee_bps: f64,
    pub sell_fee_bps: f64,
    pub transfer_cost_usd: f64,
    pub net_spread_bps: f64,
    pub net_profit_usd: f64,
    pub available_depth_usd: f64,
    pub estimated_latency_ms: u64,
    pub is_executable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpotExecutionParams {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub trade_size_usd: f64,
    pub max_slippage_bps: f64,
    #[serde(default)]
    pub net_spread_bps: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PredictionOpportunityDto {
    pub id: String,
    pub market_title: String,
    pub venue_a: String,
    pub venue_b: String,
    pub strategy: String,
    pub buy_yes_venue: String,
    pub buy_no_venue: String,
    pub yes_price_cents: f64,
    pub no_price_cents: f64,
    pub total_cost_cents: f64,
    pub guaranteed_payout_cents: f64,
    pub net_profit_cents: f64,
    pub roi_pct: f64,
    pub has_arbitrage: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StatArbStepResultDto {
    pub step: usize,
    pub price_y: f64,
    pub price_x: f64,
    pub dynamic_hedge_ratio: f64,
    pub intercept_alpha: f64,
    pub residual_spread: f64,
    pub spread_variance: f64,
    pub z_score: f64,
    pub signal: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BertramThresholdDto {
    pub entry_long: f64,
    pub exit_long: f64,
    pub entry_short: f64,
    pub exit_short: f64,
    pub optimal_distance: f64,
    pub expected_return_per_cycle: f64,
    pub estimated_trades_per_year: f64,
    pub estimated_sharpe: f64,
    pub half_life: f64,
    pub theta: f64,
    pub mu: f64,
    pub sigma: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OuFitDto {
    pub theta: f64,
    pub mu: f64,
    pub sigma: f64,
    pub half_life: f64,
    pub stationary_std: f64,
    pub r_squared: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RiskOverviewDto {
    pub circuit_breaker_state: String,
    pub is_halted: bool,
    pub current_drawdown_usd: f64,
    pub max_drawdown_limit_usd: f64,
    pub drawdown_pct: f64,
    pub consecutive_failures: u32,
    pub max_failures_allowed: u32,
    pub total_portfolio_value_usd: f64,
    pub net_deltas: Vec<NetDeltaDto>,
    pub margin_health: MarginHealthDto,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetDeltaDto {
    pub asset: String,
    pub net_quantity: f64,
    pub net_exposure_usd: f64,
    pub delta_limit_usd: f64,
    pub is_neutral: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MarginHealthDto {
    pub maintenance_margin_req_usd: f64,
    pub margin_balance_usd: f64,
    pub margin_ratio_pct: f64,
    pub liquidation_distance_pct: f64,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SimStepResultDto {
    pub cycle: usize,
    pub opportunity_found: bool,
    pub execution_success: bool,
    pub trade_type: String,
    pub direction: String,
    pub venue_a: String,
    pub venue_b: String,
    pub spread_bps: f64,
    pub realized_profit_usd: f64,
    pub total_cumulative_pnl_usd: f64,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TickerMarketQuoteDto {
    pub symbol: String,
    pub base: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub mid_price: f64,
    pub spread_bps: f64,
    pub best_buy_venue: String,
    pub best_sell_venue: String,
    pub fee_bps: f64,
    pub base_price: f64,
}

pub(crate) fn venue_fee_bps(vid: &str) -> Decimal {
    match vid.to_lowercase().as_str() {
        "hyperliquid" => dec!(2.0),
        "lighter" => dec!(1.5),
        "vanta" => dec!(3.0),
        "robinhood" => dec!(2.5),
        "arcus" => dec!(1.5),
        "popdex" => dec!(2.0),
        "polymarket" => dec!(0.0),
        "kalshi" => dec!(10.0),
        _ => dec!(2.0),
    }
}

fn validate_market_symbol(symbol: &str) -> Result<(), String> {
    if symbol.is_empty() || symbol.len() > 64 {
        return Err("Invalid market symbol: must be 1-64 characters".into());
    }
    if symbol != symbol.trim() {
        return Err("Invalid market symbol: no leading/trailing whitespace".into());
    }
    if !symbol
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
    {
        return Err(
            "Invalid market symbol: only ASCII alphanumeric, '-', '_', '/', '.' allowed".into(),
        );
    }
    if !symbol.chars().next().unwrap_or(' ').is_ascii_alphanumeric() || symbol.contains("..") {
        return Err(
            "Invalid market symbol: must start with an alphanumeric and not contain '..'".into(),
        );
    }
    let has_empty_segment = symbol.split(['-', '/', '_']).any(|seg| seg.is_empty());
    if has_empty_segment {
        return Err("Invalid market symbol: empty base/quote segment".into());
    }
    Ok(())
}

fn validate_adapter_book(
    book: &OrderBook,
    expected_venue: &VenueId,
    expected_symbol: &str,
) -> Result<(), String> {
    if &book.venue != expected_venue {
        return Err(format!(
            "order book venue '{}' does not match expected venue '{}'",
            book.venue, expected_venue
        ));
    }
    if book.symbol != expected_symbol {
        return Err(format!(
            "order book symbol '{}' does not match requested symbol '{}'",
            book.symbol, expected_symbol
        ));
    }
    book.validate_for_execution()
}

// ---------------------------------------------------------------------------
// Implementation functions (pure, unit-testable without Tauri State wrapper)
// ---------------------------------------------------------------------------

/// All venue ids currently enabled — engine flag OR an enabled account
/// profile. Drives which venues the status poll latency-probes.
fn venue_enabled_ids(state: &AppEngineState, s: &crate::state::EngineSettingsDto) -> Vec<String> {
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    let mut out = Vec::new();
    let mut push = |id: &str, on: bool| {
        if on {
            out.push(id.to_string());
        }
    };
    push("hyperliquid", s.hyperliquid_enabled || account_enabled("hyperliquid"));
    push("lighter", s.lighter_enabled || account_enabled("lighter"));
    push("robinhood", s.robinhood_enabled || account_enabled("robinhood"));
    push("vanta", s.vanta_enabled || account_enabled("vanta"));
    push("arcus", s.arcus_enabled || account_enabled("arcus"));
    push("popdex", s.popdex_enabled || account_enabled("popdex"));
    push(
        "kraken",
        s.kraken_enabled
            || s.kraken_spot_enabled
            || s.kraken_futures_enabled
            || account_enabled("kraken_spot")
            || account_enabled("kraken_futures"),
    );
    push("polymarket", s.polymarket_enabled || account_enabled("polymarket"));
    push("kalshi", s.kalshi_enabled || account_enabled("kalshi"));
    push("decibel", s.decibel_enabled || account_enabled("decibel"));
    push("aster", s.aster_enabled || account_enabled("aster"));
    push("binance", s.binance_enabled || account_enabled("binance"));
    push("coinbase", s.coinbase_enabled || account_enabled("coinbase"));
    push("okx", s.okx_enabled || account_enabled("okx"));
    push("ibkr", s.ibkr_enabled || account_enabled("ibkr"));
    push("ig", s.ig_enabled || account_enabled("ig"));
    push("variational", s.variational_enabled || account_enabled("variational"));
    out
}

/// Verified public endpoint per venue for latency probes when no adapter
/// exists. Every URL was curl-verified to return a real response — venues
/// with no reachable public API (popdex: parked domain; variational: no
/// public REST API; ibkr: local TWS gateway) return None → honest N/A.
///
/// `accept_4xx`: for endpoints that require auth, an application-level 4xx
/// (e.g. IG's JSON `error.security.api-key-missing`) still proves the
/// origin processed the request — a valid RTT sample. Only set where the
/// 4xx is known to come from the API tier, not an edge challenge page.
struct PublicProbe {
    url: &'static str,
    accept_4xx: bool,
}

fn public_probe_endpoint(vid: &str) -> Option<PublicProbe> {
    let p = |url: &'static str, accept_4xx: bool| Some(PublicProbe { url, accept_4xx });
    match vid {
        "vanta" => p("https://api-evm.orderly.org/v1/public/system_info", false),
        "arcus" => p("https://api.arcus.xyz/health", false),
        "kraken" => p("https://api.kraken.com/0/public/Time", false),
        "polymarket" => p("https://clob.polymarket.com/time", false),
        "kalshi" => p(
            "https://api.elections.kalshi.com/trade-api/v2/exchange/status",
            false,
        ),
        // Aptos fullnode is the actual transaction-submission path for
        // Decibel trades — its RTT is the arbitrage-relevant latency.
        "decibel" => p("https://fullnode.mainnet.aptoslabs.com/v1", false),
        "binance" => p("https://api.binance.com/api/v3/ping", false),
        "okx" => p("https://www.okx.com/api/v5/public/time", false),
        "coinbase" => p("https://api.coinbase.com/v2/time", false),
        "aster" => p("https://fapi.asterdex.com/fapi/v1/ping", false),
        // IG has no public endpoint; the session route returns an
        // app-level JSON 400 that proves the API gateway responded.
        "ig" => p("https://api.ig.com/gateway/deal/session", true),
        _ => None,
    }
}

/// HTTP probe for venues without an adapter: GET the venue's public
/// endpoint; success = the server demonstrably responded (2xx, or 4xx for
/// `accept_4xx` endpoints whose error is app-level JSON). A timeout or
/// connection failure yields Err → the EMA drops and the UI shows N/A.
async fn probe_http(spec: PublicProbe) -> Result<(), String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });
    let resp = client
        .get(spec.url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if resp.status().is_success() || (spec.accept_4xx && (400..500).contains(&code)) {
        Ok(())
    } else {
        Err(format!("probe got HTTP {code}"))
    }
}

pub async fn get_system_status_impl(state: &AppEngineState) -> Result<SystemStatusDto, String> {
    let cb = &state.circuit_breaker;
    let is_live = *state.is_live_mode.read();
    let total_pnl = state
        .total_realized_profit
        .read()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let (total_trades, success_count) = {
        let logs = state.execution_logs.read();
        (logs.len(), logs.iter().filter(|l| l.success).count())
    };

    // Cloned so no lock guard is held across the latency-probe awaits.
    let s = state.settings.read().clone();
    // A venue is enabled when its engine flag is set OR it owns an enabled
    // account profile — the same admission rule `get_venues` applies.
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    let venue_status = |enabled: bool, supported: bool| -> &str {
        if !enabled {
            "OFFLINE"
        } else if supported {
            "READ_ONLY"
        } else {
            "UNSUPPORTED"
        }
    };

    // Measure real round-trip latency for every *enabled* venue in
    // parallel. Admitted venues probe via adapter::ping() (must reach the
    // exchange — no local/edge cache hits); enabled venues without an
    // adapter probe a verified public endpoint — the network path to the
    // venue's API is the same either way, so the number stays honest for
    // arbitrage decisions even before an adapter lands. Probes are
    // throttled to one per venue per 30s — re-measuring on every 3s poll
    // is request volume that trips edge WAFs without meaningfully fresher
    // data. Between probes the last smoothed value is reported; a failed
    // probe drops the average so the UI shows N/A rather than a stale
    // number.
    const LATENCY_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    let admitted = state.get_venues();
    let admitted_map: HashMap<&str, &Arc<dyn ExchangeAdapter>> = admitted
        .iter()
        .map(|(vid, _, a)| (*vid, a))
        .collect();
    let enabled_ids: Vec<String> = venue_enabled_ids(state, &s);
    let mut probes = Vec::new();
    {
        let mut probed_at = state.latency_probe_at.lock();
        for vid in &enabled_ids {
            let due = probed_at
                .get(vid.as_str())
                .map(|t| t.elapsed() >= LATENCY_PROBE_INTERVAL)
                .unwrap_or(true);
            if !due {
                continue;
            }
            probed_at.insert(vid.clone(), std::time::Instant::now());
            if let Some(adapter) = admitted_map.get(vid.as_str()) {
                let vid = vid.clone();
                let adapter = Arc::clone(adapter);
                probes.push(tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    let ms = adapter
                        .ping()
                        .await
                        .ok()
                        .map(|_| start.elapsed().as_millis() as u64);
                    (vid, ms)
                }));
            } else if let Some(spec) = public_probe_endpoint(vid) {
                let vid = vid.clone();
                probes.push(tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    let ms = probe_http(spec)
                        .await
                        .ok()
                        .map(|_| start.elapsed().as_millis() as u64);
                    (vid, ms)
                }));
            }
        }
    }
    let mut results = Vec::new();
    for h in probes {
        if let Ok(pair) = h.await {
            results.push(pair);
        }
    }
    // Fold each successful sample into a per-venue exponential moving
    // average so the reported value is a rolling average, not a single
    // jumpy measurement. The lock is scoped so no guard crosses an await.
    // Report EMA only for currently-enabled venues — disabled venues stop
    // being probed and their last average must not linger.
    let enabled_set: std::collections::HashSet<&str> =
        enabled_ids.iter().map(|s| s.as_str()).collect();
    let latency_by_venue: HashMap<String, u64> = {
        let mut ema = state.latency_ema.lock();
        for (vid, res) in results {
            match res {
                Some(ms) => {
                    let smoothed = ema
                        .get(&vid)
                        .map(|prev| 0.35 * ms as f64 + 0.65 * prev)
                        .unwrap_or(ms as f64);
                    ema.insert(vid, smoothed);
                }
                None => {
                    ema.remove(&vid);
                }
            }
        }
        ema.iter()
            .filter(|(vid, _)| enabled_set.contains(vid.as_str()))
            .map(|(vid, v)| (vid.clone(), v.round() as u64))
            .collect()
    };
    let probe_ms = |vid: &str| latency_by_venue.get(vid).copied();

    let venues = vec![
        VenueStatusDto {
            id: "hyperliquid".into(),
            name: if is_live {
                "Hyperliquid (L1 DEX)".into()
            } else {
                "Hyperliquid (L1)".into()
            },
            market_type: "Perpetual CLOB".into(),
            status: venue_status(
                s.hyperliquid_enabled || account_enabled("hyperliquid"),
                true,
            )
            .into(),
            latency_ms: probe_ms("hyperliquid"),
            fee_bps: 2.0,
        },
        VenueStatusDto {
            id: "lighter".into(),
            name: "Lighter (zkDEX)".into(),
            market_type: "zk-Rollup CLOB".into(),
            status: venue_status(s.lighter_enabled || account_enabled("lighter"), true).into(),
            latency_ms: probe_ms("lighter"),
            fee_bps: 1.5,
        },
        VenueStatusDto {
            id: "vanta".into(),
            name: "Vanta.exchange".into(),
            market_type: "Omnichain Perps".into(),
            status: venue_status(s.vanta_enabled || account_enabled("vanta"), false).into(),
            latency_ms: probe_ms("vanta"),
            fee_bps: 3.0,
        },
        VenueStatusDto {
            id: "arcus".into(),
            name: "Arcus DEX".into(),
            market_type: "Fast CLOB".into(),
            status: venue_status(s.arcus_enabled || account_enabled("arcus"), false).into(),
            latency_ms: probe_ms("arcus"),
            fee_bps: 1.5,
        },
        VenueStatusDto {
            id: "popdex".into(),
            name: "PopDEX".into(),
            market_type: "Lighter Hybrid".into(),
            status: venue_status(s.popdex_enabled || account_enabled("popdex"), false).into(),
            latency_ms: probe_ms("popdex"),
            fee_bps: 2.0,
        },
        VenueStatusDto {
            id: "robinhood".into(),
            name: "Robinhood EVM".into(),
            market_type: "EVM DEX Aggregator".into(),
            status: venue_status(s.robinhood_enabled || account_enabled("robinhood"), false).into(),
            latency_ms: probe_ms("robinhood"),
            fee_bps: 2.5,
        },
        VenueStatusDto {
            id: "kraken".into(),
            name: "Kraken".into(),
            market_type: "Spot & Futures CLOB".into(),
            status: venue_status(
                s.kraken_enabled
                    || s.kraken_spot_enabled
                    || s.kraken_futures_enabled
                    || account_enabled("kraken_spot")
                    || account_enabled("kraken_futures"),
                false,
            )
            .into(),
            latency_ms: probe_ms("kraken"),
            fee_bps: 16.0,
        },
        VenueStatusDto {
            id: "polymarket".into(),
            name: "Polymarket (CTF CLOB)".into(),
            market_type: "Prediction Market".into(),
            status: venue_status(s.polymarket_enabled || account_enabled("polymarket"), false)
                .into(),
            latency_ms: probe_ms("polymarket"),
            fee_bps: 0.0,
        },
        VenueStatusDto {
            id: "kalshi".into(),
            name: "Kalshi (CFTC)".into(),
            market_type: "Prediction Market".into(),
            status: venue_status(s.kalshi_enabled || account_enabled("kalshi"), false).into(),
            latency_ms: probe_ms("kalshi"),
            fee_bps: 10.0,
        },
        VenueStatusDto {
            id: "decibel".into(),
            name: "Decibel".into(),
            market_type: "Aptos Perp DEX".into(),
            status: venue_status(s.decibel_enabled || account_enabled("decibel"), false).into(),
            latency_ms: probe_ms("decibel"),
            fee_bps: 2.5,
        },
        VenueStatusDto {
            id: "aster".into(),
            name: "Aster".into(),
            market_type: "Multi-Chain Perp DEX".into(),
            status: venue_status(s.aster_enabled || account_enabled("aster"), false).into(),
            latency_ms: probe_ms("aster"),
            fee_bps: 3.5,
        },
        VenueStatusDto {
            id: "binance".into(),
            name: "Binance".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status(s.binance_enabled || account_enabled("binance"), false).into(),
            latency_ms: probe_ms("binance"),
            fee_bps: 7.5,
        },
        VenueStatusDto {
            id: "coinbase".into(),
            name: "Coinbase".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status(s.coinbase_enabled || account_enabled("coinbase"), false).into(),
            latency_ms: probe_ms("coinbase"),
            fee_bps: 8.0,
        },
        VenueStatusDto {
            id: "okx".into(),
            name: "OKX".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status(s.okx_enabled || account_enabled("okx"), false).into(),
            latency_ms: probe_ms("okx"),
            fee_bps: 8.0,
        },
        VenueStatusDto {
            id: "ibkr".into(),
            name: "Interactive Brokers".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status(s.ibkr_enabled || account_enabled("ibkr"), false).into(),
            latency_ms: probe_ms("ibkr"),
            fee_bps: 1.0,
        },
        VenueStatusDto {
            id: "ig".into(),
            name: "IG Trading".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status(s.ig_enabled || account_enabled("ig"), false).into(),
            latency_ms: probe_ms("ig"),
            fee_bps: 5.0,
        },
        VenueStatusDto {
            id: "variational".into(),
            name: "Variational".into(),
            market_type: "Perp DEX (Read-Only)".into(),
            status: venue_status(s.variational_enabled || account_enabled("variational"), false)
                .into(),
            latency_ms: probe_ms("variational"),
            fee_bps: 0.0,
        },
    ];

    let current_drawdown = cb
        .current_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let max_drawdown = cb
        .max_daily_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(25000.0);

    let is_tripped = cb.is_tripped();
    let cb_status_str = match cb.trip_reason() {
        Some(reason) => format!("TRIPPED: {}", reason),
        None => {
            if is_tripped {
                "TRIPPED: Manual halt".into()
            } else {
                "ACTIVE".into()
            }
        }
    };

    Ok(SystemStatusDto {
        is_live_mode: is_live,
        is_paused: *state.is_paused.read(),
        total_realized_pnl: total_pnl,
        total_trades_count: total_trades,
        successful_trades_count: success_count,
        circuit_breaker_status: cb_status_str,
        circuit_breaker_tripped: is_tripped,
        current_drawdown_usd: current_drawdown,
        max_drawdown_usd: max_drawdown,
        consecutive_failures: cb.consecutive_errors(),
        active_venues: venues,
    })
}

pub async fn fetch_orderbooks_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<Vec<VenueOrderBookDto>, String> {
    validate_market_symbol(symbol)?;
    state.ensure_market(symbol);
    let venues = state.get_venues();
    let mut result = Vec::new();
    let mut sample_entries = Vec::new();

    for (vid, vname, adapter) in venues {
        let venue_id = adapter.venue_id();
        let maybe_book = state.market_data_hub.get_valid_orderbook(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_millis(350)),
        );
        let book = match maybe_book {
            Some(b) => b,
            None => match adapter.fetch_orderbook(symbol).await {
                Ok(b) => {
                    if validate_adapter_book(&b, &venue_id, symbol).is_err() {
                        continue;
                    }
                    state.market_data_hub.update_orderbook(b.clone());
                    b
                }
                Err(_) => continue,
            },
        };
        if validate_adapter_book(&book, &venue_id, symbol).is_err() {
            continue;
        }

        let best_bid = book
            .best_bid_price()
            .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let best_ask = book
            .best_ask_price()
            .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let spread = (best_ask - best_bid).max(0.0);
        let mid = if best_bid > 0.0 && best_ask > 0.0 {
            (best_bid + best_ask) / 2.0
        } else {
            0.0
        };
        let spread_bps = if mid > 0.0 {
            (spread / mid) * 10000.0
        } else {
            0.0
        };

        let bids_dto: Vec<[f64; 2]> = book
            .bids
            .iter()
            .take(5)
            .map(|b| {
                [
                    b.price.to_string().parse::<f64>().unwrap_or(0.0),
                    b.size.to_string().parse::<f64>().unwrap_or(0.0),
                ]
            })
            .collect();

        let asks_dto: Vec<[f64; 2]> = book
            .asks
            .iter()
            .take(5)
            .map(|a| {
                [
                    a.price.to_string().parse::<f64>().unwrap_or(0.0),
                    a.size.to_string().parse::<f64>().unwrap_or(0.0),
                ]
            })
            .collect();

        let total_bid_depth: f64 = bids_dto.iter().map(|b| b[1]).sum();
        let total_ask_depth: f64 = asks_dto.iter().map(|a| a[1]).sum();

        let benchmark_size = dec!(0.5);
        let vwap_bid = book
            .sell_execution_vwap(benchmark_size)
            .map(|(v, _, _)| v.to_string().parse::<f64>().unwrap_or(best_bid))
            .unwrap_or(best_bid);
        let vwap_ask = book
            .buy_execution_vwap(benchmark_size)
            .map(|(v, _, _)| v.to_string().parse::<f64>().unwrap_or(best_ask))
            .unwrap_or(best_ask);

        let funding_pct = if let Some(fr) = state.market_data_hub.get_funding_rate(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_secs(60)),
        ) {
            (fr.rate * dec!(100))
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else if let Ok(fr) = adapter.fetch_funding_rate(symbol).await {
            state.market_data_hub.update_funding(fr.clone());
            (fr.rate * dec!(100))
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else {
            0.0
        };

        result.push(VenueOrderBookDto {
            venue_id: vid.into(),
            venue_name: vname.into(),
            symbol: symbol.to_string(),
            best_bid,
            best_ask,
            spread,
            spread_bps,
            mid_price: mid,
            total_bid_depth,
            total_ask_depth,
            vwap_bid,
            vwap_ask,
            funding_rate_8h_pct: funding_pct,
            bids: bids_dto,
            asks: asks_dto,
        });
        if mid > 0.0 {
            sample_entries.push(crate::state_store::VenueSampleEntry {
                venue_id: vid.into(),
                mid,
                best_bid,
                best_ask,
            });
        }
    }

    // Durable signal history: persist this poll's venue mids so the
    // dislocation series can be sliced over arbitrary windows. Best-effort —
    // a store failure must never break the market-data path.
    let now_ms = Utc::now().timestamp_millis();
    let _ = state
        .state_store
        .record_spread_samples(symbol, now_ms, &sample_entries);

    // Hourly retention pass: keep 30 days of samples, hard-capped globally.
    let current_hour = now_ms / 3_600_000;
    {
        let mut last_hour = state.sample_prune_at.lock();
        if current_hour > *last_hour {
            *last_hour = current_hour;
            let retain_after = now_ms - 30 * 86_400_000;
            let _ = state
                .state_store
                .prune_spread_samples(retain_after, 4_000_000);
        }
    }

    Ok(result)
}

pub async fn fetch_multi_market_quotes_impl(
    symbols: Vec<String>,
    state: &AppEngineState,
) -> Result<Vec<TickerMarketQuoteDto>, String> {
    if symbols.len() > 100 {
        return Err("Too many symbols requested (max 100)".into());
    }
    for sym in &symbols {
        validate_market_symbol(sym)?;
    }
    let venues = state.get_venues();
    let mut quotes = Vec::with_capacity(symbols.len());

    for sym in symbols {
        state.ensure_market(&sym);
        let base_str = sym.split('-').next().unwrap_or(&sym).to_string();

        let mut best_bid = 0.0f64;
        let mut best_bid_venue = "Hyperliquid".to_string();
        let mut best_ask = f64::INFINITY;
        let mut best_ask_venue = "Hyperliquid".to_string();
        let mut best_buy_spread_bps = 2.0f64;
        let mut found_any = false;

        for (_vid, vname, adapter) in &venues {
            let venue_id = adapter.venue_id();
            let maybe_book = state.market_data_hub.get_valid_orderbook(
                &venue_id,
                &sym,
                Some(std::time::Duration::from_millis(500)),
            );
            let book = match maybe_book {
                Some(b) => b,
                None => match adapter.fetch_orderbook(&sym).await {
                    Ok(b) => {
                        if validate_adapter_book(&b, &venue_id, &sym).is_err() {
                            continue;
                        }
                        state.market_data_hub.update_orderbook(b.clone());
                        b
                    }
                    Err(_) => continue,
                },
            };
            if validate_adapter_book(&book, &venue_id, &sym).is_err() {
                continue;
            }

            if let Some(bid) = book.best_bid_price() {
                let b_f = bid.to_string().parse::<f64>().unwrap_or(0.0);
                if b_f > best_bid {
                    best_bid = b_f;
                    best_bid_venue = vname.to_string();
                    found_any = true;
                }
            }
            if let (Some(b), Some(a)) = (book.best_bid_price(), book.best_ask_price()) {
                let b_f = b.to_string().parse::<f64>().unwrap_or(0.0);
                let a_f = a.to_string().parse::<f64>().unwrap_or(0.0);
                if a_f > 0.0 && a_f < best_ask {
                    best_ask = a_f;
                    best_ask_venue = vname.to_string();
                    let v_mid = (b_f + a_f) / 2.0;
                    if v_mid > 0.0 && a_f >= b_f {
                        best_buy_spread_bps = ((a_f - b_f) / v_mid) * 10000.0;
                    }
                    found_any = true;
                }
            }
        }

        let (final_bid, final_ask, mid, spread_bps, best_buy_v, best_sell_v, fee) =
            if found_any && best_bid > 0.0 && best_ask < f64::INFINITY {
                let m = (best_bid + best_ask) / 2.0;
                let sp = best_buy_spread_bps.max(0.0);
                let f = venue_fee_bps(&best_ask_venue)
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(2.0);
                (best_bid, best_ask, m, sp, best_ask_venue, best_bid_venue, f)
            } else {
                (0.0, 0.0, 0.0, 0.0, "-".to_string(), "-".to_string(), 0.0)
            };

        quotes.push(TickerMarketQuoteDto {
            symbol: sym,
            base: base_str,
            best_bid: final_bid,
            best_ask: final_ask,
            mid_price: mid,
            spread_bps,
            best_buy_venue: best_buy_v,
            best_sell_venue: best_sell_v,
            fee_bps: fee,
            base_price: mid,
        });
    }

    Ok(quotes)
}

pub async fn scan_spatial_opportunities_impl(
    symbol: &str,
    min_bps: f64,
    order_qty: f64,
    state: &AppEngineState,
) -> Result<Vec<SpatialOpportunityDto>, String> {
    validate_market_symbol(symbol)?;
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
    if !min_bps.is_finite() || min_bps < 0.0 {
        return Err("min_bps must be finite and nonnegative".into());
    }
    state.ensure_market(symbol);
    let venues = state.get_venues();
    if venues.len() < 2 {
        return Ok(Vec::new());
    }

    let mut books = Vec::new();
    for (vid, vname, adapter) in &venues {
        let venue_id = adapter.venue_id();
        let maybe_book = state.market_data_hub.get_valid_orderbook(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_millis(350)),
        );
        let book = match maybe_book {
            Some(b) => b,
            None => match adapter.fetch_orderbook(symbol).await {
                Ok(b) => {
                    if validate_adapter_book(&b, &venue_id, symbol).is_err() {
                        continue;
                    }
                    state.market_data_hub.update_orderbook(b.clone());
                    b
                }
                Err(_) => continue,
            },
        };
        if validate_adapter_book(&book, &venue_id, symbol).is_err() {
            continue;
        }
        books.push((*vid, *vname, book, venue_fee_bps(vid)));
    }

    if books.len() < 2 {
        return Ok(Vec::new());
    }

    let target_qty_dec = Decimal::from_f64_retain(order_qty)
        .ok_or_else(|| "order_qty is outside supported decimal range".to_string())?;
    let (max_slippage_bps, min_depth_ratio) = {
        let s = state.settings.read();
        (s.max_slippage_bps, s.min_depth_ratio)
    };
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;
    let router = SmartOrderRouter::new(max_slip_dec);
    let required_depth = target_qty_dec * min_depth_dec;
    let mut opps = Vec::new();

    for i in 0..books.len() {
        for j in 0..books.len() {
            if i == j {
                continue;
            }
            let (buy_id, buy_name, ref book_buy, buy_fee) = books[i];
            let (sell_id, sell_name, ref book_sell, sell_fee) = books[j];

            if let Some(opp) =
                evaluate_spatial_spread(book_buy, book_sell, buy_fee, sell_fee, target_qty_dec)
            {
                let gross_spread_f = opp.gross_spread.to_string().parse::<f64>().unwrap_or(0.0);
                let gross_spread_bps_f = opp
                    .gross_spread_bps
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let net_spread_bps_f = opp.net_profit_bps.to_string().parse::<f64>().unwrap_or(0.0);
                let est_profit_f = opp
                    .total_net_profit_usd
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);

                let best_ask = book_buy
                    .best_ask_price()
                    .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let best_bid = book_sell
                    .best_bid_price()
                    .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);

                let is_executable = net_spread_bps_f >= min_bps
                    && match (book_buy.best_ask_price(), book_sell.best_bid_price()) {
                        (Some(buy_ref), Some(sell_ref)) => {
                            let buy_limit = router.calculate_bounded_price(Side::Buy, buy_ref);
                            let sell_limit = router.calculate_bounded_price(Side::Sell, sell_ref);
                            let mut bv = book_buy.clone();
                            bv.asks.retain(|l| l.price <= buy_limit);
                            let mut sv = book_sell.clone();
                            sv.bids.retain(|l| l.price >= sell_limit);
                            let buy_depth: Decimal = bv.asks.iter().map(|l| l.size).sum();
                            let sell_depth: Decimal = sv.bids.iter().map(|l| l.size).sum();
                            arbitrager_risk::safety_guards::verify_pre_execution_slippage(
                                &bv,
                                Side::Buy,
                                target_qty_dec,
                                max_slip_dec,
                            )
                            .is_ok()
                                && arbitrager_risk::safety_guards::verify_pre_execution_slippage(
                                    &sv,
                                    Side::Sell,
                                    target_qty_dec,
                                    max_slip_dec,
                                )
                                .is_ok()
                                && buy_depth >= required_depth
                                && sell_depth >= required_depth
                        }
                        _ => false,
                    };

                opps.push(SpatialOpportunityDto {
                    id: format!("{}-{}-{}", symbol, buy_id, sell_id),
                    symbol: symbol.to_string(),
                    buy_venue: buy_name.to_string(),
                    sell_venue: sell_name.to_string(),
                    buy_price: best_ask,
                    sell_price: best_bid,
                    gross_spread: gross_spread_f,
                    gross_spread_bps: gross_spread_bps_f,
                    fee_bps: (buy_fee + sell_fee)
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    net_spread_bps: net_spread_bps_f,
                    order_qty: opp
                        .executable_qty
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(order_qty),
                    estimated_profit_usd: est_profit_f,
                    is_executable,
                });
            }
        }
    }

    opps.sort_by(|a, b| {
        b.net_spread_bps
            .partial_cmp(&a.net_spread_bps)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

pub async fn scan_spot_opportunities_impl(
    symbol_filter: &str,
    state: &AppEngineState,
) -> Result<Vec<SpotOpportunityDto>, String> {
    let _ = (symbol_filter, state);
    Ok(Vec::new())
}

pub async fn execute_spot_arbitrage_impl(
    params: SpotExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = params;
    Err("Native 'spot' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

pub async fn scan_funding_opportunities_impl(
    symbol: &str,
    trade_size_usd: f64,
    state: &AppEngineState,
) -> Result<Vec<FundingOpportunityDto>, String> {
    validate_market_symbol(symbol)?;
    if !trade_size_usd.is_finite() || trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    state.ensure_market(symbol);
    let venues = state.get_venues();
    if venues.len() < 2 {
        return Ok(Vec::new());
    }

    let size_dec = Decimal::from_f64_retain(trade_size_usd)
        .ok_or_else(|| "trade_size_usd is outside supported decimal range".to_string())?;
    let mut rates = Vec::new();

    for (vid, vname, adapter) in &venues {
        let venue_id = adapter.venue_id();
        if let Some(fr) = state.market_data_hub.get_funding_rate(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_secs(120)),
        ) {
            rates.push((*vid, *vname, fr));
        } else if let Ok(rate) = adapter.fetch_funding_rate(symbol).await {
            state.market_data_hub.update_funding(rate.clone());
            rates.push((*vid, *vname, rate));
        }
    }

    if rates.len() < 2 {
        return Ok(Vec::new());
    }

    let mut opps = Vec::new();
    let fee_total = dec!(3.5); // 3.5 bps round trip

    for i in 0..rates.len() {
        for j in (i + 1)..rates.len() {
            let (_id_a, _name_a, ref r_a) = rates[i];
            let (_id_b, _name_b, ref r_b) = rates[j];

            if let Some(opp) = evaluate_funding_carry(r_a, r_b, fee_total, size_dec) {
                opps.push(FundingOpportunityDto {
                    id: format!("{}-{}-{}", symbol, opp.long_venue, opp.short_venue),
                    symbol: symbol.to_string(),
                    long_venue: opp.long_venue,
                    short_venue: opp.short_venue,
                    net_rate_8h_pct: (opp.net_rate_8h * dec!(100))
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    net_apr_pct: (opp.net_apr * dec!(100))
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    net_apy_pct: (opp.net_apy * dec!(100))
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    round_trip_fee_bps: opp
                        .round_trip_fee_bps
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    break_even_hours: opp
                        .break_even_hours
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    estimated_daily_income_usd: opp
                        .daily_income_usd
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    trade_size_usd,
                });
            }
        }
    }

    opps.sort_by(|a, b| {
        b.net_apy_pct
            .partial_cmp(&a.net_apy_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

pub async fn scan_prediction_opportunities_impl(
    market: &str,
    state: &AppEngineState,
) -> Result<Vec<PredictionOpportunityDto>, String> {
    let _ = (market, state);
    Ok(Vec::new())
}

pub fn run_stat_arb_step_impl(
    step: usize,
    price_y: f64,
    price_x: f64,
    state: &AppEngineState,
) -> Result<StatArbStepResultDto, String> {
    if !price_y.is_finite() || price_y <= 0.0 || !price_x.is_finite() || price_x <= 0.0 {
        return Err("prices must be finite and positive".into());
    }
    let mut kf = state.kalman_filter.write();
    let (hedge_ratio, residual) = kf.update(price_x, price_y);
    let z = kf.spread_z_score();
    let alpha = kf.intercept();
    let variance = if z.abs() > 1e-6 {
        (residual / z).powi(2)
    } else {
        1.0
    };

    let signal = if z > 2.0 {
        "ENTRY_SHORT_SPREAD"
    } else if z < -2.0 {
        "ENTRY_LONG_SPREAD"
    } else if z.abs() < 0.5 {
        "EXIT"
    } else {
        "NEUTRAL"
    };

    Ok(StatArbStepResultDto {
        step,
        price_y,
        price_x,
        dynamic_hedge_ratio: hedge_ratio,
        intercept_alpha: alpha,
        residual_spread: residual,
        spread_variance: variance,
        z_score: z,
        signal: signal.into(),
    })
}

pub fn compute_optimal_thresholds_impl(
    theta: f64,
    mu: f64,
    sigma: f64,
    transaction_cost: f64,
) -> Result<BertramThresholdDto, String> {
    if !theta.is_finite() || !mu.is_finite() || !sigma.is_finite() || !transaction_cost.is_finite()
    {
        return Err("Invalid parameters: all inputs must be finite".into());
    }
    if theta <= 0.0 || sigma <= 0.0 || transaction_cost < 0.0 {
        return Err(
            "Invalid parameters: theta > 0, sigma > 0, and transaction_cost >= 0 required".into(),
        );
    }

    let half_life = (2.0f64.ln()) / theta;
    let stationary_std = (sigma.powi(2) / (2.0 * theta)).sqrt();

    let ou = OuModelParams {
        theta,
        mu,
        sigma,
        half_life,
        stationary_std,
        r_squared: 0.89,
    };

    let result = compute_bertram_thresholds(&ou, transaction_cost)
        .ok_or_else(|| "Failed to compute Bertram thresholds with given parameters".to_string())?;

    Ok(BertramThresholdDto {
        entry_long: result.entry_long,
        exit_long: result.exit_long,
        entry_short: result.entry_short,
        exit_short: result.exit_short,
        optimal_distance: result.optimal_distance,
        expected_return_per_cycle: result.expected_return_per_cycle,
        estimated_trades_per_year: result.estimated_trades_per_year,
        estimated_sharpe: result.estimated_sharpe,
        half_life,
        theta,
        mu,
        sigma,
    })
}

pub fn fit_ou_process_impl(prices: &[f64], dt: f64) -> Result<OuFitDto, String> {
    if prices.len() < 10 {
        return Err("Need at least 10 price points to fit Ornstein-Uhlenbeck process".into());
    }
    if prices.iter().any(|p| !p.is_finite()) {
        return Err("Price series must contain only finite values".into());
    }
    if !dt.is_finite() || dt <= 0.0 {
        return Err("Time step dt must be finite and positive".into());
    }

    let params = OuModelParams::fit(prices, dt)
        .ok_or_else(|| "Failed to fit OU process to price series".to_string())?;

    Ok(OuFitDto {
        theta: params.theta,
        mu: params.mu,
        sigma: params.sigma,
        half_life: params.half_life,
        stationary_std: params.stationary_std,
        r_squared: params.r_squared,
    })
}

#[allow(clippy::too_many_arguments)]
/// Resolve account attribution for a two-leg signal. An explicit strategy
/// route wins; otherwise the first enabled route covering the venue pair is
/// used, falling back to each venue's default account. Accounts may be absent
/// on venues that have no configured profiles.
/// Accounts attributed to a signal's legs, resolved from an explicit or
/// eligible strategy route, else each venue's default account.
struct SignalAccounts {
    route_id: Option<String>,
    long: Option<String>,
    short: Option<String>,
}

fn resolve_signal_accounts(
    state: &AppEngineState,
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    route_id: Option<&str>,
) -> Result<SignalAccounts, String> {
    if let Some(rid) = route_id {
        let route = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == rid)
            .ok_or_else(|| format!("strategy route {rid} does not exist"))?;
        if !route.enabled {
            return Err(format!("strategy route {rid} is disabled"));
        }
        if !route.strategy_id.eq_ignore_ascii_case(strategy_id) {
            return Err(format!(
                "route {rid} belongs to strategy {}, not {strategy_id}",
                route.strategy_id
            ));
        }
        let covers = (route.account_a_venue.eq_ignore_ascii_case(venue_a)
            && route.account_b_venue.eq_ignore_ascii_case(venue_b))
            || (route.account_a_venue.eq_ignore_ascii_case(venue_b)
                && route.account_b_venue.eq_ignore_ascii_case(venue_a));
        if !covers {
            return Err(format!(
                "route {rid} does not cover venues {venue_a}/{venue_b}"
            ));
        }
        let (long, short) = if route.account_a_venue.eq_ignore_ascii_case(venue_a) {
            (route.account_a_id, route.account_b_id)
        } else {
            (route.account_b_id, route.account_a_id)
        };
        return Ok(SignalAccounts {
            route_id: Some(route.id),
            long: Some(long),
            short: Some(short),
        });
    }
    if let Ok(routes) = state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b)
    {
        if let Some(r) = routes.into_iter().next() {
            let (long, short) = if r.account_a_venue.eq_ignore_ascii_case(venue_a) {
                (r.account_a_id, r.account_b_id)
            } else {
                (r.account_b_id, r.account_a_id)
            };
            return Ok(SignalAccounts {
                route_id: Some(r.id),
                long: Some(long),
                short: Some(short),
            });
        }
    }
    Ok(SignalAccounts {
        route_id: None,
        long: state
            .state_store
            .default_account(venue_a)
            .ok()
            .flatten()
            .map(|a| a.id),
        short: state
            .state_store
            .default_account(venue_b)
            .ok()
            .flatten()
            .map(|a| a.id),
    })
}

/// Persist actual leg fills and derive the per-signal balance status.
/// A signal is `Balanced` only when its own legs net to zero delta — aggregate
/// netting across unrelated signals can never mark it balanced.
fn reconcile_signal_legs(
    state: &AppEngineState,
    signal_id: &str,
    fills: [(
        crate::state_store::LegSide,
        &arbitrager_core::order::OrderResponse,
    ); 2],
) -> Result<crate::state_store::SignalStatus, String> {
    use crate::state_store::SignalStatus;
    for (side, leg) in fills {
        let filled = leg
            .filled_qty
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable filled quantity".to_string())?;
        let fee = leg
            .fee
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable fee".to_string())?;
        if filled > 0.0 {
            let px = leg
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .ok_or_else(|| format!("leg {side:?} filled but reports no positive price"))?;
            state
                .state_store
                .apply_leg_fill(
                    signal_id,
                    side,
                    filled,
                    px,
                    fee,
                    Some(leg.order_id.as_str()),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    let delta = state
        .state_store
        .signal_delta(signal_id)
        .map_err(|e| e.to_string())?;
    let eps = 1e-6;
    Ok(if delta.is_finite() && delta.abs() <= eps {
        SignalStatus::Balanced
    } else {
        SignalStatus::Imbalanced
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_spatial_arbitrage_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    execute_spatial_arbitrage_inner(
        symbol,
        buy_venue,
        sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        None,
        state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_spatial_arbitrage_inner(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    route_id: Option<&str>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
    if buy_venue.trim().is_empty() || sell_venue.trim().is_empty() {
        return Err("Buy and sell venues must both be specified".into());
    }
    if buy_venue.eq_ignore_ascii_case(sell_venue) {
        return Err("Buy and sell venues must be distinct".into());
    }
    validate_market_symbol(symbol)?;
    let hurdle_dec = match nominal_hurdle_bps {
        Some(h) if !h.is_finite() || h < 0.0 => {
            return Err("nominal_hurdle_bps must be finite and nonnegative".into());
        }
        Some(h) => Some(
            Decimal::from_f64_retain(h)
                .ok_or("nominal_hurdle_bps is outside supported decimal range")?,
        ),
        None => None,
    };

    let _execution_guard = state.execution_lock.lock().await;

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    if *state.is_live_mode.read() {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }
    let _ = trigger_spread_bps; // caller-supplied spread is never trusted for eligibility

    // Durable per-ticker gate: an in-flight, imbalanced, or unresolved signal
    // for this symbol blocks any further order until its own legs reconcile
    // as delta-neutral. Survives restarts via the SQLite store.
    if let Some(blocker) = state
        .state_store
        .blocking_signals(symbol)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        return Err(format!(
            "Symbol {} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
            symbol,
            blocker.id,
            blocker.status.as_str()
        ));
    }

    state.ensure_market(symbol);

    let buy_adapter = state.get_adapter(buy_venue).ok_or_else(|| {
        format!(
            "Buy venue '{}' is not admitted for paper execution",
            buy_venue
        )
    })?;
    let sell_adapter = state.get_adapter(sell_venue).ok_or_else(|| {
        format!(
            "Sell venue '{}' is not admitted for paper execution",
            sell_venue
        )
    })?;
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    if crate::state::canonical_venue_id(buy_venue) == crate::state::canonical_venue_id(sell_venue) {
        return Err("Buy and sell venues must resolve to distinct canonical venues".into());
    }
    let buy_paper = state
        .get_paper_exchange(buy_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", buy_venue))?;
    let sell_paper = state
        .get_paper_exchange(sell_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", sell_venue))?;

    // Order books must come from the hub cache (max age 2s) or a fresh fetch
    // through the admitted read-only adapter; ticker data is not a substitute.
    // Cached or fetched, a book must carry the exact requested venue and symbol
    // and pass execution validation before it is trusted or cached.
    let book_buy = match state.market_data_hub.get_valid_orderbook(
        &buy_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = buy_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", buy_venue, symbol, e))?;
            validate_adapter_book(&b, &buy_vid, symbol)
                .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let book_sell = match state.market_data_hub.get_valid_orderbook(
        &sell_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = sell_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", sell_venue, symbol, e))?;
            validate_adapter_book(&b, &sell_vid, symbol)
                .map_err(|e| format!("Sell-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    validate_adapter_book(&book_buy, &buy_vid, symbol)
        .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
    validate_adapter_book(&book_sell, &sell_vid, symbol)
        .map_err(|e| format!("Sell-side order book rejected: {}", e))?;

    let qty_dec = Decimal::from_str_exact(&order_qty.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "Invalid order quantity".to_string())?;
    let (max_slippage_bps, min_depth_ratio) = {
        let s = state.settings.read();
        (s.max_slippage_bps, s.min_depth_ratio)
    };
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;

    // Preflight must see exactly what the bounded IOC orders can reach:
    // retain only asks at or below the buy limit and bids at or above the
    // sell limit before evaluating slippage, depth ratio, and VWAP.
    let router = SmartOrderRouter::new(max_slip_dec);
    let buy_ref = book_buy
        .best_ask_price()
        .ok_or_else(|| format!("No asks on buy-side book for {}", symbol))?;
    let sell_ref = book_sell
        .best_bid_price()
        .ok_or_else(|| format!("No bids on sell-side book for {}", symbol))?;
    let buy_limit = router.calculate_bounded_price(Side::Buy, buy_ref);
    let sell_limit = router.calculate_bounded_price(Side::Sell, sell_ref);

    let mut book_buy_exec = book_buy.clone();
    book_buy_exec.asks.retain(|l| l.price <= buy_limit);
    let mut book_sell_exec = book_sell.clone();
    book_sell_exec.bids.retain(|l| l.price >= sell_limit);

    arbitrager_risk::safety_guards::verify_pre_execution_slippage(
        &book_buy_exec,
        Side::Buy,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Buy-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            buy_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrager_risk::safety_guards::verify_pre_execution_slippage(
        &book_sell_exec,
        Side::Sell,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Sell-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            sell_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;

    let buy_depth: Decimal = book_buy_exec.asks.iter().map(|l| l.size).sum();
    let sell_depth: Decimal = book_sell_exec.bids.iter().map(|l| l.size).sum();
    let required_depth = qty_dec * min_depth_dec;
    if buy_depth < required_depth || sell_depth < required_depth {
        return Err(format!(
            "Insufficient order book depth ratio: required {} per leg ({}x order), buy side offers {}, sell side offers {}",
            required_depth, min_depth_dec, buy_depth, sell_depth
        ));
    }

    let (buy_vwap, buy_remaining, _) = book_buy_exec
        .buy_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable buy depth on {}", buy_venue))?;
    let (sell_vwap, sell_remaining, _) = book_sell_exec
        .sell_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable sell depth on {}", sell_venue))?;
    if buy_remaining > dec!(0) || sell_remaining > dec!(0) {
        return Err("Requested quantity exceeds executable book depth within IOC limits".into());
    }

    let fee_buy = buy_vwap * qty_dec * venue_fee_bps(buy_vid.as_str()) / dec!(10000);
    let fee_sell = sell_vwap * qty_dec * venue_fee_bps(sell_vid.as_str()) / dec!(10000);
    let expected_net = (sell_vwap - buy_vwap) * qty_dec - fee_buy - fee_sell;
    let notional = ((buy_vwap + sell_vwap) / dec!(2)) * qty_dec;
    let net_bps = if notional > dec!(0) {
        (expected_net / notional) * dec!(10000)
    } else {
        dec!(0)
    };
    if expected_net <= dec!(0) {
        return Err(format!(
            "Spread is not profitable after fees: expected net {} USD",
            expected_net
        ));
    }
    if let Some(h) = hurdle_dec {
        if net_bps < h {
            return Err(format!(
                "Calculated net spread {} BPS is below nominal hurdle {} BPS",
                net_bps, h
            ));
        }
    }

    state
        .delta_guard
        .read()
        .check_proposed_pair(
            symbol,
            Side::Buy,
            qty_dec,
            buy_vwap,
            symbol,
            Side::Sell,
            qty_dec,
            sell_vwap,
        )
        .map_err(|(asset, exposure)| {
            format!(
                "Delta neutrality breach on {}: proposed net exposure ${} exceeds limit",
                asset, exposure
            )
        })?;

    let margin_monitor = MarginMonitor::new(MarginMonitorConfig::default());
    let acc_buy = buy_paper.fetch_account().await.map_err(|e| e.to_string())?;
    let acc_sell = sell_paper
        .fetch_account()
        .await
        .map_err(|e| e.to_string())?;
    for (vname, acc) in [(buy_venue, &acc_buy), (sell_venue, &acc_sell)] {
        match margin_monitor.evaluate_account(acc) {
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                return Err(format!(
                    "Paper account {} margin status is critical/breached; execution refused",
                    vname
                ));
            }
            _ => {}
        }
    }
    let target_leverage = state
        .strategy_configs
        .read()
        .get(symbol)
        .map(|c| c.user_target_leverage)
        .unwrap_or(5);
    if !(1..=5).contains(&target_leverage) {
        return Err(format!(
            "Configured leverage {} for {} is out of range; must be 1..=5",
            target_leverage, symbol
        ));
    }
    let leverage = Decimal::from(target_leverage);
    let im_buy = (buy_vwap * qty_dec) / leverage;
    if im_buy > acc_buy.free_margin {
        return Err(format!(
            "Insufficient free margin on {}: required {} > available {}",
            buy_venue, im_buy, acc_buy.free_margin
        ));
    }
    let im_sell = (sell_vwap * qty_dec) / leverage;
    if im_sell > acc_sell.free_margin {
        return Err(format!(
            "Insufficient free margin on {}: required {} > available {}",
            sell_venue, im_sell, acc_sell.free_margin
        ));
    }
    if acc_buy.get_position(symbol).is_some() || acc_sell.get_position(symbol).is_some() {
        return Err(format!(
            "An open paper position already exists for {}; closing lifecycle is not implemented",
            symbol
        ));
    }

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }

    buy_paper
        .replace_orderbook(book_buy.clone())
        .map_err(|e| e.to_string())?;
    sell_paper
        .replace_orderbook(book_sell.clone())
        .map_err(|e| e.to_string())?;

    let order_buy =
        router.build_ioc_order(buy_paper.venue_id(), symbol, Side::Buy, buy_ref, qty_dec);
    let order_sell =
        router.build_ioc_order(sell_paper.venue_id(), symbol, Side::Sell, sell_ref, qty_dec);

    buy_paper
        .set_leverage(symbol, target_leverage)
        .await
        .map_err(|e| e.to_string())?;
    sell_paper
        .set_leverage(symbol, target_leverage)
        .await
        .map_err(|e| e.to_string())?;

    let buy_exec: Arc<dyn arbitrager_core::exchange::ExchangeAdapter> = buy_paper;
    let sell_exec: Arc<dyn arbitrager_core::exchange::ExchangeAdapter> = sell_paper;

    let mut equity_before = dec!(0);
    for (_, paper) in state.paper_exchanges() {
        equity_before += paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
            .equity;
    }

    // Persist the selected candidate as in_flight BEFORE dispatch: if the
    // process dies mid-execution the symbol stays blocked on restart until
    // the signal's own legs reconcile.
    let accounts = resolve_signal_accounts(
        state,
        "futures",
        buy_vid.as_str(),
        sell_vid.as_str(),
        route_id,
    )?;
    let signal_id = format!(
        "sig-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    state
        .state_store
        .record_signal_execution(&crate::state_store::NewSignalExecution {
            id: signal_id.clone(),
            strategy_id: "futures".into(),
            symbol: symbol.to_string(),
            route_id: accounts.route_id,
            expected_net_usd: expected_net.to_string().parse::<f64>().ok(),
            note: None,
            legs: vec![
                crate::state_store::NewSignalLeg {
                    account_id: accounts.long,
                    venue: buy_vid.to_string(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: order_qty,
                },
                crate::state_store::NewSignalLeg {
                    account_id: accounts.short,
                    venue: sell_vid.to_string(),
                    side: crate::state_store::LegSide::Short,
                    intended_qty: order_qty,
                },
            ],
        })
        .map_err(|e| format!("Failed to persist in-flight signal record: {e}"))?;

    let started = std::time::Instant::now();
    let res = state
        .coordinator
        .execute_two_leg(&buy_exec, order_buy, &sell_exec, order_sell)
        .await;
    let latency_us = started.elapsed().as_micros() as u64;
    let net_bps_f = net_bps.to_string().parse::<f64>().unwrap_or(0.0);

    let mut equity_after = dec!(0);
    for (_, paper) in state.paper_exchanges() {
        equity_after += paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
            .equity;
    }
    let attempt_pnl = equity_after - equity_before;
    let attempt_pnl_f = attempt_pnl.to_string().parse::<f64>().unwrap_or(0.0);
    // Realized portfolio P&L is always derived from actual paper-account
    // equity: fees, unwind slippage, and any residual loss are included;
    // unrealized open-position exposure is not marked into it.
    let total_f = {
        let mut total_pnl = state.total_realized_profit.write();
        *total_pnl = equity_after - state.paper_baseline_equity();
        total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
    };
    state.circuit_breaker.update_equity(equity_after);

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

    match res {
        ArbitrageExecutionResult::Success {
            leg_a,
            leg_b,
            realized_profit: _,
        } => {
            let fees_dec = leg_a.fee + leg_b.fee;
            let fee_f = fees_dec.to_string().parse::<f64>().unwrap_or(0.0);
            let buy_px_f = leg_a
                .avg_fill_price
                .unwrap_or(buy_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let sell_px_f = leg_b
                .avg_fill_price
                .unwrap_or(sell_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let expected_capture_f = expected_net.to_string().parse::<f64>().unwrap_or(0.0);

            let cycle_id = format!("paper-spatial-{}", cycle);
            state.virtual_ledger.write().open_cycle(
                cycle_id,
                "spatial_spread".to_string(),
                symbol.to_string(),
                buy_venue.to_string(),
                sell_venue.to_string(),
                net_bps_f,
                order_qty,
                buy_px_f,
                order_qty,
                sell_px_f,
                fee_f,
            );

            // Persist actual leg fills, then reconcile per-signal balance:
            // balanced only when THIS signal's own legs net to zero delta.
            let signal_status = reconcile_signal_legs(
                state,
                &signal_id,
                [
                    (crate::state_store::LegSide::Long, &leg_a),
                    (crate::state_store::LegSide::Short, &leg_b),
                ],
            )?;
            state
                .state_store
                .update_signal_status(&signal_id, signal_status, None)
                .map_err(|e| e.to_string())?;

            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: format!("{:?}", leg_a.status),
                leg_b_status: format!("{:?}", leg_b.status),
                success: true,
                gross_profit: 0.0,
                fees: fee_f,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "Paper position opened; expected convergence capture ${:.2} is unrealized",
                    expected_capture_f
                ),
                direction: direction
                    .clone()
                    .or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
            });

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: true,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!(
                    "Paper position opened on {}/{}: realized P&L ${:.2}; expected convergence capture ${:.2} is unrealized",
                    buy_venue, sell_venue, attempt_pnl_f, expected_capture_f
                ),
            })
        }
        ArbitrageExecutionResult::UnresolvedExposure {
            leg_a_fill,
            leg_b_fill,
            error_a,
            error_b,
        } => {
            // Persist whatever partial fill data exists, then mark the signal
            // unresolved — the ticker stays blocked until manual resolution.
            for (side, fill) in [
                (crate::state_store::LegSide::Long, &leg_a_fill),
                (crate::state_store::LegSide::Short, &leg_b_fill),
            ] {
                if let Some(f) = fill {
                    let filled = f.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                    let px = f
                        .avg_fill_price
                        .and_then(|p| p.to_string().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let fee = f.fee.to_string().parse::<f64>().unwrap_or(0.0);
                    if filled > 0.0 && px > 0.0 {
                        let _ = state.state_store.apply_leg_fill(
                            &signal_id,
                            side,
                            filled,
                            px,
                            fee,
                            Some(f.order_id.as_str()),
                        );
                    }
                }
            }
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Unresolved,
                Some("unresolved exposure after failed unwind"),
            );
            if !state.circuit_breaker.is_tripped() {
                state
                    .circuit_breaker
                    .trip("Unresolved exposure after failed unwind");
            }
            let describe = |f: &Option<arbitrager_core::order::OrderResponse>| {
                f.as_ref()
                    .map(|r| format!("{:?} filled {}/{}", r.status, r.filled_qty, r.original_qty))
                    .unwrap_or_else(|| "no fill".to_string())
            };
            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: describe(&leg_a_fill),
                leg_b_status: describe(&leg_b_fill),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]; realized P&L to date ${:.2}; open exposure not marked",
                    error_a, error_b, attempt_pnl_f
                ),
                direction: direction.clone().or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
            });
            Err(format!(
                "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]",
                error_a, error_b
            ))
        }
        other => {
            let (status_a, status_b) = match &other {
                ArbitrageExecutionResult::LegBFailedUnwound { .. } => ("UNWOUND", "REJECTED"),
                ArbitrageExecutionResult::LegAFailedUnwound { .. } => ("REJECTED", "UNWOUND"),
                ArbitrageExecutionResult::HaltedByRisk(_) => ("HALTED", "HALTED"),
                _ => ("REJECTED", "REJECTED"),
            };
            // Unwound legs restored flatness → `failed` (not blocking). Any
            // other terminal failure is also non-blocking once recorded.
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Failed,
                Some(&format!("{other:?}")),
            );
            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: status_a.into(),
                leg_b_status: status_b.into(),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!("Atomic trade failed: {:?}", other),
                direction: direction
                    .clone()
                    .or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
            });

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: false,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!("Execution failed/unwound: {:?}", other),
            })
        }
    }
}

/* ------------------------------------------------------------------ */
/* Same-time candidate arbitration                                      */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpatialCandidateInput {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub order_qty: f64,
    /// Caller's estimated net USD — used only for ranking candidates in the
    /// same batch; the executor recomputes economics before dispatch.
    pub expected_net_usd: Option<f64>,
    /// Explicit strategy-level route override (account A ↔ account B).
    pub route_id: Option<String>,
    pub direction: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOutcomeDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub expected_net_usd: Option<f64>,
    pub selected: bool,
    pub reason: Option<String>,
    pub result: Option<SimStepResultDto>,
}

/// Submit a synchronized batch of candidates: per symbol, at most ONE order
/// is dispatched — the candidate with the highest expected net USD. Symbols
/// already blocked by an in-flight/unreconciled signal reject all candidates.
pub async fn execute_spatial_candidates_impl(
    candidates: Vec<SpatialCandidateInput>,
    state: &AppEngineState,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_symbol: std::collections::HashMap<String, Vec<SpatialCandidateInput>> =
        std::collections::HashMap::new();
    for c in candidates {
        let key = c.symbol.trim().to_lowercase();
        by_symbol.entry(key).or_default().push(c);
    }

    let mut outcomes = Vec::new();
    for (symbol_key, mut cands) in by_symbol {
        let outcome_for =
            |c: &SpatialCandidateInput,
             selected: bool,
             reason: Option<String>,
             result: Option<SimStepResultDto>| CandidateOutcomeDto {
                symbol: c.symbol.clone(),
                buy_venue: c.buy_venue.clone(),
                sell_venue: c.sell_venue.clone(),
                expected_net_usd: c.expected_net_usd,
                selected,
                reason,
                result,
            };

        if state
            .state_store
            .ticker_blocked(&symbol_key)
            .map_err(|e| e.to_string())?
        {
            for c in &cands {
                outcomes.push(outcome_for(
                    c,
                    false,
                    Some("ticker blocked by in-flight/unreconciled signal".into()),
                    None,
                ));
            }
            continue;
        }

        // Highest expected net USD wins; missing estimates rank last.
        cands.sort_by(|a, b| {
            b.expected_net_usd
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&a.expected_net_usd.unwrap_or(f64::NEG_INFINITY))
        });
        let winner = cands.remove(0);
        for loser in &cands {
            outcomes.push(outcome_for(
                loser,
                false,
                Some("not selected: lower expected net USD than winning candidate".into()),
                None,
            ));
        }

        let res = execute_spatial_arbitrage_inner(
            &winner.symbol,
            &winner.buy_venue,
            &winner.sell_venue,
            winner.order_qty,
            winner.direction.clone(),
            None,
            None,
            None,
            None,
            None,
            winner.route_id.as_deref(),
            state,
        )
        .await;
        match res {
            Ok(dto) => outcomes.push(outcome_for(&winner, true, None, Some(dto))),
            Err(e) => outcomes.push(outcome_for(&winner, true, Some(e), None)),
        }
    }
    Ok(outcomes)
}

/// Durable signal execution record for the frontend (symbol filter optional).
#[tauri::command]
pub async fn list_signal_executions(
    symbol: Option<String>,
    status: Option<String>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::SignalExecution>, String> {
    let st = match status.as_deref() {
        Some(s) => Some(
            crate::state_store::SignalStatus::from_status(s)
                .ok_or_else(|| format!("unknown signal status '{s}'"))?,
        ),
        None => None,
    };
    state
        .state_store
        .list_signals(symbol.as_deref(), st)
        .map_err(|e| e.to_string())
}

/// Per-ticker execution/position state: whether the symbol is blocked, its
/// blocking signals, and the net filled position across all signals.
#[tauri::command]
pub async fn get_ticker_position_state(
    symbol: String,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<serde_json::Value, String> {
    let blocking = state
        .state_store
        .blocking_signals(&symbol)
        .map_err(|e| e.to_string())?;
    let net = state
        .state_store
        .ticker_net_position(&symbol)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "symbol": symbol,
        "blocked": !blocking.is_empty(),
        "blockingSignalIds": blocking.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "netPositionQty": net,
    }))
}

#[tauri::command]
pub async fn execute_spatial_candidates(
    candidates: Vec<SpatialCandidateInput>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    execute_spatial_candidates_impl(candidates, &state).await
}

pub async fn execute_cross_ticker_arbitrage_impl(
    long_symbol: &str,
    short_symbol: &str,
    long_venue: &str,
    short_venue: &str,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (
        long_symbol,
        short_symbol,
        long_venue,
        short_venue,
        order_qty,
        net_spread_bps,
    );
    Err("Native 'cross-ticker' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

pub async fn execute_funding_carry_impl(
    symbol: &str,
    long_venue: &str,
    short_venue: &str,
    trade_size_usd: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (symbol, long_venue, short_venue, trade_size_usd);
    Err("Native 'funding' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

pub async fn execute_prediction_arbitrage_impl(
    market_title: &str,
    buy_yes_venue: &str,
    buy_no_venue: &str,
    quantity: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (market_title, buy_yes_venue, buy_no_venue, quantity);
    Err("Native 'prediction' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

pub async fn get_risk_overview_impl(state: &AppEngineState) -> Result<RiskOverviewDto, String> {
    let cb = &state.circuit_breaker;
    let monitor = MarginMonitor::new(MarginMonitorConfig::default());

    // Paper exchange accounts are the source of truth for risk telemetry;
    // live execution is unavailable, so no venue adapters are queried here.
    let mut accounts = Vec::new();
    for (vid, paper) in state.paper_exchanges() {
        let mut acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
        acc.venue = vid.into();
        accounts.push(acc);
    }

    let total_equity_dec: Decimal = accounts.iter().map(|a| a.equity).sum();
    let total_free_margin_dec: Decimal = accounts.iter().map(|a| a.free_margin).sum();
    let total_mm_dec: Decimal = accounts.iter().map(|a| a.maintenance_margin).sum();
    let all_positions: Vec<_> = accounts.iter().flat_map(|a| a.positions.clone()).collect();

    // Feed observed equity into the breaker, then re-read its state so a
    // drawdown trip is reflected in this same snapshot.
    cb.update_equity(total_equity_dec);
    let is_halted = cb.is_tripped();
    let cb_str = match cb.trip_reason() {
        Some(reason) => format!("TRIPPED: {}", reason),
        None => {
            if is_halted {
                "TRIPPED: Manual halt".into()
            } else {
                "ACTIVE".into()
            }
        }
    };

    let delta_limit = state.delta_guard.read().max_unhedged_usd_per_asset();
    let delta_limit_f = delta_limit.to_string().parse::<f64>().unwrap_or(0.0);

    let net_deltas_map = {
        let delta_guard = state.delta_guard.read();
        delta_guard.calculate_net_delta(&all_positions)
    };

    let mut deltas_dto = Vec::new();
    for (asset, (qty, usd)) in net_deltas_map {
        let qty_f = qty.to_string().parse::<f64>().unwrap_or(0.0);
        let usd_f = usd.to_string().parse::<f64>().unwrap_or(0.0);
        deltas_dto.push(NetDeltaDto {
            asset,
            net_quantity: qty_f,
            net_exposure_usd: usd_f,
            delta_limit_usd: delta_limit_f,
            is_neutral: usd.abs() <= delta_limit,
        });
    }
    deltas_dto.sort_by(|a, b| a.asset.cmp(&b.asset));

    let total_equity = total_equity_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let total_free_margin = total_free_margin_dec
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let total_mm = total_mm_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let current_drawdown = cb
        .current_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let max_drawdown_limit = cb
        .max_daily_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let day_start = cb
        .day_start_equity()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let drawdown_pct = if day_start > 0.0 {
        (current_drawdown / day_start) * 100.0
    } else {
        0.0
    };

    let severity = |s: MarginHealthStatus| match s {
        MarginHealthStatus::Healthy => 0,
        MarginHealthStatus::Warning => 1,
        MarginHealthStatus::Critical => 2,
        MarginHealthStatus::Breached => 3,
    };
    let mut worst = MarginHealthStatus::Healthy;
    for acc in &accounts {
        let s = monitor.evaluate_account(acc);
        if severity(s) > severity(worst) {
            worst = s;
        }
    }
    let mut min_liq_dist_pct: Option<f64> = None;
    for pos in &all_positions {
        let s = monitor.evaluate_position(pos);
        if severity(s) > severity(worst) {
            worst = s;
        }
        if let Some(dist) = pos.liquidation_distance_pct() {
            let pct = (dist * dec!(100)).to_string().parse::<f64>().unwrap_or(0.0);
            min_liq_dist_pct = Some(match min_liq_dist_pct {
                Some(cur) => cur.min(pct),
                None => pct,
            });
        }
    }

    // Liquidation safety cannot be claimed without actual liquidation prices;
    // open positions with no liquidation data report UNKNOWN_LIQUIDATION
    // unless a margin severity already demands a stronger state.
    let margin_status = if is_halted {
        "HALTED"
    } else if all_positions.is_empty() {
        "FLAT"
    } else {
        match worst {
            MarginHealthStatus::Breached => "BREACHED",
            MarginHealthStatus::Critical => "CRITICAL",
            MarginHealthStatus::Warning => "WARNING",
            MarginHealthStatus::Healthy => {
                if min_liq_dist_pct.is_none() {
                    "UNKNOWN_LIQUIDATION"
                } else {
                    "SAFE"
                }
            }
        }
    };

    Ok(RiskOverviewDto {
        circuit_breaker_state: cb_str,
        is_halted,
        current_drawdown_usd: current_drawdown,
        max_drawdown_limit_usd: max_drawdown_limit,
        drawdown_pct,
        consecutive_failures: cb.consecutive_errors(),
        max_failures_allowed: cb.max_consecutive_errors(),
        total_portfolio_value_usd: total_equity,
        net_deltas: deltas_dto,
        margin_health: MarginHealthDto {
            maintenance_margin_req_usd: total_mm,
            margin_balance_usd: total_free_margin,
            margin_ratio_pct: if total_equity > 0.0 {
                (total_mm / total_equity) * 100.0
            } else {
                0.0
            },
            liquidation_distance_pct: if all_positions.is_empty() {
                0.0
            } else {
                min_liq_dist_pct.unwrap_or(0.0)
            },
            status: margin_status.into(),
        },
    })
}

pub fn reset_circuit_breaker_impl(state: &AppEngineState) -> Result<String, String> {
    state.circuit_breaker.reset();
    Ok("Circuit Breaker reset successfully. Trading resumed.".into())
}

pub fn trigger_emergency_killswitch_impl(state: &AppEngineState) -> Result<String, String> {
    state
        .circuit_breaker
        .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
    Ok("EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.".into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenResultDto {
    pub strategy_id: String,
    pub symbol: Option<String>,
    pub signals_processed: usize,
    pub signals_closed: usize,
    pub legs_closed: usize,
    /// Open signals whose legs could not be fully closed — residual exposure
    /// remains and those signals stay `unresolved` (ticker stays blocked).
    pub residual_signals: usize,
    pub notes: Vec<String>,
}

fn ledger_qty_to_dec(value: f64, what: &str) -> Result<Decimal, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{what} must be finite and nonnegative, got {value}"
        ));
    }
    Decimal::from_str_exact(&value.to_string())
        .map_err(|_| format!("{what} {value} is not representable as Decimal"))
}

fn dec_to_f64(value: Decimal, what: &str) -> Result<f64, String> {
    let f = value
        .to_string()
        .parse::<f64>()
        .map_err(|_| format!("{what} {value} is not representable as f64"))?;
    if !f.is_finite() {
        return Err(format!("{what} {value} is not finite as f64"));
    }
    Ok(f)
}

/// Place a reduce-only market order on a venue's paper exchange to close one
/// filled leg. Returns (filled_qty, avg_price, fee_usd, order_id).
async fn flatten_leg_on_paper(
    state: &AppEngineState,
    venue: &str,
    symbol: &str,
    side: Side,
    qty: Decimal,
) -> Result<Option<(Decimal, Decimal, Decimal, String)>, String> {
    let canonical = crate::state::canonical_venue_id(venue)
        .ok_or_else(|| format!("Unknown venue '{venue}'"))?;
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for paper execution"))?;
    let vid = adapter.venue_id();
    let paper = state
        .get_paper_exchange(canonical)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{venue}'"))?;

    let book = match state.market_data_hub.get_valid_orderbook(
        &vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {venue} orderbook for {symbol}: {e}"))?;
            validate_adapter_book(&b, &vid, symbol)
                .map_err(|e| format!("Flatten order book rejected: {e}"))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    paper.replace_orderbook(book).map_err(|e| e.to_string())?;

    if qty <= dec!(0) {
        return Err("Invalid leg quantity".to_string());
    }
    let mut order =
        arbitrager_core::order::OrderRequest::new_market(paper.venue_id(), symbol, side, qty);
    order.reduce_only = true;

    let resp = paper.place_order(order).await.map_err(|e| e.to_string())?;
    if resp.filled_qty <= dec!(0) {
        return Ok(None);
    }
    let px = resp
        .avg_fill_price
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
    Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)))
}

fn close_flattened_spatial_lot(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    family: &[crate::state_store::SignalExecution],
) {
    use crate::state_store::LegSide;
    use arbitrager_execution::virtual_ledger::CycleStatus;

    let venue_of = |side: LegSide| {
        sig.legs
            .iter()
            .find(|l| l.side == side)
            .and_then(|l| crate::state::canonical_venue_id(&l.venue))
    };
    let (Some(long_venue), Some(short_venue)) = (venue_of(LegSide::Long), venue_of(LegSide::Short))
    else {
        return;
    };

    #[derive(Default)]
    struct LegAgg {
        qty: f64,
        px_qty: f64,
        fee: f64,
    }
    let mut fills: HashMap<(String, &'static str), LegAgg> = HashMap::new();
    for child in family {
        for leg in &child.legs {
            let Some(canon) = crate::state::canonical_venue_id(&leg.venue) else {
                continue;
            };
            let (Some(px), qty, fee) = (leg.fill_price, leg.filled_qty, leg.fee_usd) else {
                continue;
            };
            if !(qty.is_finite() && qty > 0.0 && px.is_finite() && px > 0.0 && fee.is_finite()) {
                continue;
            }
            let agg = fills
                .entry((canon.to_string(), leg.side.as_str()))
                .or_default();
            agg.qty += qty;
            agg.px_qty += qty * px;
            agg.fee += fee;
        }
    }
    let exit_fees: f64 = fills.values().map(|a| a.fee).sum();
    let agg_for = |venue: &str, side: LegSide| fills.get(&(venue.to_string(), side.as_str()));
    let (Some(long_exit), Some(short_exit)) = (
        agg_for(long_venue, LegSide::Short),
        agg_for(short_venue, LegSide::Long),
    ) else {
        return;
    };

    let lot_id = {
        let ledger = state.virtual_ledger.read();
        ledger
            .lots
            .values()
            .find(|lot| {
                lot.strategy_id == "spatial_spread"
                    && lot.symbol == sig.symbol
                    && lot.status == CycleStatus::Open
                    && crate::state::canonical_venue_id(&lot.venue_long) == Some(long_venue)
                    && crate::state::canonical_venue_id(&lot.venue_short) == Some(short_venue)
                    && long_exit.qty + 1e-9 >= lot.long_qty
                    && short_exit.qty + 1e-9 >= lot.short_qty
            })
            .map(|l| l.id.clone())
    };
    if let Some(id) = lot_id {
        state.virtual_ledger.write().close_cycle(
            &id,
            long_exit.px_qty / long_exit.qty,
            short_exit.px_qty / short_exit.qty,
            exit_fees,
            CycleStatus::StoppedOut,
        );
    }
}

/// Emergency per-strategy exit: for every open signal of `strategy_id`
/// (optionally scoped to `symbol`), dispatch inverse reduce-only market orders
/// sized to that root family's attributed residual on each paper venue, and
/// mark the original `closed` only once its attributed legs are neutralized.
/// Bypasses pause/circuit-breaker because it is strictly risk-reducing;
/// refused in live mode until authenticated venue exits exist.
pub async fn flatten_ticker_positions_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    if *state.is_live_mode.read() {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }
    let _execution_guard = state.execution_lock.lock().await;

    let all_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let roots: Vec<crate::state_store::SignalExecution> = all_signals
        .iter()
        .filter(|s| {
            !s.id.starts_with("flat-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .cloned()
        .collect();

    let mut out = FlattenResultDto {
        strategy_id: sid.clone(),
        symbol: symbol.map(|s| s.to_string()),
        signals_processed: roots.len(),
        signals_closed: 0,
        legs_closed: 0,
        residual_signals: 0,
        notes: Vec::new(),
    };

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

    let papers = state.paper_exchanges();
    let opposite = |s: LegSide| match s {
        LegSide::Long => LegSide::Short,
        LegSide::Short => LegSide::Long,
    };

    struct LegExposure {
        canonical: &'static str,
        entry_side: LegSide,
        expected_pos_side: Side,
        entry_qty: Decimal,
    }
    struct CloseTarget {
        venue: String,
        account_id: Option<String>,
        order_side: Side,
        leg_side: LegSide,
        qty: Decimal,
    }

    let child_fills_on = |family: &[crate::state_store::SignalExecution],
                          canonical: &str,
                          side: LegSide|
     -> Result<Decimal, String> {
        let mut total = dec!(0);
        for child in family {
            for cl in &child.legs {
                if cl.side == side && crate::state::canonical_venue_id(&cl.venue) == Some(canonical)
                {
                    total += ledger_qty_to_dec(cl.filled_qty, "child leg fill")?;
                }
            }
        }
        Ok(total)
    };

    for sig in roots {
        let family_prefix = format!("flat-{}-", sig.id);
        let family: Vec<crate::state_store::SignalExecution> = all_signals
            .iter()
            .filter(|s| s.id.starts_with(&family_prefix))
            .cloned()
            .collect();

        let mut exposures: Vec<LegExposure> = Vec::new();
        let mut targets: Vec<CloseTarget> = Vec::new();
        let mut undetermined = false;
        let mut seen: std::collections::HashSet<(String, &'static str)> =
            std::collections::HashSet::new();
        let mut accounts: HashMap<&'static str, arbitrager_core::position::AccountSnapshot> =
            HashMap::new();
        let mut unreadable: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();

        for leg in &sig.legs {
            let Some(canonical) = crate::state::canonical_venue_id(&leg.venue) else {
                undetermined = true;
                out.notes.push(format!(
                    "{}: venue '{}' is not a canonical paper venue",
                    sig.id, leg.venue
                ));
                continue;
            };
            if !seen.insert((canonical.to_string(), leg.side.as_str())) {
                continue;
            }
            let entry_qty = ledger_qty_to_dec(leg.filled_qty, "root leg fill")?;
            let closed_qty = child_fills_on(&family, canonical, opposite(leg.side))?;
            let residual = (entry_qty - closed_qty).max(dec!(0));
            let expected_pos_side = match leg.side {
                LegSide::Long => Side::Buy,
                LegSide::Short => Side::Sell,
            };
            exposures.push(LegExposure {
                canonical,
                entry_side: leg.side,
                expected_pos_side,
                entry_qty,
            });
            if residual <= dec!(0) {
                continue;
            }
            if !accounts.contains_key(canonical) && !unreadable.contains(canonical) {
                match papers
                    .iter()
                    .find(|(vid, _)| *vid == canonical)
                    .map(|(_, p)| p.clone())
                {
                    Some(paper) => match paper.fetch_account().await {
                        Ok(acc) => {
                            accounts.insert(canonical, acc);
                        }
                        Err(e) => {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: cannot read {} paper account: {e}",
                                sig.id, leg.venue
                            ));
                        }
                    },
                    None => {
                        unreadable.insert(canonical);
                        out.notes.push(format!(
                            "{}: no paper account exists for venue '{}'",
                            sig.id, leg.venue
                        ));
                    }
                }
            }
            if unreadable.contains(canonical) {
                undetermined = true;
                continue;
            }
            let pos = accounts.get(canonical).and_then(|acc| {
                acc.positions
                    .iter()
                    .find(|p| p.symbol == sig.symbol && p.quantity > dec!(0))
            });
            match pos {
                None => {}
                Some(p) if p.side != expected_pos_side => {
                    undetermined = true;
                    out.notes.push(format!(
                        "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                        sig.id, leg.venue, p.side, leg.side
                    ));
                }
                Some(p) => {
                    let qty = residual.min(p.quantity);
                    targets.push(CloseTarget {
                        venue: leg.venue.clone(),
                        account_id: leg.account_id.clone(),
                        order_side: match p.side {
                            Side::Buy => Side::Sell,
                            Side::Sell => Side::Buy,
                        },
                        leg_side: opposite(leg.side),
                        qty,
                    });
                }
            }
        }

        if undetermined {
            continue;
        }

        let close_family = |note: &str| -> Result<(), String> {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Closed, Some(note))
                .map_err(|e| e.to_string())?;
            for child in state
                .state_store
                .list_signals(None, None)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|s| s.id.starts_with(&family_prefix) && s.status.counts_as_open())
            {
                state
                    .state_store
                    .update_signal_status(&child.id, SignalStatus::Closed, Some(note))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        };

        if targets.is_empty() {
            close_family(&format!("flatten {}: no residual paper position", sig.id))?;
            out.signals_closed += 1;
            close_flattened_spatial_lot(state, &sig, &family);
            continue;
        }

        let close_id = format!("flat-{}-{:04x}", sig.id, rand::random::<u16>());
        let child_legs = targets
            .iter()
            .map(|t| -> Result<NewSignalLeg, String> {
                let intended = dec_to_f64(t.qty, "close quantity")?;
                if intended <= 0.0 {
                    return Err(format!("close quantity {} is not positive", t.qty));
                }
                Ok(NewSignalLeg {
                    account_id: t.account_id.clone(),
                    venue: t.venue.clone(),
                    side: t.leg_side,
                    intended_qty: intended,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: close_id.clone(),
                strategy_id: sig.strategy_id.clone(),
                symbol: sig.symbol.clone(),
                route_id: sig.route_id.clone(),
                expected_net_usd: None,
                note: Some(format!("emergency flatten of {}", sig.id)),
                legs: child_legs,
            })
            .map_err(|e| format!("Failed to persist flatten signal record: {e}"))?;

        let mut leg_desc = Vec::new();
        for t in &targets {
            match flatten_leg_on_paper(state, &t.venue, &sig.symbol, t.order_side, t.qty).await {
                Ok(Some((qty, px, fee, oid))) => {
                    state
                        .state_store
                        .apply_leg_fill(
                            &close_id,
                            t.leg_side,
                            dec_to_f64(qty, "fill quantity")?,
                            dec_to_f64(px, "fill price")?,
                            dec_to_f64(fee, "fill fee")?,
                            Some(oid.as_str()),
                        )
                        .map_err(|e| e.to_string())?;
                    out.legs_closed += 1;
                    leg_desc.push(format!("{} {:?} {}/{}", t.venue, t.leg_side, qty, t.qty));
                    if qty < t.qty {
                        out.notes.push(format!(
                            "{}: {} leg partially closed {}/{}",
                            sig.id, t.venue, qty, t.qty
                        ));
                    }
                }
                Ok(None) => {
                    out.notes.push(format!(
                        "{}: {} leg got no fill (empty book)",
                        sig.id, t.venue
                    ));
                }
                Err(e) => {
                    out.notes
                        .push(format!("{}: {} leg close failed: {e}", sig.id, t.venue));
                }
            }
        }

        let family_now: Vec<crate::state_store::SignalExecution> = state
            .state_store
            .list_signals(None, None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|s| s.id.starts_with(&family_prefix))
            .collect();

        let mut fully_flat = true;
        for exp in &exposures {
            let residual = (exp.entry_qty
                - child_fills_on(&family_now, exp.canonical, opposite(exp.entry_side))?)
            .max(dec!(0));
            if residual <= dec!(0) {
                continue;
            }
            let Some((_, paper)) = papers.iter().find(|(vid, _)| *vid == exp.canonical) else {
                fully_flat = false;
                continue;
            };
            match paper.fetch_account().await {
                Ok(acc) => match acc
                    .positions
                    .iter()
                    .find(|p| p.symbol == sig.symbol && p.quantity > dec!(0))
                {
                    None => {}
                    Some(p) if p.side != exp.expected_pos_side => {
                        fully_flat = false;
                        out.notes.push(format!(
                            "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                            sig.id, exp.canonical, p.side, exp.entry_side
                        ));
                    }
                    Some(_) => fully_flat = false,
                },
                Err(_) => fully_flat = false,
            }
        }

        let note = format!("emergency flatten via {close_id}");
        if fully_flat {
            close_family(&note)?;
            out.signals_closed += 1;
            close_flattened_spatial_lot(state, &sig, &family_now);
        } else {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
            state
                .state_store
                .update_signal_status(&close_id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
        }

        let (venue_a, venue_b) = (
            sig.legs
                .first()
                .map(|l| l.venue.clone())
                .unwrap_or_default(),
            sig.legs.get(1).map(|l| l.venue.clone()).unwrap_or_default(),
        );
        state.execution_logs.write().push(ExecutionLogEntry {
            id: format!("exec-flatten-{}-{}", cycle, sig.id),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Flatten".into(),
            symbol: sig.symbol.clone(),
            venue_a,
            venue_b,
            leg_a_status: leg_desc.first().cloned().unwrap_or_else(|| "—".into()),
            leg_b_status: leg_desc.get(1).cloned().unwrap_or_else(|| "—".into()),
            success: fully_flat,
            gross_profit: 0.0,
            fees: 0.0,
            net_profit: 0.0,
            latency_us: 0,
            note: if fully_flat {
                format!("Emergency flatten closed signal {}", sig.id)
            } else {
                format!(
                    "Emergency flatten PARTIAL for signal {} — residual exposure",
                    sig.id
                )
            },
            direction: Some("Emergency Flatten".into()),
            trigger_spread_bps: None,
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: None,
        });
    }

    out.residual_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| {
            !s.id.starts_with("flat-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .count();

    let mut equity_after = dec!(0);
    for (_, paper) in state.paper_exchanges() {
        equity_after += paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
            .equity;
    }
    {
        let mut total_pnl = state.total_realized_profit.write();
        *total_pnl = equity_after - state.paper_baseline_equity();
    }
    state.circuit_breaker.update_equity(equity_after);

    Ok(out)
}

pub fn get_execution_logs_impl(state: &AppEngineState) -> Result<Vec<ExecutionLogEntry>, String> {
    Ok(state.execution_logs.read().clone())
}

pub fn set_live_mode_impl(enabled: bool, state: &AppEngineState) -> Result<bool, String> {
    *state.is_live_mode.write() = false;
    if enabled {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }
    Ok(false)
}

pub fn set_engine_paused_impl(paused: bool, state: &AppEngineState) -> Result<bool, String> {
    let mut p = state.is_paused.write();
    *p = paused;
    Ok(*p)
}

pub fn get_engine_settings_impl(state: &AppEngineState) -> Result<EngineSettingsDto, String> {
    let mut sanitized = state.settings.read().clone();
    sanitized.clear_credential_material();
    sanitized.configured_venues = state.vault.write().configured_venues().unwrap_or_default();
    Ok(sanitized)
}

pub fn save_engine_settings_impl(
    settings: EngineSettingsDto,
    state: &AppEngineState,
) -> Result<String, String> {
    if !settings.trading_mode.eq_ignore_ascii_case("paper") {
        return Err("trading_mode must be 'paper'; live execution is unavailable".into());
    }
    if !settings.max_slippage_bps.is_finite()
        || settings.max_slippage_bps < 0.1
        || settings.max_slippage_bps > 500.0
    {
        return Err("Max slippage tolerance must be between 0.1 and 500 BPS".into());
    }
    if !settings.min_depth_ratio.is_finite() || settings.min_depth_ratio < 1.0 {
        return Err("Min depth ratio must be at least 1.0".into());
    }
    if settings.engine_rpc_port == 0
        || settings.engine_ws_port == 0
        || settings.engine_rpc_port == settings.engine_ws_port
    {
        return Err("Engine ports must be nonzero and distinct".into());
    }
    match settings.bind_address.parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_loopback() => {}
        _ => return Err("bind_address must be a loopback IP address".into()),
    }
    if !settings.paper_starting_balance_usd.is_finite()
        || settings.paper_starting_balance_usd <= 0.0
        || settings.paper_starting_balance_usd > 1_000_000_000_000.0
    {
        return Err("Paper starting balance must be a positive number up to 1e12 USD".into());
    }
    let submitted = settings.submitted_credentials();
    if !submitted.is_empty() {
        // Bind any pre-account-model records to `{venue}-default` profiles
        // before writing so every exchange credential lands on a profile.
        ensure_credential_migration(state)?;
        let mut vault = state.vault.write();
        for cred in submitted {
            let venue_key = cred.venue.trim().to_lowercase();
            if venue_key == "remote_hsm" {
                // The HSM token is not an exchange account.
                vault
                    .upsert_credentials(vec![cred])
                    .map_err(|e| e.to_string())?;
                continue;
            }
            let profile = ensure_default_profile(state, &venue_key, cred.enabled)?;
            vault
                .upsert_profile_credentials(&profile.id, cred)
                .map_err(|e| e.to_string())?;
        }
    }
    // Toggle-only saves must still update enabled metadata on existing vault
    // records; a locked vault skips this silently since no secrets changed.
    // A venue credential stays enabled when either the engine flag is on or
    // an enabled account profile exists — a settings save can never disable a
    // venue the user explicitly switched on at the account level.
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    let enablement: Vec<(String, bool)> = vec![
        ("remote_hsm".into(), true),
        (
            "hyperliquid".into(),
            settings.hyperliquid_enabled || account_enabled("hyperliquid"),
        ),
        (
            "lighter".into(),
            settings.lighter_enabled || account_enabled("lighter"),
        ),
        (
            "vanta".into(),
            settings.vanta_enabled || account_enabled("vanta"),
        ),
        (
            "robinhood".into(),
            settings.robinhood_enabled || account_enabled("robinhood"),
        ),
        (
            "arcus".into(),
            settings.arcus_enabled || account_enabled("arcus"),
        ),
        (
            "popdex".into(),
            settings.popdex_enabled || account_enabled("popdex"),
        ),
        (
            "kraken_spot".into(),
            (settings.kraken_enabled && settings.kraken_spot_enabled)
                || account_enabled("kraken_spot"),
        ),
        (
            "kraken_futures".into(),
            (settings.kraken_enabled && settings.kraken_futures_enabled)
                || account_enabled("kraken_futures"),
        ),
        (
            "polymarket".into(),
            settings.polymarket_enabled || account_enabled("polymarket"),
        ),
        (
            "kalshi".into(),
            settings.kalshi_enabled || account_enabled("kalshi"),
        ),
    ];
    {
        let mut vault = state.vault.write();
        if vault.is_unlocked() {
            vault
                .update_enabled(&enablement)
                .map_err(|e| e.to_string())?;
        }
    }
    let mut sanitized = settings;
    sanitized.clear_credential_material();
    sanitized.configured_venues = state.vault.write().configured_venues().unwrap_or_default();
    // A changed starting balance reinitializes every paper account: fresh
    // equity, no open positions. Unrelated saves leave paper state alone.
    let prev_balance = state.settings.read().paper_starting_balance_usd;
    if sanitized.paper_starting_balance_usd != prev_balance {
        if let Some(eq) = Decimal::from_f64_retain(sanitized.paper_starting_balance_usd) {
            for (_, paper) in state.paper_exchanges() {
                paper.reset_account(eq);
            }
        }
    }
    *state.settings.write() = sanitized;
    Ok("Configuration applied to the current session".into())
}

// ---------------------------------------------------------------------------
// Tauri Command Handlers (thin IPC delegators)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_system_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SystemStatusDto, String> {
    get_system_status_impl(&state).await
}

#[tauri::command]
pub async fn fetch_orderbooks(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueOrderBookDto>, String> {
    fetch_orderbooks_impl(&symbol, &state).await
}

/// Venue 24h-volume maps behind a 5-minute shared cache — volumes move
/// slowly and both the liquidity sort and the signal-series weighting
/// consume them. Venues that fail to report are simply absent.
pub(crate) async fn cached_venue_volumes(
    state: &AppEngineState,
) -> HashMap<String, HashMap<String, f64>> {
    const TTL: std::time::Duration = std::time::Duration::from_secs(300);
    {
        let cache = state.volume_cache.lock();
        if let Some((t, m)) = cache.as_ref() {
            if t.elapsed() < TTL {
                return m.clone();
            }
        }
    }
    let mut map = HashMap::new();
    for (vid, _vname, adapter) in state.get_venues() {
        if let Ok(vols) = adapter.fetch_volumes_24h().await {
            map.insert(
                vid.to_string(),
                vols.into_iter()
                    .filter_map(|(k, v)| v.to_string().parse::<f64>().ok().map(|f| (k, f)))
                    .collect(),
            );
        }
    }
    *state.volume_cache.lock() = Some((std::time::Instant::now(), map.clone()));
    map
}

#[tauri::command]
pub async fn get_ticker_volumes(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueVolumesDto>, String> {
    let map = cached_venue_volumes(&state).await;
    Ok(map
        .into_iter()
        .map(|(venue_id, volumes)| VenueVolumesDto { venue_id, volumes })
        .collect())
}

// ---------------------------------------------------------------------------
// Signal Trend — durable dislocation series computed by arbitrager-math
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSeriesDto {
    pub symbol: String,
    /// "consensus" | "leader"
    pub mode: String,
    pub window_secs: u64,
    /// Total stored venue samples in the window (history-depth indicator).
    pub sample_count: u64,
    /// Earliest sample ever recorded for this symbol, if any.
    pub earliest_ts_ms: Option<i64>,
    /// Bucket timestamps (epoch ms), ascending.
    pub ts_ms: Vec<i64>,
    /// Sorted venue ids; `dev_bps[i][j]` is venue `venues[j]` at `ts_ms[i]`,
    /// or null when the venue had no observation in that bucket.
    pub venues: Vec<String>,
    /// venue_id -> display name for legend labels.
    pub venue_names: HashMap<String, String>,
    pub dev_bps: Vec<Vec<Option<f64>>>,
    /// Max/min deviation across venues present in each bucket; their
    /// difference is the gross pairwise spread.
    pub band_max_bps: Vec<f64>,
    pub band_min_bps: Vec<f64>,
}

pub async fn get_signal_series_impl(
    symbol: &str,
    mode: &str,
    window_secs: u64,
    state: &AppEngineState,
) -> Result<SignalSeriesDto, String> {
    validate_market_symbol(symbol)?;
    let window_secs = window_secs.clamp(10, 7 * 86_400); // 10s..7d
    let max_points = 500usize;
    // Bucket granularity: ~poll cadence for short windows, coarser for
    // hour/day horizons so long slices stay renderable.
    let bucket_ms = ((window_secs * 1000) / max_points as u64).max(1_000) as i64;
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - (window_secs * 1000) as i64 - bucket_ms;

    let store = state.state_store.clone();
    let sym = symbol.to_string();
    let (raw, earliest_ts_ms) = tokio::task::spawn_blocking(move || {
        let rows = store.spread_samples(&sym, since_ms, now_ms)?;
        let earliest = store.earliest_spread_sample_ts(&sym)?;
        Ok::<_, anyhow::Error>((rows, earliest))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let venue_names: HashMap<String, String> = state
        .get_venues()
        .into_iter()
        .map(|(vid, vname, _)| (vid.to_string(), vname.to_string()))
        .collect();

    let mut samples = Vec::with_capacity(raw.len());
    let mut sample_count = 0u64;
    for row in &raw {
        samples.push(MidSample {
            ts_ms: row.ts_ms,
            venue: row.venue_id.clone(),
            mid: row.mid,
        });
        sample_count += 1;
    }

    // Volume weights: venue_id -> this asset's reported 24h notional USD.
    // Asset naming differs per venue (PEPE / kPEPE / 1000PEPE).
    let base = symbol
        .split('-')
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let vol_maps = cached_venue_volumes(state).await;
    let candidates = [base.clone(), format!("K{}", base), format!("1000{}", base)];
    let mut weights = HashMap::new();
    for (vid, map) in &vol_maps {
        let v = candidates
            .iter()
            .filter_map(|c| {
                map.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(c))
                    .map(|(_, v)| *v)
            })
            .next()
            .unwrap_or(0.0);
        if v > 0.0 {
            weights.insert(vid.clone(), v);
        }
    }

    let mode_enum = match mode {
        "leader" => BaselineMode::Leader,
        _ => BaselineMode::Consensus,
    };
    let series = compute_deviation_series(&samples, &weights, mode_enum, bucket_ms, max_points);
    Ok(SignalSeriesDto {
        symbol: symbol.to_string(),
        mode: match mode_enum {
            BaselineMode::Consensus => "consensus",
            BaselineMode::Leader => "leader",
        }
        .to_string(),
        window_secs,
        sample_count,
        earliest_ts_ms,
        ts_ms: series.ts_ms,
        venues: series.venues,
        venue_names,
        dev_bps: series.dev_bps,
        band_max_bps: series.band_max_bps,
        band_min_bps: series.band_min_bps,
    })
}

#[tauri::command]
pub async fn get_signal_series(
    symbol: String,
    mode: Option<String>,
    window_secs: Option<u64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SignalSeriesDto, String> {
    get_signal_series_impl(
        &symbol,
        mode.as_deref().unwrap_or("consensus"),
        window_secs.unwrap_or(60),
        &state,
    )
    .await
}

#[tauri::command]
pub async fn scan_spatial_opportunities(
    symbol: String,
    min_bps: f64,
    order_qty: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<SpatialOpportunityDto>, String> {
    scan_spatial_opportunities_impl(&symbol, min_bps, order_qty, &state).await
}

#[tauri::command]
pub async fn scan_spot_opportunities(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<SpotOpportunityDto>, String> {
    scan_spot_opportunities_impl(&symbol, &state).await
}

#[tauri::command]
pub async fn execute_spot_arbitrage(
    params: SpotExecutionParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_spot_arbitrage_impl(params, &state).await
}

#[tauri::command]
pub async fn scan_funding_opportunities(
    symbol: String,
    trade_size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<FundingOpportunityDto>, String> {
    scan_funding_opportunities_impl(&symbol, trade_size_usd, &state).await
}

#[tauri::command]
pub async fn scan_prediction_opportunities(
    market: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<PredictionOpportunityDto>, String> {
    scan_prediction_opportunities_impl(&market, &state).await
}

#[tauri::command]
pub async fn run_stat_arb_step(
    step: usize,
    price_y: f64,
    price_x: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StatArbStepResultDto, String> {
    run_stat_arb_step_impl(step, price_y, price_x, &state)
}

#[tauri::command]
pub async fn compute_optimal_thresholds(
    theta: f64,
    mu: f64,
    sigma: f64,
    transaction_cost: f64,
) -> Result<BertramThresholdDto, String> {
    compute_optimal_thresholds_impl(theta, mu, sigma, transaction_cost)
}

#[tauri::command]
pub async fn fit_ou_process(prices: Vec<f64>, dt: f64) -> Result<OuFitDto, String> {
    fit_ou_process_impl(&prices, dt)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn execute_spatial_arbitrage(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_spatial_arbitrage_impl(
        &symbol,
        &buy_venue,
        &sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn execute_cross_ticker_arbitrage(
    long_symbol: String,
    short_symbol: String,
    long_venue: String,
    short_venue: String,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_cross_ticker_arbitrage_impl(
        &long_symbol,
        &short_symbol,
        &long_venue,
        &short_venue,
        order_qty,
        net_spread_bps,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn fetch_multi_market_quotes(
    symbols: Vec<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TickerMarketQuoteDto>, String> {
    fetch_multi_market_quotes_impl(symbols, &state).await
}

#[tauri::command]
pub async fn execute_funding_carry(
    symbol: String,
    long_venue: String,
    short_venue: String,
    trade_size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_funding_carry_impl(&symbol, &long_venue, &short_venue, trade_size_usd, &state).await
}

#[tauri::command]
pub async fn execute_prediction_arbitrage(
    market_title: String,
    buy_yes_venue: String,
    buy_no_venue: String,
    quantity: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_prediction_arbitrage_impl(
        &market_title,
        &buy_yes_venue,
        &buy_no_venue,
        quantity,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn get_risk_overview(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<RiskOverviewDto, String> {
    get_risk_overview_impl(&state).await
}

#[tauri::command]
pub async fn reset_circuit_breaker(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    reset_circuit_breaker_impl(&state)
}

#[tauri::command]
pub async fn trigger_emergency_killswitch(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    trigger_emergency_killswitch_impl(&state)
}

#[tauri::command]
pub async fn flatten_ticker_positions(
    symbol: Option<String>,
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_impl(symbol.as_deref(), &strategy_id, &state).await
}

#[tauri::command]
pub async fn get_execution_logs(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<ExecutionLogEntry>, String> {
    get_execution_logs_impl(&state)
}

#[tauri::command]
pub async fn set_live_mode(
    enabled: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    set_live_mode_impl(enabled, &state)
}

#[tauri::command]
pub async fn set_engine_paused(
    paused: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    set_engine_paused_impl(paused, &state)
}

#[tauri::command]
pub async fn get_engine_settings(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<EngineSettingsDto, String> {
    get_engine_settings_impl(&state)
}

#[tauri::command]
pub async fn save_engine_settings(
    settings: EngineSettingsDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    save_engine_settings_impl(settings, &state)
}

pub fn get_ticker_strategy_config_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<TickerStrategyConfigDto, String> {
    let configs = state.strategy_configs.read();
    if let Some(cfg) = configs.get(symbol) {
        Ok(cfg.clone())
    } else {
        Ok(TickerStrategyConfigDto {
            symbol: symbol.to_string(),
            ..TickerStrategyConfigDto::default()
        })
    }
}

pub fn save_ticker_strategy_config_impl(
    config: TickerStrategyConfigDto,
    state: &AppEngineState,
) -> Result<(), String> {
    if !config.target_profit_bps.is_finite() || config.target_profit_bps <= 0.0 {
        return Err("target_profit_bps must be finite and positive".into());
    }
    if config.window_percentile < 50 {
        return Err("window_percentile must be at least 50%".into());
    }
    if config.max_accumulated_units < 1 {
        return Err("max_accumulated_units must be at least 1".into());
    }
    if !(1..=5).contains(&config.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 5".into());
    }
    if !config.unit_size.is_finite() || config.unit_size <= 0.0 {
        return Err("unit_size must be finite and positive".into());
    }
    let mut configs = state.strategy_configs.write();
    configs.insert(config.symbol.clone(), config);
    Ok(())
}

pub fn get_all_ticker_strategy_configs_impl(
    state: &AppEngineState,
) -> Result<Vec<TickerStrategyConfigDto>, String> {
    let configs = state.strategy_configs.read();
    Ok(configs.values().cloned().collect())
}

#[tauri::command]
pub async fn get_ticker_strategy_config(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<TickerStrategyConfigDto, String> {
    get_ticker_strategy_config_impl(&symbol, &state)
}

#[tauri::command]
pub async fn save_ticker_strategy_config(
    config: TickerStrategyConfigDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    save_ticker_strategy_config_impl(config, &state)
}

#[tauri::command]
pub async fn get_all_ticker_strategy_configs(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TickerStrategyConfigDto>, String> {
    get_all_ticker_strategy_configs_impl(&state)
}

pub fn get_vault_status_impl(state: &AppEngineState) -> VaultStatus {
    let mut vault = state.vault.write();
    vault.get_status()
}

pub fn initialize_vault_impl(
    password: &str,
    state: &AppEngineState,
) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.initialize(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
    Ok(status)
}

pub fn unlock_vault_impl(password: &str, state: &AppEngineState) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.unlock(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
    Ok(status)
}

pub fn lock_vault_impl(state: &AppEngineState) -> VaultStatus {
    let mut vault = state.vault.write();
    vault.lock();
    vault.get_status()
}

const DELETABLE_CREDENTIAL_VENUES: &[&str] = &[
    "remote_hsm",
    "hyperliquid",
    "lighter",
    "vanta",
    "robinhood",
    "arcus",
    "popdex",
    "kraken_spot",
    "kraken_futures",
    "polymarket",
    "kalshi",
];

pub fn delete_exchange_credentials_impl(
    venue: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let normalized = venue.trim().to_lowercase();
    if !DELETABLE_CREDENTIAL_VENUES.contains(&normalized.as_str()) {
        return Err(format!(
            "Credential deletion is not supported for venue '{}'",
            venue
        ));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before removing credentials.".into());
    }
    vault
        .delete_credentials(&normalized)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Account profiles and strategy-level account routes
// ---------------------------------------------------------------------------

/// Sanitized account view for the settings UI: durable profile metadata plus
/// credential-presence flags. Never contains secret material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    pub venue: String,
    pub label: String,
    pub enabled: bool,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
    /// `None` while the vault is locked (presence is unknowable); otherwise
    /// whether any credential material is stored for this account.
    pub credentials_configured: Option<bool>,
    pub has_api_key: bool,
    pub has_api_secret: bool,
    pub has_passphrase: bool,
    pub has_account_id: bool,
    pub has_wallet_address: bool,
    pub account_index: Option<u64>,
    pub subaccount_index: Option<u64>,
    pub api_key_index: Option<u64>,
    pub has_private_key: bool,
    /// "standard" | "plus" | "premium" — Lighter-family tier drives the
    /// venue's shared-IP rate budget and fee adjustments.
    pub account_tier: String,
}

// Intentionally no Debug: may transiently hold credential material.
#[derive(Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredentialInput {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub wallet_address: Option<String>,
    #[serde(default)]
    pub account_index: Option<u64>,
    #[serde(default)]
    pub subaccount_index: Option<u64>,
    #[serde(default)]
    pub api_key_index: Option<u64>,
    #[serde(default)]
    pub private_key: Option<String>,
}

impl AccountCredentialInput {
    fn has_material(&self) -> bool {
        !self.api_key.trim().is_empty()
            || !self.api_secret.trim().is_empty()
            || self
                .passphrase
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self
                .account_id
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self
                .wallet_address
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self.account_index.is_some()
            || self.subaccount_index.is_some()
            || self.api_key_index.is_some()
            || self
                .private_key
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
    }

    fn into_record(self, venue: &str, profile_id: &str, enabled: bool) -> ExchangeCredentials {
        let trimmed_opt = |v: &Option<String>| {
            v.as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        ExchangeCredentials {
            venue: venue.to_string(),
            profile_id: Some(profile_id.to_string()),
            api_key: self.api_key.trim().to_string(),
            api_secret: self.api_secret.trim().to_string(),
            passphrase: trimmed_opt(&self.passphrase),
            account_id: trimmed_opt(&self.account_id),
            wallet_address: trimmed_opt(&self.wallet_address),
            account_index: self.account_index,
            subaccount_index: self.subaccount_index,
            api_key_index: self.api_key_index,
            private_key: trimmed_opt(&self.private_key),
            enabled,
        }
    }
}

// Intentionally no Debug: `credentials` may hold secret material.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpsertInput {
    /// Existing account ID for updates; `None` creates a new account.
    #[serde(default)]
    pub id: Option<String>,
    /// Venue ID. Required on create; verified immutable on update.
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub make_default: bool,
    /// "standard" | "plus" | "premium". `None` preserves the stored tier on
    /// update and means standard on create.
    #[serde(default)]
    pub account_tier: Option<String>,
    #[serde(default)]
    pub credentials: Option<AccountCredentialInput>,
}

// Intentionally no Debug.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyRouteUpsertInput {
    /// Existing route ID for updates; `None` creates a new route.
    #[serde(default)]
    pub id: Option<String>,
    pub strategy_id: String,
    #[serde(default)]
    pub label: String,
    pub account_a_id: String,
    pub account_b_id: String,
    #[serde(default = "default_route_enabled")]
    pub enabled: bool,
}

fn default_route_enabled() -> bool {
    true
}

fn is_valid_account_id(raw: &str) -> bool {
    let v = raw.trim();
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Fetch (creating when necessary) the `{venue}-default` account profile
/// used by the legacy flat settings credential fields. The legacy venue
/// toggle is mirrored onto the default account's enabled switch; label and
/// default status are preserved for existing profiles.
fn ensure_default_profile(
    state: &AppEngineState,
    venue: &str,
    enabled: bool,
) -> Result<AccountProfile, String> {
    let pid = format!("{venue}-default");
    match state
        .state_store
        .get_account(&pid)
        .map_err(|e| e.to_string())?
    {
        Some(existing) => {
            if existing.venue != venue {
                return Err(format!("account ID {pid} is bound to a different venue"));
            }
            if existing.enabled != enabled {
                state
                    .state_store
                    .upsert_account(
                        &pid,
                        venue,
                        &existing.label,
                        enabled,
                        existing.is_default,
                        &existing.account_tier,
                    )
                    .map_err(|e| e.to_string())
            } else {
                Ok(existing)
            }
        }
        None => state
            .state_store
            .upsert_account(&pid, venue, "Default", enabled, true, "standard")
            .map_err(|e| e.to_string()),
    }
}

/// Migrate legacy venue-keyed credential records onto `{venue}-default`
/// account profiles and prune bound records whose profile no longer exists.
/// No-op while the vault is locked. `remote_hsm` is not an exchange account
/// and is intentionally left unbound.
fn ensure_credential_migration(state: &AppEngineState) -> Result<(), String> {
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Ok(());
    }

    // Prune bound records whose profile was deleted (e.g. credential cleanup
    // failed after the account row was removed).
    let presence = vault.credential_presence().map_err(|e| e.to_string())?;
    for p in presence.iter() {
        if let Some(pid) = &p.profile_id {
            let exists = state
                .state_store
                .get_account(pid)
                .map_err(|e| e.to_string())?
                .is_some();
            if !exists {
                vault
                    .delete_profile_credentials(pid)
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    for cred in vault.unbound_credentials().map_err(|e| e.to_string())? {
        if cred.venue == "remote_hsm" {
            continue;
        }
        let pid = format!("{}-default", cred.venue);
        if !is_valid_account_id(&pid) {
            tracing::warn!(
                "skipping migration of credential record with invalid venue '{}'",
                cred.venue
            );
            continue;
        }
        match state
            .state_store
            .get_account(&pid)
            .map_err(|e| e.to_string())?
        {
            Some(existing) => {
                if existing.venue != cred.venue {
                    return Err(format!(
                        "cannot migrate {} credentials: {pid} is bound to another venue",
                        cred.venue
                    ));
                }
            }
            None => {
                state
                    .state_store
                    .upsert_account(&pid, &cred.venue, "Default", cred.enabled, true, "standard")
                    .map_err(|e| e.to_string())?;
            }
        }
        vault
            .bind_profile(&cred.venue, &pid)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn account_dto(
    profile: &AccountProfile,
    presence: Option<&CredentialPresence>,
    vault_locked: bool,
) -> AccountDto {
    let configured = presence.is_some_and(|p| {
        p.has_api_key
            || p.has_api_secret
            || p.has_passphrase
            || p.has_account_id
            || p.has_wallet_address
            || p.account_index.is_some()
            || p.subaccount_index.is_some()
            || p.api_key_index.is_some()
            || p.has_private_key
    });
    AccountDto {
        id: profile.id.clone(),
        venue: profile.venue.clone(),
        label: profile.label.clone(),
        enabled: profile.enabled,
        is_default: profile.is_default,
        created_at: profile.created_at.clone(),
        updated_at: profile.updated_at.clone(),
        credentials_configured: if vault_locked { None } else { Some(configured) },
        has_api_key: presence.is_some_and(|p| p.has_api_key),
        has_api_secret: presence.is_some_and(|p| p.has_api_secret),
        has_passphrase: presence.is_some_and(|p| p.has_passphrase),
        has_account_id: presence.is_some_and(|p| p.has_account_id),
        has_wallet_address: presence.is_some_and(|p| p.has_wallet_address),
        has_private_key: presence.is_some_and(|p| p.has_private_key),
        account_index: presence.and_then(|p| p.account_index),
        subaccount_index: presence.and_then(|p| p.subaccount_index),
        api_key_index: presence.and_then(|p| p.api_key_index),
        account_tier: profile.account_tier.clone(),
    }
}

/// Merge durable profiles with sanitized vault credential presence.
fn accounts_with_presence(
    state: &AppEngineState,
    profiles: Vec<AccountProfile>,
) -> Result<Vec<AccountDto>, String> {
    let (presence, locked) = {
        let mut vault = state.vault.write();
        let locked = !vault.is_unlocked();
        let presence = if locked {
            Vec::new()
        } else {
            vault.credential_presence().map_err(|e| e.to_string())?
        };
        (presence, locked)
    };
    let by_profile: std::collections::HashMap<&str, &CredentialPresence> = presence
        .iter()
        .filter_map(|p| p.profile_id.as_deref().map(|id| (id, p)))
        .collect();
    Ok(profiles
        .iter()
        .map(|p| account_dto(p, by_profile.get(p.id.as_str()).copied(), locked))
        .collect())
}

pub fn list_accounts_impl(
    venue: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<AccountDto>, String> {
    ensure_credential_migration(state)?;
    let profiles = state
        .state_store
        .list_accounts(venue)
        .map_err(|e| e.to_string())?;
    accounts_with_presence(state, profiles)
}

pub fn upsert_account_impl(
    input: AccountUpsertInput,
    state: &AppEngineState,
) -> Result<AccountDto, String> {
    ensure_credential_migration(state)?;

    let profile = match &input.id {
        Some(id) => {
            let existing = state
                .state_store
                .get_account(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("account {id} does not exist"))?;
            if !input.venue.trim().is_empty()
                && !input.venue.trim().eq_ignore_ascii_case(&existing.venue)
            {
                return Err("account venue is immutable once created".into());
            }
            state
                .state_store
                .upsert_account(
                    id,
                    &existing.venue,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input
                        .account_tier
                        .as_deref()
                        .unwrap_or(&existing.account_tier),
                )
                .map_err(|e| e.to_string())?
        }
        None => {
            let venue_lc = input.venue.trim().to_lowercase();
            if venue_lc.is_empty() {
                return Err("venue is required when creating an account".into());
            }
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", venue_lc, rand::random::<u32>());
                if state
                    .state_store
                    .get_account(&candidate)
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    break;
                }
            }
            state
                .state_store
                .upsert_account(
                    &candidate,
                    &venue_lc,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input.account_tier.as_deref().unwrap_or("standard"),
                )
                .map_err(|e| e.to_string())?
        }
    };

    let mut cred_error: Option<String> = None;
    if let Some(creds) = &input.credentials {
        if creds.has_material() {
            let mut vault = state.vault.write();
            if !vault.is_unlocked() {
                cred_error = Some("vault is locked".to_string());
            } else {
                let record =
                    creds
                        .clone()
                        .into_record(&profile.venue, &profile.id, profile.enabled);
                if let Err(e) = vault.upsert_profile_credentials(&profile.id, record) {
                    cred_error = Some(e.to_string());
                }
            }
        }
    }
    // Mirror the account switch onto the stored record's metadata flag when
    // the vault is reachable; the profile remains the authoritative switch.
    {
        let mut vault = state.vault.write();
        if vault.is_unlocked() {
            let _ = vault.update_profile_enabled(&[(profile.id.clone(), profile.enabled)]);
        }
    }

    let dto = accounts_with_presence(state, vec![profile])?
        .into_iter()
        .next()
        .ok_or_else(|| "account disappeared after save".to_string())?;
    if let Some(err) = cred_error {
        return Err(format!(
            "account saved but credentials were not stored: {err}"
        ));
    }
    Ok(dto)
}

/// Delete an account profile and its bound credential record. Route
/// references are checked before anything is removed so a foreign-key block
/// can never destroy secrets; the vault must be unlocked so bound secrets
/// are never orphaned.
pub fn delete_account_impl(account_id: &str, state: &AppEngineState) -> Result<bool, String> {
    let refs = state
        .state_store
        .routes_referencing(account_id)
        .map_err(|e| e.to_string())?;
    if !refs.is_empty() {
        let route_list = refs
            .iter()
            .map(|r| format!("{}:{}", r.strategy_id, r.label))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "account {account_id} is referenced by {} strategy route(s) ({route_list}); remove or reassign them first",
            refs.len()
        ));
    }
    // Accounts that own recorded position legs keep their attribution; delete
    // is refused so signal history never loses its account binding.
    if state
        .state_store
        .account_has_signal_legs(account_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "account {account_id} has recorded signal position history; disable the account instead of deleting it"
        ));
    }
    {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before deleting an account.".into());
        }
    }
    let deleted = state
        .state_store
        .delete_account(account_id)
        .map_err(|e| e.to_string())?;
    if deleted {
        let mut vault = state.vault.write();
        if let Err(e) = vault.delete_profile_credentials(account_id) {
            // Orphaned bound records are pruned by ensure_credential_migration
            // on the next unlock; surface in logs only.
            tracing::warn!("account {account_id} deleted but credential cleanup failed: {e}");
        }
    }
    Ok(deleted)
}

/// Removes only the credential records bound to one account profile; the
/// account itself is kept. Distinct from `delete_account_impl`, which removes
/// the whole profile, and from `delete_exchange_credentials_impl`, which is
/// venue-wide.
pub fn delete_account_credentials_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before removing credentials.".into());
    }
    vault
        .delete_profile_credentials(account_id)
        .map_err(|e| e.to_string())
}

/// Returns the decrypted credentials bound to one account profile for the
/// local settings UI. Requires an unlocked vault; unknown accounts and locked
/// vaults fail closed. Values are returned over local IPC only — they are
/// never logged or persisted anywhere new.
pub fn get_account_credentials_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<AccountCredentialInput, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before viewing credentials.".into());
    }
    let data = vault.get_data().map_err(|e| e.to_string())?;
    let cred = data
        .credentials
        .iter()
        .find(|c| c.profile_id.as_deref() == Some(account_id));
    match cred {
        Some(c) => Ok(AccountCredentialInput {
            api_key: c.api_key.clone(),
            api_secret: c.api_secret.clone(),
            passphrase: c.passphrase.clone(),
            account_id: c.account_id.clone(),
            wallet_address: c.wallet_address.clone(),
            account_index: c.account_index,
            subaccount_index: c.subaccount_index,
            api_key_index: c.api_key_index,
            private_key: c.private_key.clone(),
        }),
        None => Err(format!(
            "No credentials configured for account '{account_id}'"
        )),
    }
}

pub fn set_default_account_impl(
    venue: &str,
    account_id: &str,
    state: &AppEngineState,
) -> Result<(), String> {
    state
        .state_store
        .set_default_account(venue, account_id)
        .map_err(|e| e.to_string())
}

pub fn list_strategy_routes_impl(
    strategy_id: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .list_strategy_routes(strategy_id)
        .map_err(|e| e.to_string())
}

pub fn upsert_strategy_route_impl(
    input: StrategyRouteUpsertInput,
    state: &AppEngineState,
) -> Result<StrategyAccountRoute, String> {
    let id = match &input.id {
        Some(id) => id.clone(),
        None => {
            let sid = input.strategy_id.trim().to_lowercase();
            let existing: std::collections::HashSet<String> = state
                .state_store
                .list_strategy_routes(None)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| r.id)
                .collect();
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", sid, rand::random::<u32>());
                if !existing.contains(&candidate) {
                    break;
                }
            }
            candidate
        }
    };
    // Labels are required by the store; an omitted label preserves the
    // existing route's label on update, or derives a readable default.
    let label = if input.label.trim().is_empty() {
        let existing_label = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.label);
        match existing_label {
            Some(l) => l,
            None => {
                let name = |aid: &str| -> String {
                    state
                        .state_store
                        .get_account(aid)
                        .ok()
                        .flatten()
                        .map(|a| format!("{}:{}", a.venue, a.label))
                        .unwrap_or_else(|| aid.to_string())
                };
                format!(
                    "{} \u{2194} {}",
                    name(&input.account_a_id),
                    name(&input.account_b_id)
                )
            }
        }
    } else {
        input.label.clone()
    };
    state
        .state_store
        .upsert_strategy_route(
            &id,
            &input.strategy_id,
            &label,
            &input.account_a_id,
            &input.account_b_id,
            input.enabled,
        )
        .map_err(|e| e.to_string())
}

pub fn delete_strategy_route_impl(route_id: &str, state: &AppEngineState) -> Result<bool, String> {
    state
        .state_store
        .delete_strategy_route(route_id)
        .map_err(|e| e.to_string())
}

/// Eligible account routes for a strategy across a venue pair: enabled
/// routes whose two endpoint accounts are enabled and sit on the two
/// venues (in either order).
pub fn eligible_strategy_routes_impl(
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b)
        .map_err(|e| e.to_string())
}

pub async fn get_venue_balances_impl(
    state: &AppEngineState,
) -> Result<Vec<VenueBalanceInfo>, String> {
    if *state.is_live_mode.read() {
        return Ok(Vec::new());
    }
    let monitor = MarginMonitor::new(MarginMonitorConfig::default());
    let mut results = Vec::new();

    // Only venues currently admitted for paper execution report balances;
    // figures come from the real simulated account snapshots.
    for (vid, name, _adapter) in state.get_venues() {
        let Some(paper) = state.get_paper_exchange(vid) else {
            continue;
        };
        let account = paper.fetch_account().await.map_err(|e| e.to_string())?;

        let free_margin_usd = account
            .free_margin
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);
        let equity_usd = account.equity.to_string().parse::<f64>().unwrap_or(0.0);
        let margin_rate_pct = account
            .margin_usage_pct
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);

        let (status, action) = match monitor.evaluate_account(&account) {
            MarginHealthStatus::Healthy => ("HEALTHY".to_string(), None),
            MarginHealthStatus::Warning => (
                "LOW_MARGIN".to_string(),
                Some(format!("Warning: {} margin utilization elevated.", name)),
            ),
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => (
                "DEPLETED".to_string(),
                Some(format!(
                    "Critical: {} margin utilization breached. Execution halted on this leg.",
                    name
                )),
            ),
        };

        results.push(VenueBalanceInfo {
            venue: vid.to_string(),
            equity_usd,
            available_margin_usd: free_margin_usd,
            margin_rate_pct,
            status,
            recommended_action: action,
        });
    }

    Ok(results)
}

pub fn get_strategy_performance_impl(
    strategy_id: Option<String>,
    state: &AppEngineState,
) -> Result<Vec<StrategyMetrics>, String> {
    let ledger = state.virtual_ledger.read();
    let strat_ids = match strategy_id {
        Some(s) => vec![s],
        None => vec![
            "spatial_spread".to_string(),
            "stat_arb".to_string(),
            "funding_rate".to_string(),
            "prediction_parity".to_string(),
        ],
    };

    let metrics = strat_ids
        .into_iter()
        .map(|id| ledger.compute_strategy_metrics(&id))
        .collect();

    Ok(metrics)
}

pub async fn run_wfo_calibration_impl(
    _symbol: &str,
    _state: &AppEngineState,
) -> Result<WfoOptimizationResult, String> {
    Err("WFO calibration is unavailable until sufficient recorded quote history exists".into())
}

#[tauri::command]
pub async fn get_vault_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    Ok(get_vault_status_impl(&state))
}

#[tauri::command]
pub async fn initialize_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    initialize_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn unlock_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    unlock_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn lock_vault(state: State<'_, Arc<AppEngineState>>) -> Result<VaultStatus, String> {
    Ok(lock_vault_impl(&state))
}

#[tauri::command]
pub async fn delete_exchange_credentials(
    venue: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_exchange_credentials_impl(&venue, &state)
}

#[tauri::command]
pub async fn list_accounts(
    venue: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<AccountDto>, String> {
    list_accounts_impl(venue.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_account(
    input: AccountUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AccountDto, String> {
    upsert_account_impl(input, &state)
}

#[tauri::command]
pub async fn delete_account(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_impl(&account_id, &state)
}

#[tauri::command]
pub async fn delete_account_credentials(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_credentials_impl(&account_id, &state)
}

#[tauri::command]
pub async fn get_account_credentials(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AccountCredentialInput, String> {
    get_account_credentials_impl(&account_id, &state)
}

#[tauri::command]
pub async fn set_default_account(
    venue: String,
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    set_default_account_impl(&venue, &account_id, &state)
}

#[tauri::command]
pub async fn list_strategy_routes(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    list_strategy_routes_impl(strategy_id.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_strategy_route(
    input: StrategyRouteUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StrategyAccountRoute, String> {
    upsert_strategy_route_impl(input, &state)
}

#[tauri::command]
pub async fn delete_strategy_route(
    route_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_strategy_route_impl(&route_id, &state)
}

#[tauri::command]
pub async fn eligible_strategy_routes(
    strategy_id: String,
    venue_a: String,
    venue_b: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    eligible_strategy_routes_impl(&strategy_id, &venue_a, &venue_b, &state)
}

#[tauri::command]
pub async fn get_venue_balances(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueBalanceInfo>, String> {
    get_venue_balances_impl(&state).await
}

#[tauri::command]
pub async fn get_strategy_performance(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyMetrics>, String> {
    get_strategy_performance_impl(strategy_id, &state)
}

#[tauri::command]
pub async fn run_wfo_calibration(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<WfoOptimizationResult, String> {
    run_wfo_calibration_impl(&symbol, &state).await
}

// ---------------------------------------------------------------------------
// Spread Range Monitor — systematic cross-venue opportunity radar
// ---------------------------------------------------------------------------

fn monitor_engine(
    state: &AppEngineState,
) -> Result<Arc<crate::monitor::SpreadMonitorEngine>, String> {
    state
        .monitor
        .lock()
        .clone()
        .ok_or_else(|| "spread monitor is not running".to_string())
}

#[tauri::command]
pub async fn get_monitor_snapshot(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Option<crate::monitor::MonitorSnapshotDto>, String> {
    Ok(monitor_engine(&state)?.snapshot())
}

#[tauri::command]
pub async fn get_monitor_config(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::monitor::MonitorConfigDto, String> {
    Ok(monitor_engine(&state)?.config())
}

#[tauri::command]
pub async fn set_monitor_config(
    config: crate::monitor::MonitorConfigDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    monitor_engine(&state)?.set_config(config, &state)
}

#[tauri::command]
pub async fn set_ticker_exempted(
    symbol: String,
    exempt: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    let sym = crate::monitor::canonical_base(&symbol);
    if sym.is_empty() || sym.len() > 64 {
        return Err("invalid symbol".into());
    }
    state
        .state_store
        .set_ticker_exempt(&sym, exempt)
        .map_err(|e| e.to_string())?;
    // Force the watchlist to re-derive on the next tick.
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(())
}

/// Currently exempted symbols (canonical base form).
#[tauri::command]
pub async fn get_exempted_tickers(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<String>, String> {
    state
        .state_store
        .exempt_tickers()
        .map_err(|e| e.to_string())
}

/// Global "reveal all": clears every ticker exemption. Returns the number
/// of exemptions removed.
#[tauri::command]
pub async fn reveal_all_tickers(state: State<'_, Arc<AppEngineState>>) -> Result<usize, String> {
    let n = state
        .state_store
        .clear_ticker_exemptions()
        .map_err(|e| e.to_string())?;
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(n)
}

/// Per-row Execute: re-validates the opportunity against the latest
/// snapshot (must still be qualified and unblocked), converts the USD size
/// to base quantity on the live book, then dispatches the existing guarded
/// paper execution path.
#[tauri::command]
pub async fn execute_monitor_opportunity(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    let engine = monitor_engine(&state)?;
    let sym = crate::monitor::canonical_base(&symbol);
    if !(size_usd.is_finite() && size_usd > 0.0) {
        return Err("invalid size".into());
    }
    let snap = engine
        .snapshot()
        .ok_or_else(|| "monitor has not produced a snapshot yet".to_string())?;
    let row = snap
        .rows
        .iter()
        .find(|r| r.symbol == sym && r.buy_venue == buy_venue && r.sell_venue == sell_venue)
        .ok_or_else(|| "opportunity is no longer tracked".to_string())?;
    if !row.executable {
        return Err(row
            .block_reason
            .clone()
            .unwrap_or_else(|| "opportunity is not executable".into()));
    }
    // Convert USD size to base qty at the current buy-side ask.
    let book = state
        .market_data_hub
        .get_valid_orderbook(
            &arbitrager_core::types::VenueId(buy_venue.clone()),
            &sym,
            Some(std::time::Duration::from_secs(60)),
        )
        .ok_or_else(|| "buy-side orderbook went stale".to_string())?;
    let ask = book
        .asks
        .first()
        .and_then(|l| l.price.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0)
        .ok_or_else(|| "no ask on buy-side book".to_string())?;
    let qty = size_usd / ask;
    execute_spatial_arbitrage_impl(
        &sym,
        &buy_venue,
        &sell_venue,
        qty,
        Some("OPEN".into()),
        row.current_edge_bps,
        row.baseline_bps,
        None,
        None,
        None,
        &state,
    )
    .await
}

// ---------------------------------------------------------------------------
// Unit Tests (deep verification of all command business logic and edge cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_state_with_venues() -> AppEngineState {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
            s.vanta_enabled = true;
            s.robinhood_enabled = true;
            s.arcus_enabled = true;
            s.popdex_enabled = true;
        }
        state
    }

    fn test_state_with_vault(tag: &str) -> (AppEngineState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "arbitrager_cmd_vault_{}_{}",
            tag,
            rand::random::<u64>()
        ));
        let state = AppEngineState::new_with_vault_path(dir.join("vault.enc"));
        (state, dir)
    }

    fn seed_test_book(
        state: &AppEngineState,
        venue_id: &str,
        symbol: &str,
        bid: Decimal,
        ask: Decimal,
    ) {
        let mut book = arbitrager_core::quote::OrderBook::new(
            arbitrager_core::types::VenueId::new(venue_id),
            symbol.to_string(),
        );
        book.bids
            .push(arbitrager_core::quote::BookLevel::new(bid, dec!(10.0)));
        book.asks
            .push(arbitrager_core::quote::BookLevel::new(ask, dec!(10.0)));
        book.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(book);
    }

    fn seed_test_funding(state: &AppEngineState, venue_id: &str, symbol: &str) {
        state
            .market_data_hub
            .update_funding(arbitrager_core::quote::FundingRate {
                venue: arbitrager_core::types::VenueId::new(venue_id),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });
    }

    #[tokio::test]
    async fn test_system_status_command() {
        let state = AppEngineState::new();
        let status = get_system_status_impl(&state)
            .await
            .expect("status should succeed");

        assert!(!status.is_live_mode);
        assert_eq!(status.total_realized_pnl, 0.0);
        assert!(!status.circuit_breaker_tripped);
        assert_eq!(status.circuit_breaker_status, "ACTIVE");
        assert_eq!(status.active_venues.len(), 17);
        for v in &status.active_venues {
            assert_eq!(v.status, "OFFLINE");
            assert_eq!(v.latency_ms, None);
        }
        assert!(!status.is_paused);
    }

    #[tokio::test]
    async fn test_system_status_reflects_account_level_enablement() {
        let (state, dir) = test_state_with_vault("status-acct");

        // No flags, no accounts: everything OFFLINE.
        let status = get_system_status_impl(&state).await.expect("status");
        assert!(status.active_venues.iter().all(|v| v.status == "OFFLINE"));

        // An enabled account profile admits the venue even with the engine
        // flag unset — status must match what get_venues actually admits.
        upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "lighter".into(),
                label: "L1".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create account");

        let status = get_system_status_impl(&state).await.expect("status");
        let lighter = status
            .active_venues
            .iter()
            .find(|v| v.id == "lighter")
            .expect("lighter row");
        assert_eq!(lighter.status, "READ_ONLY");
        assert_eq!(
            status
                .active_venues
                .iter()
                .filter(|v| v.status != "OFFLINE")
                .count(),
            1
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_pause_and_continue_trading_state() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));

        assert!(!*state.is_paused.read());

        // 1. Pause engine
        let p = set_engine_paused_impl(true, &state).expect("pause");
        assert!(p);
        assert!(*state.is_paused.read());

        let status = get_system_status_impl(&state).await.expect("status");
        assert!(status.is_paused);

        // 2. Spatial arbitrage execution fails with PAUSED error while paused
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.01,
            Some("LONG_HL_SHORT_LGT".into()),
            Some(15.0),
            Some(0.0),
            Some(0),
            Some(3),
            Some(12.0),
            &state,
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("PAUSED"));

        // Verify open cycles in virtual ledger are untouched (0 open cycles created)
        assert_eq!(state.virtual_ledger.read().lots.len(), 0);

        // 3. Continue trading
        let c = set_engine_paused_impl(false, &state).expect("continue");
        assert!(!c);
        assert!(!*state.is_paused.read());

        let status_resumed = get_system_status_impl(&state).await.expect("status");
        assert!(!status_resumed.is_paused);

        // 4. Executions succeed after continue
        let res_resumed = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.01,
            Some("LONG_HL_SHORT_LGT".into()),
            Some(15.0),
            Some(0.0),
            Some(0),
            Some(3),
            Some(12.0),
            &state,
        )
        .await;
        assert!(res_resumed.is_ok());
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);

        // 5. Re-pausing preserves existing open position (does NOT clear it)
        let _ = set_engine_paused_impl(true, &state).expect("pause again");
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
    }

    #[tokio::test]
    async fn test_unconfigured_state_returns_zero_synthetic_data() {
        let state = AppEngineState::new();

        // 1. Zero venues active without user credentials
        assert_eq!(state.get_venues().len(), 0);

        // 2. Orderbooks return empty vector (fail-closed, no synthetic quotes fabricated)
        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("fetch should succeed");
        assert_eq!(books.len(), 0);

        // 3. Multi-market quotes return 0.0 with no fake numbers
        let quotes = fetch_multi_market_quotes_impl(vec!["BTC-USDC".into()], &state)
            .await
            .expect("quotes");
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].best_bid, 0.0);
        assert_eq!(quotes[0].best_ask, 0.0);
        assert_eq!(quotes[0].mid_price, 0.0);
        assert_eq!(quotes[0].best_buy_venue, "-");
        assert_eq!(quotes[0].best_sell_venue, "-");

        // 4. Spatial, spot, funding, and prediction scans return empty vector (fail-closed)
        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 0.1, &state)
            .await
            .expect("opps");
        assert_eq!(opps.len(), 0);

        let spot_opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("spot");
        assert_eq!(spot_opps.len(), 0);

        let funding_opps = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("funding");
        assert_eq!(funding_opps.len(), 0);

        let pred_opps = scan_prediction_opportunities_impl("", &state)
            .await
            .expect("pred");
        assert_eq!(pred_opps.len(), 0);

        // 5. Venue balances return empty
        let balances = get_venue_balances_impl(&state).await.expect("balances");
        assert_eq!(balances.len(), 0);
    }

    #[tokio::test]
    async fn test_orderbooks_fetch_and_spread() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64210.0), dec!(64215.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("fetch should succeed");

        assert!(books.len() >= 2);
        let hl = books
            .iter()
            .find(|b| b.venue_id == "hyperliquid")
            .expect("hyperliquid book");
        assert_eq!(hl.best_bid, 64200.0);
        assert_eq!(hl.best_ask, 64205.0);
        assert!(hl.spread >= 0.0);
        assert!(hl.vwap_ask >= hl.best_ask);
    }

    #[tokio::test]
    async fn test_spatial_opportunity_scan() {
        let state = AppEngineState::new();
        let opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("scan should succeed");
        // Zero synthetic data: unconfigured state returns empty
        assert_eq!(opps.len(), 0);
    }

    #[tokio::test]
    async fn test_funding_opportunities_multi_symbol() {
        let state = setup_test_state_with_venues();
        let r_hl = arbitrager_core::quote::FundingRate {
            venue: arbitrager_core::types::VenueId::new("hyperliquid"),
            symbol: "BTC-USDC".into(),
            rate: dec!(0.00015),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        let r_lgt = arbitrager_core::quote::FundingRate {
            venue: arbitrager_core::types::VenueId::new("lighter"),
            symbol: "BTC-USDC".into(),
            rate: dec!(0.00005),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        state.market_data_hub.update_funding(r_hl);
        state.market_data_hub.update_funding(r_lgt);

        let opps_btc = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("btc scan");
        assert!(!opps_btc.is_empty());
    }

    #[tokio::test]
    async fn test_prediction_fail_closed_unconfigured() {
        let state = AppEngineState::new();
        let opps = scan_prediction_opportunities_impl("FED", &state)
            .await
            .expect("pred scan");
        // Zero synthetic data: unconfigured state returns empty
        assert_eq!(opps.len(), 0);
    }

    #[tokio::test]
    async fn test_execute_spatial_arbitrage() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            Some("Divergence Entry".into()),
            Some(18.5),
            Some(12.0),
            Some(1),
            Some(3),
            Some(15.0),
            &state,
        )
        .await
        .expect("exec spatial");
        assert_eq!(res.trade_type, "Spatial Arb");
        assert_eq!(res.direction, "Divergence Entry");
        // Caller-supplied trigger spread is not trusted; the returned spread is
        // the calculated net BPS (~20.6) which must clear the 15 BPS hurdle
        assert!(res.spread_bps > 15.0);
        assert!(res.spread_bps < 30.0);
        // Opening a paper position realizes only fee outflow
        assert!(res.realized_profit_usd < 0.0);

        let logs = get_execution_logs_impl(&state).expect("logs");
        let last = logs.last().expect("last log");
        assert_eq!(last.direction.as_deref(), Some("Divergence Entry"));
        assert_eq!(last.trigger_spread_bps, Some(res.spread_bps));
        assert_eq!(last.baseline_mean_spread_bps, Some(12.0));
        assert_eq!(last.position_step, Some(1));
        assert_eq!(last.max_units, Some(3));
        assert_eq!(last.nominal_hurdle_bps, Some(15.0));
        assert!(last.net_profit < 0.0);

        // Simulated paper accounts must hold the opposite positions
        let acc_hl = state
            .get_paper_exchange("hyperliquid")
            .expect("hl paper")
            .fetch_account()
            .await
            .expect("hl account");
        let acc_lgt = state
            .get_paper_exchange("lighter")
            .expect("lgt paper")
            .fetch_account()
            .await
            .expect("lgt account");
        let pos_hl = acc_hl
            .positions
            .iter()
            .find(|p| p.symbol == "BTC-USDC")
            .expect("hl position");
        let pos_lgt = acc_lgt
            .positions
            .iter()
            .find(|p| p.symbol == "BTC-USDC")
            .expect("lgt position");
        assert_eq!(pos_hl.side, Side::Buy);
        assert_eq!(pos_lgt.side, Side::Sell);
    }

    #[tokio::test]
    async fn test_risk_overview_and_killswitch() {
        let state = AppEngineState::new();
        let risk = get_risk_overview_impl(&state).await.expect("risk overview");

        assert_eq!(risk.circuit_breaker_state, "ACTIVE");
        assert!(!risk.is_halted);
        // Three seeded paper accounts at 100k equity each
        assert_eq!(risk.total_portfolio_value_usd, 300000.0);

        // Flat book: no observed positions means no delta entries at all
        assert!(risk.net_deltas.is_empty());
        assert_eq!(risk.margin_health.status, "FLAT");
        assert_eq!(risk.margin_health.liquidation_distance_pct, 0.0);
        assert_eq!(risk.margin_health.maintenance_margin_req_usd, 0.0);

        // Trigger killswitch
        let halt_msg = trigger_emergency_killswitch_impl(&state).expect("killswitch");
        assert!(halt_msg.contains("EMERGENCY KILLSWITCH"));

        let risk_after = get_risk_overview_impl(&state)
            .await
            .expect("risk overview after halt");
        assert!(risk_after.is_halted);
        assert!(risk_after.circuit_breaker_state.contains("TRIPPED"));

        // Reset circuit breaker
        let reset_msg = reset_circuit_breaker_impl(&state).expect("reset");
        assert!(reset_msg.contains("reset successfully"));

        let risk_resumed = get_risk_overview_impl(&state)
            .await
            .expect("risk overview resumed");
        assert!(!risk_resumed.is_halted);
    }

    #[tokio::test]
    async fn test_engine_settings_command() {
        let state = AppEngineState::new();
        let cur = get_engine_settings_impl(&state).expect("get settings");
        assert_eq!(cur.max_slippage_bps, 15.0);

        let new_settings = EngineSettingsDto {
            hsm_url: "http://hsm.internal:9000".into(),
            max_slippage_bps: 20.0,
            min_depth_ratio: 4.0,
            hyperliquid_url: "https://api.hyperliquid.xyz".into(),
            lighter_url: "https://mainnet.zklighter.elliot.ai".into(),
            vanta_url: "https://api-evm.orderly.org".into(),
            robinhood_rpc_url: "https://rpc.robinhood.network".into(),
            robinhood_router: "0x1111".into(),
            polymarket_url: "https://clob.polymarket.com".into(),
            kalshi_url: "https://trading-api.kalshi.com".into(),
            ..EngineSettingsDto::default()
        };

        save_engine_settings_impl(new_settings, &state).expect("save settings");
        let updated = get_engine_settings_impl(&state).expect("get updated");
        assert_eq!(updated.max_slippage_bps, 20.0);
        assert_eq!(updated.min_depth_ratio, 4.0);
        assert_eq!(updated.hsm_url, "http://hsm.internal:9000");
        assert_eq!(updated.robinhood_router, "0x1111");
        assert!(updated.configured_venues.is_empty());
    }

    #[tokio::test]
    async fn test_credential_settings_rejected_while_vault_locked() {
        let (state, dir) = test_state_with_vault("locked");
        let before = get_engine_settings_impl(&state).expect("settings before");

        let mut settings = before.clone();
        settings.hyperliquid_private_key = "SENTINEL_HL_KEY".into();
        settings.hyperliquid_wallet = "SENTINEL_HL_WALLET".into();
        let res = save_engine_settings_impl(settings, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("locked"));

        let after = get_engine_settings_impl(&state).expect("settings after");
        assert_eq!(after.max_slippage_bps, before.max_slippage_bps);
        assert!(after.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_credential_save_is_vault_only_and_sanitized() {
        let (state, dir) = test_state_with_vault("creds");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_wallet: "SENTINEL_WALLET_0x".into(),
            hyperliquid_private_key: "SENTINEL_PRIV_9f2".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save credential settings");

        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("hyperliquid credentials in vault");
        assert_eq!(hl.api_secret, "SENTINEL_PRIV_9f2");
        assert_eq!(hl.wallet_address.as_deref(), Some("SENTINEL_WALLET_0x"));
        assert!(hl.enabled);

        let runtime = state.settings.read().clone();
        let returned = get_engine_settings_impl(&state).expect("sanitized settings");
        let returned_json = serde_json::to_string(&returned).expect("serialize");
        let runtime_json = serde_json::to_string(&runtime).expect("serialize runtime");
        for sentinel in ["SENTINEL_WALLET_0x", "SENTINEL_PRIV_9f2"] {
            assert!(
                runtime.hyperliquid_private_key.is_empty()
                    || !runtime.hyperliquid_private_key.contains(sentinel)
            );
            assert!(
                !runtime_json.contains(sentinel),
                "runtime settings JSON leaked {}",
                sentinel
            );
            assert!(
                !returned_json.contains(sentinel),
                "settings JSON leaked {}",
                sentinel
            );
        }
        assert_eq!(returned.configured_venues, vec!["hyperliquid".to_string()]);

        lock_vault_impl(&state);
        let locked = get_engine_settings_impl(&state).expect("locked settings");
        assert!(locked.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_delete_exchange_credentials_allowlist_and_lifecycle() {
        let (state, dir) = test_state_with_vault("delete_creds");

        let err = delete_exchange_credentials_impl("hyperliquid", &state).unwrap_err();
        assert!(err.contains("locked"), "err: {}", err);

        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_private_key: "SENTINEL_PRIV_del".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save credentials");

        for bad in ["unknown_venue", "hyperliquid;rm", "*", "hyperliquid wallet"] {
            assert!(
                delete_exchange_credentials_impl(bad, &state).is_err(),
                "deletion accepted non-allowlisted venue {:?}",
                bad
            );
        }

        assert!(!delete_exchange_credentials_impl("kalshi", &state).unwrap());
        assert!(delete_exchange_credentials_impl(" HyperLiquid ", &state).unwrap());

        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert!(stored.credentials.is_empty());
        let sanitized = get_engine_settings_impl(&state).expect("sanitized");
        assert!(sanitized.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_crud_and_credential_binding() {
        let (state, dir) = test_state_with_vault("accounts");
        initialize_vault_impl("test password", &state).expect("init vault");

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_ACCT1".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        assert!(a1.is_default, "first enabled account becomes default");
        assert_eq!(a1.credentials_configured, Some(true));
        assert!(a1.has_api_secret);
        assert!(!a1.has_api_key);

        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_ACCT2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");
        assert!(!a2.is_default);
        assert_ne!(a1.id, a2.id);

        // Regression: two accounts on one venue must not overwrite each
        // other's secrets (the old venue-keyed upsert clobbered them).
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 2);
        let secrets: Vec<&str> = stored
            .credentials
            .iter()
            .map(|c| c.api_secret.as_str())
            .collect();
        assert!(secrets.contains(&"SENTINEL_ACCT1"));
        assert!(secrets.contains(&"SENTINEL_ACCT2"));
        assert!(stored.credentials.iter().all(|c| c.profile_id.is_some()));

        // Sanitized DTO must never carry secret values over IPC.
        let dto_json = serde_json::to_string(&a1).expect("dto json");
        assert!(!dto_json.contains("SENTINEL_ACCT1"));

        // Venue identity is immutable on update.
        assert!(upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: Some(a2.id.clone()),
                venue: "lighter".into(),
                label: "Moved".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .is_err());

        let updated = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: Some(a2.id.clone()),
                venue: "hyperliquid".into(),
                label: "Alt Renamed".into(),
                enabled: true,
                make_default: true,
                credentials: None,
            },
            &state,
        )
        .expect("update a2");
        assert!(updated.is_default);
        assert_eq!(updated.label, "Alt Renamed");

        set_default_account_impl("hyperliquid", &a1.id, &state).expect("set default");
        let listed = list_accounts_impl(Some("hyperliquid"), &state).expect("list");
        assert!(listed.iter().find(|a| a.id == a1.id).unwrap().is_default);
        assert!(!listed.iter().find(|a| a.id == a2.id).unwrap().is_default);

        // Deleting an account removes its bound secret record as well.
        assert!(delete_account_impl(&a2.id, &state).expect("delete a2"));
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_ACCT1");
        assert_eq!(
            stored.credentials[0].profile_id.as_deref(),
            Some(a1.id.as_str())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_delete_account_credentials_keeps_profile_and_siblings() {
        let (state, dir) = test_state_with_vault("cred-del");

        // Locked vault and unknown accounts both fail closed.
        assert!(delete_account_credentials_impl("lighter-default", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");
        assert!(delete_account_credentials_impl("no-such-account", &state).is_err());

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL1".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");

        // Removing a1's credentials leaves the account profile and a2's
        // secrets untouched.
        assert!(delete_account_credentials_impl(&a1.id, &state).expect("delete creds"));
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_DEL2");
        assert!(state
            .state_store
            .get_account(&a1.id)
            .expect("account lookup")
            .is_some());

        let listed = list_accounts_impl(Some("hyperliquid"), &state).expect("list");
        assert!(!listed
            .iter()
            .find(|a| a.id == a1.id)
            .unwrap()
            .credentials_configured
            .unwrap_or(true));
        assert!(listed
            .iter()
            .find(|a| a.id == a2.id)
            .unwrap()
            .credentials_configured
            .unwrap_or(false));

        // Second delete reports no remaining records.
        assert!(!delete_account_credentials_impl(&a1.id, &state).unwrap());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_get_account_credentials_scoped_to_profile_and_unlocked() {
        let (state, dir) = test_state_with_vault("cred-reveal");

        // Locked vault and unknown accounts both fail closed.
        assert!(get_account_credentials_impl("lighter-default", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");
        assert!(get_account_credentials_impl("no-such-account", &state).is_err());

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_REVEAL1".into(),
                    private_key: Some("0xSENTINEL_PK1".into()),
                    account_index: Some(7),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_REVEAL2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");

        // Reveal returns only the requested profile's material.
        let revealed = get_account_credentials_impl(&a1.id, &state).expect("reveal a1");
        assert_eq!(revealed.api_secret, "SENTINEL_REVEAL1");
        assert_eq!(revealed.private_key.as_deref(), Some("0xSENTINEL_PK1"));
        assert_eq!(revealed.account_index, Some(7));

        // An accountless profile read errors rather than leaking siblings.
        let a3 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Empty".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create a3");
        assert!(get_account_credentials_impl(&a3.id, &state).is_err());
        let revealed2 = get_account_credentials_impl(&a2.id, &state).expect("reveal a2");
        assert_eq!(revealed2.api_secret, "SENTINEL_REVEAL2");
        assert!(revealed2.private_key.as_deref().unwrap_or("").is_empty());

        // Re-locking the vault blocks reveal again.
        lock_vault_impl(&state);
        assert!(get_account_credentials_impl(&a1.id, &state).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_legacy_credentials_migrate_to_default_profile() {
        let (state, dir) = test_state_with_vault("migrate");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Simulate a pre-account-model record: venue-keyed, unbound.
        state
            .vault
            .write()
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: "SENTINEL_LEGACY".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xlegacy".into()),
                account_index: Some(2),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: true,
            }])
            .expect("seed legacy cred");
        // The HSM token is not an exchange account and stays unbound.
        state
            .vault
            .write()
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "remote_hsm".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: "SENTINEL_HSM".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: true,
            }])
            .expect("seed hsm");

        lock_vault_impl(&state);
        unlock_vault_impl("test password", &state).expect("unlock migrates");

        let accounts = list_accounts_impl(Some("hyperliquid"), &state).expect("accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "hyperliquid-default");
        assert_eq!(accounts[0].label, "Default");
        assert!(accounts[0].is_default);
        assert!(accounts[0].enabled);
        assert_eq!(accounts[0].credentials_configured, Some(true));
        assert!(accounts[0].has_wallet_address);
        assert_eq!(accounts[0].account_index, Some(2));

        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("hl record");
        assert_eq!(hl.profile_id.as_deref(), Some("hyperliquid-default"));
        assert_eq!(hl.api_secret, "SENTINEL_LEGACY");
        let hsm = stored
            .credentials
            .iter()
            .find(|c| c.venue == "remote_hsm")
            .expect("hsm record");
        assert!(hsm.profile_id.is_none(), "remote_hsm must stay unbound");

        // A flat settings save now merges into the bound default profile
        // instead of creating a second record for the same venue.
        let s = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_private_key: "SENTINEL_NEW".into(),
            ..Default::default()
        };
        save_engine_settings_impl(s, &state).expect("save");
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(
            stored
                .credentials
                .iter()
                .filter(|c| c.venue == "hyperliquid")
                .count(),
            1
        );
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .unwrap();
        assert_eq!(hl.api_secret, "SENTINEL_NEW");
        assert_eq!(hl.wallet_address.as_deref(), Some("0xlegacy"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_strategy_route_crud_and_eligibility() {
        let (state, dir) = test_state_with_vault("routes");
        initialize_vault_impl("test password", &state).expect("init vault");

        let hl = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "HL Main".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("hl account");
        let lt = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "lighter".into(),
                label: "LT Main".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("lt account");
        let lt_disabled = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "lighter".into(),
                label: "LT Off".into(),
                enabled: false,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("lt disabled account");

        let route = upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Main Route".into(),
                account_a_id: lt.id.clone(),
                account_b_id: hl.id.clone(),
                enabled: true,
            },
            &state,
        )
        .expect("create route");
        // Canonical ordering places hyperliquid first regardless of input order.
        assert_eq!(route.account_a_id, hl.id);
        assert_eq!(route.account_a_venue, "hyperliquid");
        assert_eq!(route.account_b_id, lt.id);
        assert_eq!(route.account_b_venue, "lighter");

        let eligible = eligible_strategy_routes_impl("futures", "lighter", "hyperliquid", &state)
            .expect("eligible");
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].id, route.id);

        // Disabled endpoint accounts cannot form an enabled route.
        assert!(upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Bad".into(),
                account_a_id: hl.id.clone(),
                account_b_id: lt_disabled.id.clone(),
                enabled: true,
            },
            &state,
        )
        .is_err());

        // Duplicate account pair within a strategy is rejected (either order).
        assert!(upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Dup".into(),
                account_a_id: hl.id.clone(),
                account_b_id: lt.id.clone(),
                enabled: true,
            },
            &state,
        )
        .is_err());

        // Referenced accounts cannot be deleted; the error names the routes.
        let err = delete_account_impl(&hl.id, &state).unwrap_err();
        assert!(err.contains("strategy route"), "err: {err}");

        assert!(delete_strategy_route_impl(&route.id, &state).expect("delete route"));
        assert!(delete_account_impl(&hl.id, &state).expect("delete after route removal"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_delete_requires_unlock_and_listing_is_lock_safe() {
        let (state, dir) = test_state_with_vault("del_locked");
        initialize_vault_impl("test password", &state).expect("init vault");

        let a = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create");

        lock_vault_impl(&state);
        let err = delete_account_impl(&a.id, &state).unwrap_err();
        assert!(err.to_lowercase().contains("locked"), "err: {err}");

        // Account metadata remains listable while locked; credential
        // presence is reported as unknown rather than fabricated.
        let listed = list_accounts_impl(None, &state).expect("list locked");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].credentials_configured, None);

        // The secret survives the refused deletion.
        unlock_vault_impl("test password", &state).expect("unlock");
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_DEL");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_no_credential_read_ipc_registered() {
        let main_src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
            .expect("read main.rs");
        for cmd in [
            "get_exchange_credentials",
            "get_credentials",
            "read_credentials",
            "export_credentials",
            "export_vault",
            "get_vault_data",
        ] {
            assert!(
                !main_src.contains(cmd),
                "secret-read IPC registered in main.rs: {}",
                cmd
            );
        }
    }

    #[tokio::test]
    async fn test_nonsecret_settings_save_while_vault_locked() {
        let (state, dir) = test_state_with_vault("nonsecret");
        let settings = EngineSettingsDto {
            max_slippage_bps: 25.0,
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save nonsecret settings");
        let updated = get_engine_settings_impl(&state).expect("get settings");
        assert_eq!(updated.max_slippage_bps, 25.0);
        assert!(updated.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_multi_dex_ticker_orderbooks() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "AVAX-USDC", dec!(28.5), dec!(28.6));
        seed_test_book(&state, "lighter", "AVAX-USDC", dec!(28.55), dec!(28.65));
        seed_test_funding(&state, "hyperliquid", "AVAX-USDC");
        seed_test_funding(&state, "lighter", "AVAX-USDC");

        let books_avax = fetch_orderbooks_impl("AVAX-USDC", &state)
            .await
            .expect("avax books");
        assert!(books_avax.len() >= 2);
        for b in &books_avax {
            assert!(b.best_bid > 0.0);
            assert!(b.best_ask >= b.best_bid);
            assert!(b.total_bid_depth > 0.0);
        }
    }

    #[tokio::test]
    async fn test_multi_dex_ticker_spatial_and_funding() {
        let state = setup_test_state_with_venues();
        let r_hl = arbitrager_core::quote::FundingRate {
            venue: arbitrager_core::types::VenueId::new("hyperliquid"),
            symbol: "AVAX-USDC".into(),
            rate: dec!(0.00020),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        let r_lgt = arbitrager_core::quote::FundingRate {
            venue: arbitrager_core::types::VenueId::new("lighter"),
            symbol: "AVAX-USDC".into(),
            rate: dec!(0.00008),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        state.market_data_hub.update_funding(r_hl);
        state.market_data_hub.update_funding(r_lgt);

        let opps_avax = scan_funding_opportunities_impl("AVAX-USDC", 10000.0, &state)
            .await
            .expect("avax funding");
        assert!(!opps_avax.is_empty());
    }

    #[tokio::test]
    async fn test_initial_execution_logs_presence() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let logs = get_execution_logs_impl(&state).expect("execution logs");
        assert!(logs.is_empty());

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "Hyperliquid",
            "Lighter",
            0.1,
            Some("Divergence Entry".into()),
            Some(18.5),
            Some(12.0),
            Some(1),
            Some(3),
            Some(15.0),
            &state,
        )
        .await
        .expect("spatial execution");

        assert!(res.opportunity_found);
        let logs_after = get_execution_logs_impl(&state).expect("execution logs after");
        assert_eq!(logs_after.len(), 1);
        assert_eq!(logs_after[0].symbol, "BTC-USDC");
    }

    #[tokio::test]
    async fn test_paper_execution_against_live_depth() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "DOT-USDC", dec!(4.40), dec!(4.42));
        seed_test_book(&state, "lighter", "DOT-USDC", dec!(4.45), dec!(4.47));

        let exec_res = execute_spatial_arbitrage_impl(
            "DOT-USDC",
            "Hyperliquid",
            "Lighter",
            0.5,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("spatial exec on dot");
        assert!(exec_res.execution_success);
        assert!(exec_res.realized_profit_usd < 0.0);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_unsafe_inputs() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        // Same venue, zero qty, NaN qty
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "Hyperliquid",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("distinct"));
        // Same canonical venue through different aliases must also reject
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "hl",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("distinct"));
        // Malformed symbol
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC'; DROP",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("Invalid market symbol"));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("positive"));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            f64::NAN,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("finite"));
        // Negative hurdle
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(-1.0),
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("hurdle"));

        // Nothing may have mutated state
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_bad_books() {
        let state = setup_test_state_with_venues();

        // Crossed book on the buy side (bid > ask)
        let mut crossed = arbitrager_core::quote::OrderBook::new(
            arbitrager_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        crossed.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(64300.0),
            dec!(10.0),
        ));
        crossed.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64200.0),
            dec!(10.0),
        ));
        crossed.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(crossed);
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = res.expect_err("crossed book must be rejected");
        assert!(
            err.contains("order book rejected"),
            "unexpected err: {}",
            err
        );
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_depth_slippage_and_economics() {
        let state = setup_test_state_with_venues();

        // Depth ratio: 11 units of within-limit ask depth fills the order but
        // cannot satisfy 11 * 3 = 33 required ratio; out-of-limit depth at
        // 64500 must not count toward the requirement
        {
            let mut book = arbitrager_core::quote::OrderBook::new(
                arbitrager_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrager_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(11.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64500.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);

            let mut deep = arbitrager_core::quote::OrderBook::new(
                arbitrager_core::types::VenueId::new("lighter"),
                "BTC-USDC".to_string(),
            );
            deep.bids.push(arbitrager_core::quote::BookLevel::new(
                dec!(64360.0),
                dec!(50.0),
            ));
            deep.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64365.0),
                dec!(50.0),
            ));
            deep.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(deep);
        }

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            11.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(
            res.expect_err("depth ratio must reject").contains("depth"),
            "expected depth error"
        );

        // Slippage: qty 5 walks deep into a second level 300+ bps above top
        {
            let mut book = arbitrager_core::quote::OrderBook::new(
                arbitrager_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrager_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(1.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(66200.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);
        }
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            5.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(
            res.expect_err("slippage must reject").contains("slippage"),
            "expected slippage error"
        );

        // Non-profitable spread: sell VWAP below buy VWAP
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64330.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64200.0), dec!(64210.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res
            .expect_err("negative spread must reject")
            .contains("not profitable"));

        // Profitable but below the caller's nominal hurdle
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64260.0), dec!(64265.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(50.0),
            &state,
        )
        .await;
        assert!(res.expect_err("hurdle must reject").contains("hurdle"));

        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    #[tokio::test]
    async fn test_spatial_execution_equal_qty_wide_spread_passes_delta() {
        let state = setup_test_state_with_venues();
        // Equal 3 BTC legs at materially different marks are delta-neutral
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(61000.0), dec!(61005.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            3.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let res = res.expect("wide-spread equal-quantity pair must pass all guards");
        assert!(res.execution_success);
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_critical_margin() {
        let state = setup_test_state_with_venues();
        let hl_paper = state.get_paper_exchange("hyperliquid").expect("hl paper");

        // Drive the paper account into critical margin utilization directly:
        // open 1.5 BTC at 5x (IM 18000), then drop leverage to 1x so the
        // position's initial margin becomes 90000 against 100k equity (90%)
        let mut big_book = arbitrager_core::quote::OrderBook::new(
            arbitrager_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        big_book.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(59900.0),
            dec!(500.0),
        ));
        big_book.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(60000.0),
            dec!(500.0),
        ));
        big_book.timestamp = Utc::now();
        hl_paper.replace_orderbook(big_book).expect("seed big book");
        let resp = hl_paper
            .place_order(arbitrager_core::order::OrderRequest::new_ioc(
                arbitrager_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC",
                Side::Buy,
                dec!(60000.0),
                dec!(1.5),
            ))
            .await
            .expect("direct paper order");
        assert_eq!(resp.status, arbitrager_core::types::OrderStatus::Filled);
        hl_paper
            .set_leverage("BTC-USDC", 1)
            .await
            .expect("lower leverage");
        let acc = hl_paper.fetch_account().await.expect("account");
        assert!(
            acc.margin_usage_pct >= dec!(85.0),
            "usage {} must be critical",
            acc.margin_usage_pct
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = res.expect_err("critical margin must reject");
        assert!(err.contains("margin"), "unexpected err: {}", err);
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_toggle_off_keeps_stored_venue_discoverable_and_deletable() {
        let (state, dir) = test_state_with_vault("toggle");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_wallet: "SENTINEL_WALLET_toggle".into(),
            hyperliquid_private_key: "SENTINEL_PRIV_toggle".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save creds");
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .configured_venues,
            vec!["hyperliquid".to_string()]
        );

        // Toggle the venue off: disable the account profile created by the
        // first save plus the engine flag — both halves of the unified
        // account/venue switch. The stored record stays discoverable because
        // configured_venues means "credentials exist", not "venue active".
        state
            .state_store
            .upsert_account(
                "hyperliquid-default",
                "hyperliquid",
                "Default",
                false,
                false,
                "standard",
            )
            .expect("disable account");
        let off = EngineSettingsDto {
            hyperliquid_enabled: false,
            ..Default::default()
        };
        save_engine_settings_impl(off, &state).expect("save toggle off");
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .configured_venues,
            vec!["hyperliquid".to_string()]
        );
        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("secret must remain stored");
        assert_eq!(hl.api_secret, "SENTINEL_PRIV_toggle");
        assert!(!hl.enabled);

        // Disabled records remain deletable
        assert!(
            delete_exchange_credentials_impl("hyperliquid", &state).expect("delete disabled creds")
        );
        assert!(get_engine_settings_impl(&state)
            .expect("settings")
            .configured_venues
            .is_empty());
        assert!(state
            .vault
            .write()
            .get_data()
            .expect("vault data")
            .credentials
            .is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_cross_ticker_arbitrage_execution() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "SOL-USDC", dec!(152.0), dec!(152.2));
        seed_test_book(&state, "lighter", "AVAX-USDC", dec!(28.5), dec!(28.7));

        let pnl_before = *state.total_realized_profit.read();
        let res = execute_cross_ticker_arbitrage_impl(
            "SOL-USDC",
            "AVAX-USDC",
            "Hyperliquid",
            "Lighter",
            10.0,
            Some(18.5),
            &state,
        )
        .await;

        assert!(res.is_err());
        assert!(res.unwrap_err().contains("cross-ticker"));
        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_live_mode_enable_rejected() {
        let state = AppEngineState::new();
        let res = set_live_mode_impl(true, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Live execution is disabled"));
        assert!(!*state.is_live_mode.read());

        let res_off = set_live_mode_impl(false, &state).expect("disable live mode");
        assert!(!res_off);
        assert!(!*state.is_live_mode.read());
    }

    #[tokio::test]
    async fn test_real_trading_mode_rejected() {
        let state = AppEngineState::new();
        let settings = EngineSettingsDto {
            trading_mode: "real".into(),
            ..EngineSettingsDto::default()
        };
        let res = save_engine_settings_impl(settings, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("paper"));
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .trading_mode,
            "paper"
        );
    }

    #[tokio::test]
    async fn test_paper_starting_balance_resets_paper_accounts_on_change() {
        let state = AppEngineState::new();

        // Invalid values fail closed.
        for bad in [0.0, -500.0, f64::NAN, f64::INFINITY, 2e12] {
            let res = save_engine_settings_impl(
                EngineSettingsDto {
                    paper_starting_balance_usd: bad,
                    ..EngineSettingsDto::default()
                },
                &state,
            );
            assert!(res.is_err(), "bad balance {bad} must be rejected");
        }

        // A new starting balance reinitializes every paper account.
        save_engine_settings_impl(
            EngineSettingsDto {
                paper_starting_balance_usd: 25000.0,
                ..EngineSettingsDto::default()
            },
            &state,
        )
        .expect("save");
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert_eq!(acc.equity, dec!(25000.0));
            assert_eq!(acc.free_margin, dec!(25000.0));
            assert!(acc.positions.is_empty());
        }

        // Re-saving an unchanged balance is a no-op for paper accounts.
        save_engine_settings_impl(
            EngineSettingsDto {
                paper_starting_balance_usd: 25000.0,
                max_slippage_bps: 42.0,
                ..EngineSettingsDto::default()
            },
            &state,
        )
        .expect("resave");
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert_eq!(acc.equity, dec!(25000.0));
        }
        assert_eq!(state.settings.read().max_slippage_bps, 42.0);
    }

    #[tokio::test]
    async fn test_unsupported_native_executions_fail_closed() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60020.0), dec!(60025.0));

        let pnl_before = *state.total_realized_profit.read();

        let spot = execute_spot_arbitrage_impl(
            SpotExecutionParams {
                symbol: "BTC-USDC".into(),
                buy_venue: "hyperliquid".into(),
                sell_venue: "lighter".into(),
                trade_size_usd: 5000.0,
                max_slippage_bps: 15.0,
                net_spread_bps: Some(12.0),
            },
            &state,
        )
        .await;
        assert!(spot.is_err());
        assert!(spot.unwrap_err().contains("spot"));

        let funding =
            execute_funding_carry_impl("BTC-USDC", "hyperliquid", "lighter", 25000.0, &state).await;
        assert!(funding.is_err());
        assert!(funding.unwrap_err().contains("funding"));

        let pred = execute_prediction_arbitrage_impl(
            "FED DECISION",
            "polymarket",
            "kalshi",
            100.0,
            &state,
        )
        .await;
        assert!(pred.is_err());
        assert!(pred.unwrap_err().contains("prediction"));

        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_multi_market_quotes() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3450.0),
            dec!(3452.0),
        );
        seed_test_book(&state, "hyperliquid", "SOL-USDC", dec!(152.0), dec!(152.2));

        let symbols = vec![
            "BTC-USDC".to_string(),
            "ETH-USDC".to_string(),
            "SOL-USDC".to_string(),
        ];
        let quotes = fetch_multi_market_quotes_impl(symbols, &state)
            .await
            .expect("quotes");
        assert_eq!(quotes.len(), 3);
        for q in &quotes {
            assert!(q.best_bid > 0.0);
            assert!(q.best_ask > 0.0);
            assert!(q.spread_bps >= 0.0);
            assert!(q.mid_price > 0.0);
            assert_ne!(q.best_buy_venue, "-");
            assert_ne!(q.best_sell_venue, "-");
        }
    }

    #[tokio::test]
    async fn test_ticker_strategy_config_crud_and_validation() {
        let state = AppEngineState::new();

        // 1. Initial query for non-existing returns default with correct symbol
        let def = get_ticker_strategy_config_impl("BTC-USDC", &state).expect("default config");
        assert_eq!(def.symbol, "BTC-USDC");
        assert_eq!(def.target_profit_bps, 5.0);
        assert_eq!(def.window_percentile, 50);
        assert_eq!(def.execution_mode, "SINGLE_WAY");
        assert_eq!(def.user_target_leverage, 5);

        // 2. Validation: window_percentile must be >= 50
        let invalid_percentile = TickerStrategyConfigDto {
            window_percentile: 40,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(invalid_percentile, &state).is_err());

        // Leverage above the hard cap and nonpositive unit sizes are rejected
        let bad_leverage = TickerStrategyConfigDto {
            user_target_leverage: 6,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(bad_leverage, &state).is_err());
        let zero_leverage = TickerStrategyConfigDto {
            user_target_leverage: 0,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(zero_leverage, &state).is_err());
        let bad_unit = TickerStrategyConfigDto {
            unit_size: 0.0,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(bad_unit, &state).is_err());
        let nan_unit = TickerStrategyConfigDto {
            unit_size: f64::NAN,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(nan_unit, &state).is_err());

        // 3. Save valid updated config
        let updated = TickerStrategyConfigDto {
            symbol: "BTC-USDC".into(),
            target_profit_bps: 8.5,
            window_percentile: 80,
            max_accumulated_units: 5,
            current_accumulated_units: 1,
            execution_mode: "DOUBLE_WAY".into(),
            unit_size: 0.01,
            user_target_leverage: 5,
            sliding_window: "5m".into(),
        };
        assert!(save_ticker_strategy_config_impl(updated.clone(), &state).is_ok());

        // 4. Retrieve saved config
        let retrieved =
            get_ticker_strategy_config_impl("BTC-USDC", &state).expect("retrieved config");
        assert_eq!(retrieved.target_profit_bps, 8.5);
        assert_eq!(retrieved.window_percentile, 80);
        assert_eq!(retrieved.max_accumulated_units, 5);
        assert_eq!(retrieved.execution_mode, "DOUBLE_WAY");
        assert_eq!(retrieved.unit_size, 0.01);

        // 5. Query all
        let all = get_all_ticker_strategy_configs_impl(&state).expect("all configs");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].symbol, "BTC-USDC");
    }

    #[tokio::test]
    async fn test_venue_balances_report_paper_account() {
        let state = setup_test_state_with_venues();
        let balances = get_venue_balances_impl(&state).await.expect("balances");
        assert_eq!(balances.len(), 3);
        for b in &balances {
            assert!(b.available_margin_usd > 0.0);
            assert!(b.equity_usd > 0.0);
            assert_eq!(b.status, "HEALTHY");
        }
    }

    #[tokio::test]
    async fn test_strategy_performance_metrics() {
        let state = AppEngineState::new();
        let perfs = get_strategy_performance_impl(None, &state).expect("performance");
        assert_eq!(perfs.len(), 4);

        let spatial = perfs
            .iter()
            .find(|p| p.strategy_id == "spatial_spread")
            .expect("spatial metrics");
        assert_eq!(spatial.total_cycles_completed, 0);
        assert_eq!(spatial.total_pnl_usd, 0.0);
    }

    #[tokio::test]
    async fn test_wfo_calibration_command() {
        let state = AppEngineState::new();
        let configs_before = state.strategy_configs.read().len();
        let logs_before = state.execution_logs.read().len();

        let res = run_wfo_calibration_impl("BTC-USDC", &state).await;
        let err = res.expect_err("WFO must fail closed without recorded history");
        assert!(err.contains("unavailable"), "unexpected err: {}", err);

        assert_eq!(state.strategy_configs.read().len(), configs_before);
        assert_eq!(state.execution_logs.read().len(), logs_before);
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_scans_fail_closed_even_when_fully_configured() {
        let (state, dir) = test_state_with_vault("scans");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            lighter_enabled: true,
            vanta_enabled: true,
            robinhood_enabled: true,
            arcus_enabled: true,
            popdex_enabled: true,
            kraken_enabled: true,
            kraken_spot_enabled: true,
            kraken_futures_enabled: true,
            polymarket_enabled: true,
            kalshi_enabled: true,
            hyperliquid_wallet: "WALLET_x".into(),
            hyperliquid_private_key: "PRIV_x".into(),
            lighter_api_key: "LGT_KEY".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save configured settings");

        let spot = scan_spot_opportunities_impl("BTC", &state)
            .await
            .expect("spot scan");
        assert!(spot.is_empty());
        let pred = scan_prediction_opportunities_impl("FED", &state)
            .await
            .expect("pred scan");
        assert!(pred.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_repeated_open() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let first = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("first execution");
        assert!(first.execution_success);

        let logs_before = state.execution_logs.read().len();
        let pnl_before = *state.total_realized_profit.read();
        let lots_before = state.virtual_ledger.read().lots.len();

        let second = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = second.expect_err("second open must reject");
        assert!(err.contains("already exists"), "unexpected err: {}", err);

        assert_eq!(state.execution_logs.read().len(), logs_before);
        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert_eq!(state.virtual_ledger.read().lots.len(), lots_before);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_insufficient_ioc_bounded_depth() {
        let state = setup_test_state_with_venues();
        // Unbounded slippage for 2.0 would pass (VWAP ~8 bps), but only 1.0
        // ask sits inside the 15 BPS IOC limit; the remainder is unreachable.
        {
            let mut book = arbitrager_core::quote::OrderBook::new(
                arbitrager_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrager_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(1.0),
            ));
            book.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64310.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);

            let mut deep = arbitrager_core::quote::OrderBook::new(
                arbitrager_core::types::VenueId::new("lighter"),
                "BTC-USDC".to_string(),
            );
            deep.bids.push(arbitrager_core::quote::BookLevel::new(
                dec!(64360.0),
                dec!(50.0),
            ));
            deep.asks.push(arbitrager_core::quote::BookLevel::new(
                dec!(64365.0),
                dec!(50.0),
            ));
            deep.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(deep);
        }

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            2.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res
            .expect_err("must reject before dispatch")
            .contains("slippage"));
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    #[tokio::test]
    async fn test_symbol_and_numeric_input_validation() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );

        let oversized_symbol = "A".repeat(65);
        for bad in [
            "",
            "  ",
            "BTC -USDC",
            "../etc/passwd",
            "BTC-USDC?x=1",
            "BTC-USDC;rm",
            "BTC-\u{0001}USDC",
            "-USDC",
            "BTC-",
            oversized_symbol.as_str(),
        ] {
            assert!(
                fetch_orderbooks_impl(bad, &state).await.is_err(),
                "symbol {:?} must be rejected",
                bad
            );
        }

        let oversized: Vec<String> = (0..101).map(|i| format!("S{}", i)).collect();
        assert!(fetch_multi_market_quotes_impl(oversized, &state)
            .await
            .is_err());
        assert!(
            fetch_multi_market_quotes_impl(vec!["BTC-USDC?bad".into()], &state)
                .await
                .is_err()
        );

        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", 5.0, f64::NAN, &state)
                .await
                .is_err()
        );
        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", 5.0, -1.0, &state)
                .await
                .is_err()
        );
        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", f64::NAN, 0.1, &state)
                .await
                .is_err()
        );
        assert!(scan_funding_opportunities_impl("BTC-USDC", -5.0, &state)
            .await
            .is_err());
        assert!(
            scan_funding_opportunities_impl("BTC-USDC", f64::INFINITY, &state)
                .await
                .is_err()
        );

        assert!(run_stat_arb_step_impl(0, f64::NAN, 100.0, &state).is_err());
        assert!(run_stat_arb_step_impl(0, 100.0, -1.0, &state).is_err());
        assert!(fit_ou_process_impl(&[100.0; 12], f64::NAN).is_err());
        assert!(fit_ou_process_impl(&[f64::INFINITY; 12], 1.0).is_err());
        assert!(compute_optimal_thresholds_impl(f64::NAN, 0.0, 1.0, 0.1).is_err());
        assert!(compute_optimal_thresholds_impl(1.0, 0.0, f64::INFINITY, 0.1).is_err());
        assert!(compute_optimal_thresholds_impl(1.0, 0.0, 1.0, f64::NAN).is_err());

        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_engine_settings_rejects_invalid_inputs() {
        let state = AppEngineState::new();
        let base = EngineSettingsDto::default();

        let bad = EngineSettingsDto {
            max_slippage_bps: f64::NAN,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            max_slippage_bps: 0.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            min_depth_ratio: f64::NAN,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            trading_mode: "simulation".into(),
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            bind_address: "0.0.0.0".into(),
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            engine_rpc_port: 9000,
            engine_ws_port: 9000,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            engine_rpc_port: 0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());

        let ok = save_engine_settings_impl(base, &state);
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn test_risk_overview_open_position_free_margin_and_unknown_liquidation() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let before = get_risk_overview_impl(&state).await.expect("risk before");
        assert_eq!(before.total_portfolio_value_usd, 300000.0);
        assert_eq!(before.margin_health.margin_balance_usd, 300000.0);
        assert_eq!(before.margin_health.status, "FLAT");

        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");

        let after = get_risk_overview_impl(&state).await.expect("risk after");
        assert!(
            after.margin_health.margin_balance_usd < before.margin_health.margin_balance_usd,
            "free margin must decrease after open: {}",
            after.margin_health.margin_balance_usd
        );
        assert_eq!(
            after.margin_health.status, "UNKNOWN_LIQUIDATION",
            "open positions without liquidation prices must not report SAFE"
        );
        let btc = after
            .net_deltas
            .iter()
            .find(|d| d.asset == "BTC")
            .expect("BTC delta entry");
        assert_eq!(btc.net_quantity, 0.0);
        assert_eq!(btc.net_exposure_usd, 0.0);
        assert!(btc.is_neutral);
    }

    #[tokio::test]
    async fn test_spatial_pnl_synced_from_paper_equity() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");
        assert!(res.execution_success);

        let mut equity_sum = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_sum += paper.fetch_account().await.expect("account").equity;
        }
        assert_eq!(
            *state.total_realized_profit.read(),
            equity_sum - dec!(300000)
        );
        // Fees are real outflow; the position open is not marked as profit
        assert!(*state.total_realized_profit.read() < dec!(0));

        let metrics = state
            .virtual_ledger
            .read()
            .compute_strategy_metrics("spatial_spread");
        assert_eq!(metrics.total_cycles_initiated, 1);

        let perfs = get_strategy_performance_impl(None, &state).expect("performance");
        let spatial = perfs
            .iter()
            .find(|p| p.strategy_id == "spatial_spread")
            .expect("spatial_spread metrics");
        assert_eq!(spatial.total_cycles_completed, 0);
    }

    #[tokio::test]
    async fn test_spatial_execution_queued_call_reevaluates_breaker() {
        let state = Arc::new(setup_test_state_with_venues());
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let guard = state.execution_lock.lock().await;
        let st = state.clone();
        let task = tokio::spawn(async move {
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &st,
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        state
            .circuit_breaker
            .trip("killswitch while execution queued");
        drop(guard);

        let res = task.await.expect("join");
        let err = res.expect_err("queued execution must observe the tripped breaker");
        assert!(err.contains("Circuit Breaker"), "unexpected err: {}", err);
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert!(acc.positions.is_empty());
            assert_eq!(acc.equity, dec!(100000.0));
        }
    }

    #[tokio::test]
    async fn test_spatial_concurrent_same_symbol_only_one_opens() {
        let state = Arc::new(setup_test_state_with_venues());
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let (r1, r2) = tokio::join!(
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &state,
            ),
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &state,
            ),
        );
        let oks = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert_eq!(oks, 1, "exactly one execution may open: {:?} {:?}", r1, r2);
        let errs: Vec<String> = [r1, r2].into_iter().filter_map(|r| r.err()).collect();
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].contains("already exists"),
            "unexpected err: {}",
            errs[0]
        );
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
        assert_eq!(state.execution_logs.read().len(), 1);
    }

    #[tokio::test]
    async fn test_spatial_execution_persists_durable_signal() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");
        assert!(res.execution_success);

        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert_eq!(signals.len(), 1);
        let sig = &signals[0];
        assert_eq!(sig.status, crate::state_store::SignalStatus::Balanced);
        assert_eq!(sig.legs.len(), 2);
        for leg in &sig.legs {
            assert!(
                (leg.filled_qty - 0.1).abs() < 1e-9,
                "leg must carry its own fill attribution: {:?}",
                leg
            );
            assert!(leg.fill_price.unwrap_or(0.0) > 0.0);
        }
        let venues: std::collections::HashSet<&str> =
            sig.legs.iter().map(|l| l.venue.as_str()).collect();
        assert!(venues.contains("hyperliquid") && venues.contains("lighter"));
        // Self-hedged signal: per-signal delta and ticker net are both zero.
        assert!(state.state_store.signal_delta(&sig.id).unwrap().abs() < 1e-9);
        assert!(
            state
                .state_store
                .ticker_net_position("BTC-USDC")
                .unwrap()
                .abs()
                < 1e-9
        );
        // Balanced signals no longer block the ticker.
        assert!(!state.state_store.ticker_blocked("BTC-USDC").unwrap());
    }

    #[tokio::test]
    async fn test_spatial_execution_blocked_by_unreconciled_signal() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        // A prior signal for BTC-USDC holds residual exposure: only its long
        // leg filled. The ticker must refuse new orders until it reconciles.
        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-stuck".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: Some(5.0),
                note: None,
                legs: vec![
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: crate::state_store::LegSide::Long,
                        intended_qty: 0.1,
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "lighter".into(),
                        side: crate::state_store::LegSide::Short,
                        intended_qty: 0.1,
                    },
                ],
            })
            .expect("record signal");
        state
            .state_store
            .apply_leg_fill(
                "sig-stuck",
                crate::state_store::LegSide::Long,
                0.1,
                64205.0,
                0.05,
                Some("o-stuck"),
            )
            .expect("leg fill");
        state
            .state_store
            .update_signal_status(
                "sig-stuck",
                crate::state_store::SignalStatus::Imbalanced,
                Some("short leg rejected"),
            )
            .expect("status");

        let err = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect_err("blocked ticker must reject");
        assert!(err.contains("blocked"), "unexpected err: {}", err);

        // The gate is per-ticker: a different symbol still executes.
        let other = execute_spatial_arbitrage_impl(
            "ETH-USDC",
            "hyperliquid",
            "lighter",
            0.5,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("different ticker executes");
        assert!(other.execution_success);
    }

    #[tokio::test]
    async fn test_spatial_candidates_batch_picks_highest_expected() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        let outcomes = execute_spatial_candidates_impl(
            vec![
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.1,
                    expected_net_usd: Some(8.0),
                    route_id: None,
                    direction: None,
                },
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.2,
                    expected_net_usd: Some(15.0),
                    route_id: None,
                    direction: None,
                },
                SpatialCandidateInput {
                    symbol: "ETH-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.5,
                    expected_net_usd: Some(20.0),
                    route_id: None,
                    direction: None,
                },
            ],
            &state,
        )
        .await
        .expect("batch outcomes");
        assert_eq!(outcomes.len(), 3);

        let btc: Vec<_> = outcomes.iter().filter(|o| o.symbol == "BTC-USDC").collect();
        assert_eq!(btc.len(), 2);
        let winner = btc.iter().find(|o| o.selected).expect("one BTC winner");
        assert_eq!(winner.expected_net_usd, Some(15.0));
        assert!(winner.result.as_ref().unwrap().execution_success);
        let loser = btc.iter().find(|o| !o.selected).expect("one BTC loser");
        assert_eq!(loser.expected_net_usd, Some(8.0));
        assert!(loser.reason.as_deref().unwrap().contains("lower expected"));

        let eth = outcomes.iter().find(|o| o.symbol == "ETH-USDC").unwrap();
        assert!(eth.selected);
        assert!(eth.result.as_ref().unwrap().execution_success);

        // Exactly one durable signal per dispatched ticker.
        assert_eq!(
            state
                .state_store
                .list_signals(Some("BTC-USDC"), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state
                .state_store
                .list_signals(Some("ETH-USDC"), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_spatial_candidates_reject_all_when_ticker_blocked() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        // In-flight signal on BTC-USDC blocks every candidate in the batch.
        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-flight".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![crate::state_store::NewSignalLeg {
                    account_id: None,
                    venue: "hyperliquid".into(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: 0.1,
                }],
            })
            .expect("record in-flight signal");

        let outcomes = execute_spatial_candidates_impl(
            vec![
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.1,
                    expected_net_usd: Some(99.0),
                    route_id: None,
                    direction: None,
                },
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.2,
                    expected_net_usd: Some(50.0),
                    route_id: None,
                    direction: None,
                },
                SpatialCandidateInput {
                    symbol: "ETH-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.5,
                    expected_net_usd: Some(20.0),
                    route_id: None,
                    direction: None,
                },
            ],
            &state,
        )
        .await
        .expect("batch outcomes");

        for o in outcomes.iter().filter(|o| o.symbol == "BTC-USDC") {
            assert!(!o.selected, "blocked ticker rejects every candidate");
            assert!(o.reason.as_deref().unwrap().contains("blocked"));
            assert!(o.result.is_none());
        }
        let eth = outcomes.iter().find(|o| o.symbol == "ETH-USDC").unwrap();
        assert!(eth.selected);
        assert!(eth.result.as_ref().unwrap().execution_success);
        // Only the in-flight seed exists for BTC — nothing new was dispatched.
        assert_eq!(
            state
                .state_store
                .list_signals(Some("BTC-USDC"), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_delete_account_blocked_by_signal_history() {
        let state = setup_test_state_with_venues();
        let account = state
            .state_store
            .upsert_account("hl-main", "hyperliquid", "Main", true, false, "standard")
            .expect("account");

        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-hist".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![crate::state_store::NewSignalLeg {
                    account_id: Some(account.id.clone()),
                    venue: "hyperliquid".into(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: 0.1,
                }],
            })
            .expect("record signal");

        let err = delete_account_impl(&account.id, &state).expect_err("must refuse");
        assert!(err.contains("position history"), "unexpected err: {}", err);
        assert!(state
            .state_store
            .get_account(&account.id)
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_validate_adapter_book_helper() {
        let venue = arbitrager_core::types::VenueId::new("hyperliquid");
        let mut book = OrderBook::new(venue.clone(), "BTC-USDC".to_string());
        book.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(64000.0),
            dec!(1.0),
        ));
        book.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64001.0),
            dec!(1.0),
        ));

        assert!(validate_adapter_book(&book, &venue, "BTC-USDC").is_ok());

        let other_venue = arbitrager_core::types::VenueId::new("lighter");
        let err = validate_adapter_book(&book, &other_venue, "BTC-USDC").unwrap_err();
        assert!(err.contains("venue"), "unexpected err: {}", err);

        let err = validate_adapter_book(&book, &venue, "ETH-USDC").unwrap_err();
        assert!(err.contains("symbol"), "unexpected err: {}", err);

        let mut crossed = book.clone();
        crossed.bids[0].price = dec!(65000.0);
        let err = validate_adapter_book(&crossed, &venue, "BTC-USDC").unwrap_err();
        assert!(err.contains("crossed"), "unexpected err: {}", err);
    }

    #[tokio::test]
    async fn test_read_only_paths_skip_malformed_cached_book() {
        let state = setup_test_state_with_venues();
        let mut crossed = OrderBook::new(
            arbitrager_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        crossed.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(65000.0),
            dec!(10.0),
        ));
        crossed.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64000.0),
            dec!(10.0),
        ));
        crossed.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(crossed);
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "robinhood",
            "BTC-USDC",
            dec!(64360.0),
            dec!(64365.0),
        );
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].venue_id, "lighter");
        assert_eq!(books[1].venue_id, "robinhood");

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 0.1, &state)
            .await
            .expect("scan");
        // The malformed hyperliquid book must not participate; lighter and
        // robinhood share identical seeded prices so any pair is non-executable.
        assert!(opps.iter().all(|o| !o.is_executable));
        assert!(opps
            .iter()
            .all(|o| o.buy_venue != "hyperliquid" && o.sell_venue != "hyperliquid"));
    }

    #[tokio::test]
    async fn test_decimal_conversion_overflow_rejects() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(f64::MAX),
            &state,
        )
        .await;
        let err = res.expect_err("f64::MAX hurdle must reject");
        assert!(err.contains("decimal range"), "unexpected err: {}", err);

        let res = scan_spatial_opportunities_impl("BTC-USDC", 5.0, f64::MAX, &state).await;
        let err = res.expect_err("f64::MAX order_qty must reject");
        assert!(err.contains("decimal range"), "unexpected err: {}", err);

        let res = scan_funding_opportunities_impl("BTC-USDC", f64::MAX, &state).await;
        let err = res.expect_err("f64::MAX trade size must reject");
        assert!(err.contains("decimal range"), "unexpected err: {}", err);

        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_scan_is_executable_respects_bounded_depth() {
        let state = setup_test_state_with_venues();
        let mut thin = OrderBook::new(
            arbitrager_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        thin.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(64100.0),
            dec!(50.0),
        ));
        thin.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(1.0),
        ));
        thin.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64310.0),
            dec!(50.0),
        ));
        thin.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(thin);

        let mut deep = OrderBook::new(
            arbitrager_core::types::VenueId::new("lighter"),
            "BTC-USDC".to_string(),
        );
        deep.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(64360.0),
            dec!(50.0),
        ));
        deep.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64365.0),
            dec!(50.0),
        ));
        deep.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(deep);

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 2.0, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue.contains("Hyperliquid"))
            .expect("hl->lgt opportunity row");
        assert!(
            !opp.is_executable,
            "out-of-limit depth must mark non-executable"
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3400.0),
            dec!(3401.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3410.0), dec!(3411.0));
        let opps = scan_spatial_opportunities_impl("ETH-USDC", 5.0, 0.1, &state)
            .await
            .expect("scan eth");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue.contains("Hyperliquid"))
            .expect("hl->lgt eth row");
        assert!(opp.is_executable, "deep profitable pair must be executable");
    }

    #[tokio::test]
    async fn test_risk_overview_drawdown_pct_uses_day_start() {
        let state = setup_test_state_with_venues();
        state.circuit_breaker.update_equity(dec!(400000.0));
        state.circuit_breaker.reset();
        assert_eq!(state.circuit_breaker.day_start_equity(), dec!(400000.0));

        let risk = get_risk_overview_impl(&state).await.expect("risk");
        assert_eq!(risk.current_drawdown_usd, 100000.0);
        assert_eq!(risk.drawdown_pct, 25.0);

        state.circuit_breaker.update_equity(dec!(0.0));
        state.circuit_breaker.reset();
        let risk = get_risk_overview_impl(&state)
            .await
            .expect("risk zero start");
        assert_eq!(risk.drawdown_pct, 0.0);
    }

    fn seed_samples(state: &AppEngineState, symbol: &str, ts_ms: i64, mids: &[(&str, f64)]) {
        let entries: Vec<crate::state_store::VenueSampleEntry> = mids
            .iter()
            .map(|(vid, mid)| crate::state_store::VenueSampleEntry {
                venue_id: vid.to_string(),
                mid: *mid,
                best_bid: mid - 0.5,
                best_ask: mid + 0.5,
            })
            .collect();
        state
            .state_store
            .record_spread_samples(symbol, ts_ms, &entries)
            .expect("record samples");
    }

    #[tokio::test]
    async fn test_signal_series_empty_is_honest() {
        // No venues enabled -> no adapter traffic; empty store must return a
        // structured empty series, not an error or synthetic points.
        let state = AppEngineState::new();
        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 0);
        assert_eq!(s.earliest_ts_ms, None);
        assert!(s.ts_ms.is_empty());
        assert!(s.dev_bps.is_empty());
        assert_eq!(s.mode, "consensus");
    }

    #[tokio::test]
    async fn test_signal_series_roundtrip_computes_in_rust() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        // Three buckets, two venues: A=100.0, B=101.0 -> equal-weight
        // consensus baseline 100.5; devs -49.75/+49.75 bps; band ~99.5 bps.
        for i in 0..3 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (2 - i) * 1_000,
                &[("hyperliquid", 100.0), ("lighter", 101.0)],
            );
        }

        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 6);
        assert_eq!(s.venues, vec!["hyperliquid", "lighter"]);
        assert_eq!(s.ts_ms.len(), 3);
        assert!(s.earliest_ts_ms.unwrap() <= now - 2_000);
        for row in &s.dev_bps {
            let hl = row[0].unwrap();
            let lgt = row[1].unwrap();
            assert!((hl + lgt).abs() < 1e-6, "consensus deviations sum to ~0");
            assert!(hl < 0.0 && lgt > 0.0);
        }
        let band = s.band_max_bps[2] - s.band_min_bps[2];
        assert!(
            (band - 99.5025).abs() < 0.01,
            "band height = gross spread bps"
        );
    }

    #[tokio::test]
    async fn test_signal_series_leader_mode_and_gap_preservation() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        seed_samples(
            &state,
            "ETH-USDC",
            now - 2_000,
            &[("hyperliquid", 3000.0), ("lighter", 3006.0)],
        );
        // Second bucket: only hyperliquid -> lighter cell must be null.
        seed_samples(&state, "ETH-USDC", now - 1_000, &[("hyperliquid", 3001.0)]);

        let s = get_signal_series_impl("ETH-USDC", "leader", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.mode, "leader");
        assert_eq!(s.ts_ms.len(), 2);
        // No volumes in test state -> deterministic leader = alphabetically
        // first venue ("hyperliquid"), pinned at 0.
        assert_eq!(s.dev_bps[0][0], Some(0.0));
        assert_eq!(s.dev_bps[1][1], None, "missing venue stays a gap");
        assert_eq!(
            s.band_max_bps[1], s.band_min_bps[1],
            "single-venue band collapses"
        );
    }

    #[tokio::test]
    async fn test_signal_series_rejects_bad_symbol() {
        let state = AppEngineState::new();
        assert!(get_signal_series_impl("  ", "consensus", 60, &state)
            .await
            .is_err());
        assert!(get_signal_series_impl("BTC\u{0}", "consensus", 60, &state)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_fetch_orderbooks_persists_spread_samples() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        seed_test_book(
            &state,
            "robinhood",
            "BTC-USDC",
            dec!(60120.0),
            dec!(60125.0),
        );
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 3);

        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert_eq!(rows.len(), 3, "one row per reporting venue per poll");
        let hl = rows.iter().find(|r| r.venue_id == "hyperliquid").unwrap();
        assert_eq!(hl.mid, 60002.5);
        assert_eq!(hl.best_bid, 60000.0);
        assert_eq!(hl.best_ask, 60005.0);
    }

    #[test]
    fn test_tauri_commands_resolve_managed_arc_state() {
        let app = tauri::test::mock_builder()
            .manage(Arc::new(AppEngineState::new()))
            .invoke_handler(tauri::generate_handler![
                list_signal_executions,
                get_ticker_position_state,
                execute_spatial_candidates
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview builds");

        for (cmd, body) in [
            (
                "list_signal_executions",
                serde_json::json!({"symbol": null, "status": null}),
            ),
            (
                "get_ticker_position_state",
                serde_json::json!({"symbol": "BTC-USDC"}),
            ),
            (
                "execute_spatial_candidates",
                serde_json::json!({"candidates": []}),
            ),
        ] {
            let response = tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "tauri://localhost".parse().expect("invoke url"),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            );
            assert!(
                response.is_ok(),
                "{cmd} must resolve the managed Arc<AppEngineState>: {:?}",
                response.err()
            );
        }
    }

    fn seed_book_depth(
        state: &AppEngineState,
        venue_id: &str,
        symbol: &str,
        bid: Decimal,
        bid_qty: Decimal,
        ask: Decimal,
        ask_qty: Decimal,
    ) {
        let mut book = arbitrager_core::quote::OrderBook::new(
            arbitrager_core::types::VenueId::new(venue_id),
            symbol.to_string(),
        );
        book.bids
            .push(arbitrager_core::quote::BookLevel::new(bid, bid_qty));
        book.asks
            .push(arbitrager_core::quote::BookLevel::new(ask, ask_qty));
        book.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(book);
    }

    #[tokio::test]
    async fn test_emergency_flatten_retries_partial_close() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("spatial execution");
        assert!(res.execution_success);

        seed_book_depth(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(0.05),
            dec!(64205.0),
            dec!(10.0),
        );
        seed_book_depth(
            &state,
            "lighter",
            "BTC-USDC",
            dec!(64360.0),
            dec!(10.0),
            dec!(64365.0),
            dec!(0.05),
        );

        let first = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("first flatten");
        assert_eq!(
            first.residual_signals, 1,
            "partial close must leave exactly one residual root signal"
        );

        let acc_hl = state.ex_hl.fetch_account().await.expect("hl account");
        let acc_lgt = state.ex_lighter.fetch_account().await.expect("lgt account");
        let hl_qty = acc_hl
            .get_position("BTC-USDC")
            .map(|p| p.quantity)
            .expect("hl residual");
        let lgt_qty = acc_lgt
            .get_position("BTC-USDC")
            .map(|p| p.quantity)
            .expect("lgt residual");
        assert!(
            (hl_qty - dec!(0.05)).abs() < dec!(0.0001),
            "hl residual {hl_qty}"
        );
        assert!(
            (lgt_qty - dec!(0.05)).abs() < dec!(0.0001),
            "lgt residual {lgt_qty}"
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let second = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("second flatten");
        assert_eq!(
            second.residual_signals, 0,
            "second flatten notes: {:?}",
            second.notes
        );

        let acc_hl = state.ex_hl.fetch_account().await.expect("hl account");
        let acc_lgt = state.ex_lighter.fetch_account().await.expect("lgt account");
        assert!(
            acc_hl.positions.is_empty(),
            "no residual or reverse position may remain: {:?}",
            acc_hl.positions
        );
        assert!(
            acc_lgt.positions.is_empty(),
            "no residual or reverse position may remain: {:?}",
            acc_lgt.positions
        );

        assert!(!state
            .state_store
            .ticker_blocked("BTC-USDC")
            .expect("blocked"));
        for sig in state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals")
        {
            assert_eq!(
                sig.status,
                crate::state_store::SignalStatus::Closed,
                "record {} must be closed",
                sig.id
            );
        }

        let ledger = state.virtual_ledger.read();
        let open_lots: Vec<_> = ledger
            .lots
            .values()
            .filter(|l| {
                l.strategy_id == "spatial_spread"
                    && l.symbol == "BTC-USDC"
                    && l.status == arbitrager_execution::virtual_ledger::CycleStatus::Open
            })
            .collect();
        assert!(open_lots.is_empty(), "spatial lot must not stay Open");
        let lot = ledger
            .lots
            .values()
            .find(|l| l.strategy_id == "spatial_spread" && l.symbol == "BTC-USDC")
            .expect("spatial lot exists");
        assert_eq!(
            lot.status,
            arbitrager_execution::virtual_ledger::CycleStatus::StoppedOut
        );
        assert!(lot.realized_pnl_usd.is_some(), "realized PnL populated");
    }

    #[tokio::test]
    async fn test_flatten_futures_root_preserves_other_strategy_position() {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );

        let mut paper_book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        paper_book.bids.push(arbitrager_core::quote::BookLevel::new(
            dec!(64200.0),
            dec!(10.0),
        ));
        paper_book.asks.push(arbitrager_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(10.0),
        ));
        state
            .ex_hl
            .replace_orderbook(paper_book)
            .expect("seed paper book");

        let open = arbitrager_core::order::OrderRequest::new_market(
            state.ex_hl.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(0.2),
        );
        let resp = state.ex_hl.place_order(open).await.expect("open position");
        assert_eq!(resp.status, arbitrager_core::types::OrderStatus::Filled);

        for (id, strategy) in [("root-fut", "futures"), ("root-spot", "spot")] {
            state
                .state_store
                .record_signal_execution(&NewSignalExecution {
                    id: id.into(),
                    strategy_id: strategy.into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: LegSide::Long,
                        intended_qty: 0.1,
                    }],
                })
                .expect("record root");
            state
                .state_store
                .apply_leg_fill(id, LegSide::Long, 0.1, 64205.0, 0.0, None)
                .expect("root fill");
        }

        let res = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("flatten futures");

        let acc = state.ex_hl.fetch_account().await.expect("hl account");
        let pos = acc
            .get_position("BTC-USDC")
            .expect("spot strategy's residual must remain");
        assert_eq!(pos.side, Side::Buy, "no reverse position may be opened");
        assert!(
            (pos.quantity - dec!(0.1)).abs() < dec!(0.0001),
            "only the futures-attributed 0.1 may be closed, got residual {}",
            pos.quantity
        );

        let fut = state
            .state_store
            .get_signal("root-fut")
            .expect("fut")
            .expect("fut exists");
        assert_eq!(fut.status, SignalStatus::Closed);
        let spot = state
            .state_store
            .get_signal("root-spot")
            .expect("spot")
            .expect("spot exists");
        assert!(
            spot.status.counts_as_open(),
            "spot root must stay open, got {:?}",
            spot.status
        );
        for sig in state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals")
        {
            if sig.id.starts_with("flat-root-fut-") {
                assert_eq!(sig.status, SignalStatus::Closed, "child {}", sig.id);
            }
        }
        assert_eq!(res.residual_signals, 0);
        assert_eq!(res.signals_closed, 1);
    }
}
