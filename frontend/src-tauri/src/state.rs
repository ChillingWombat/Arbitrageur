use crate::vault::ExchangeCredentials;
use arbitrageur_core::hub::MarketDataHub;
use arbitrageur_core::types::{dec, Decimal};
use arbitrageur_exchanges::arcus::ArcusAdapter;
use arbitrageur_exchanges::aster::AsterAdapter;
use arbitrageur_exchanges::binance::BinanceAdapter;
use arbitrageur_exchanges::coinbase::CoinbaseAdapter;
use arbitrageur_exchanges::decibel::DecibelAdapter;
use arbitrageur_exchanges::hyperliquid::HyperliquidAdapter;
use arbitrageur_exchanges::kraken::KrakenAdapter;
use arbitrageur_exchanges::lighter::LighterAdapter;
use arbitrageur_exchanges::okx::OkxAdapter;
use arbitrageur_exchanges::prediction::{KalshiAdapter, PolymarketAdapter};
use arbitrageur_exchanges::simulated::SimulatedExchange;
use arbitrageur_exchanges::vanta::VantaAdapter;
use arbitrageur_execution::coordinator::AtomicArbitrageCoordinator;
use arbitrageur_execution::passive_unwind::PassiveUnwindConfig;
use arbitrageur_execution::smart_routing::SmartOrderRouter;
use arbitrageur_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use arbitrageur_risk::delta_guard::DeltaGuard;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ExecutionLogEntry {
    pub id: String,
    pub timestamp: String,
    pub trade_type: String, // "Spatial" or "Funding" or "Prediction"
    /// Owning strategy for this fill event (`futures`, `radar`, `triangle`,
    /// ...). Rows that predate attribution carry `"unattributed"`: they are
    /// excluded from per-strategy sums and counted separately, never
    /// absorbed. `serde(default)` keeps old serialized rows loadable.
    #[serde(default = "default_unattributed_strategy")]
    pub strategy_id: String,
    pub symbol: String,
    pub venue_a: String,
    pub venue_b: String,
    pub leg_a_status: String,
    pub leg_b_status: String,
    pub success: bool,
    pub gross_profit: f64,
    pub fees: f64,
    pub net_profit: f64,
    pub latency_us: u64,
    pub note: String,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub trigger_spread_bps: Option<f64>,
    #[serde(default)]
    pub baseline_mean_spread_bps: Option<f64>,
    #[serde(default)]
    pub position_step: Option<u32>,
    #[serde(default)]
    pub max_units: Option<u32>,
    #[serde(default)]
    pub nominal_hurdle_bps: Option<f64>,
    #[serde(default)]
    pub is_live: bool,
}

fn default_unattributed_strategy() -> String {
    crate::attribution::UNATTRIBUTED_STRATEGY_ID.to_string()
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TickerStrategyConfigDto {
    pub symbol: String,
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    pub max_accumulated_units: u32,
    pub current_accumulated_units: u32,
    pub execution_mode: String, // "SINGLE_WAY" or "DOUBLE_WAY"
    pub unit_size: f64,
    pub user_target_leverage: u32,
    #[serde(default = "default_sliding_window")]
    pub sliding_window: String,
}

fn default_sliding_window() -> String {
    "24h".into()
}

fn default_sliding_window_mode() -> String {
    "manual".into()
}

fn default_sliding_window_secs() -> u64 {
    86_400
}

fn default_step_size_mult() -> f64 {
    10.0
}

fn default_max_step_usd() -> f64 {
    2500.0
}

fn default_max_ticker_equity_pct() -> f64 {
    20.0
}

/// Universal, strategy-scoped parameters — one record per strategy_id
/// ("futures" | "funding" | "spot"), shared across all tickers. Per-ticker
/// contract specifics (unit size in base units, accumulated grid units) stay in
/// TickerStrategyConfigDto keyed by symbol.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StrategyParamsDto {
    pub strategy_id: String,
    // Single Spread (futures)
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    pub sliding_window: String,
    #[serde(default = "default_sliding_window_mode")]
    pub sliding_window_mode: String,
    #[serde(default = "default_sliding_window_secs")]
    pub sliding_window_secs: u64,
    /// Single Spread sizing redesign: one grid step = min(mult ×
    /// venue-compatible min lot, maxStepUsd / price), quantized UP to the
    /// venue step. Max steps derive per ticker from total paper equity × pct.
    /// `max_accumulated_units` is therefore DERIVED, not authored.
    /// `serde(default)` on each field keeps old persisted rows (which carry
    /// the removed `unit_size_usd`/`max_accumulated_units` keys) loadable —
    /// unknown keys are ignored, missing new keys fall back to defaults.
    #[serde(default = "default_step_size_mult")]
    pub step_size_mult: f64,
    #[serde(default = "default_max_step_usd")]
    pub max_step_usd: f64,
    #[serde(default = "default_max_ticker_equity_pct")]
    pub max_ticker_equity_pct: f64,
    pub execution_mode: String,
    pub user_target_leverage: u32,
    // Per-strategy auto-hide filters (paper/research visibility + bot/scan
    // targeting). None (or <= 0) disables the filter. Aggregation is the MIN
    // across the ticker's in-scope venues (the >= 2-venue intersection that
    // lists it); any in-scope venue missing the metric excludes the ticker
    // (fail-closed). MIN bounds executable size and defeats single-venue
    // wash inflation — a mean would let one inflated venue whitelist meme
    // coins. Price uses the minimum mid across in-scope venues.
    #[serde(default)]
    pub auto_min_volume_usd: Option<f64>,
    #[serde(default)]
    pub auto_min_price_usd: Option<f64>,
    // Prediction-market event-volume threshold. Defined for schema parity
    // but inert: prediction adapters are scaffolds with no venue volume
    // data, so no code path reads this field yet.
    #[serde(default)]
    pub auto_min_event_volume_usd: Option<f64>,
    // Funding Rate
    pub funding_trade_size_usd: f64,
    pub funding_min_apr_pct: f64,
    pub funding_holding_days: u32,
    // Per-strategy max slippage (bps). Each strategy enforces its own gate
    // through the shared slippage standard
    // (verify_pre_execution_slippage / route-economics); there is no global
    // setting. Prediction carries the field for schema parity but it is
    // inert: prediction adapters are scaffolds with no execution path, like
    // auto_min_event_volume_usd.
    #[serde(default = "default_max_slippage")]
    pub futures_max_slippage_bps: f64,
    #[serde(default = "default_max_slippage")]
    pub funding_max_slippage_bps: f64,
    #[serde(default = "default_max_slippage")]
    pub prediction_max_slippage_bps: f64,
    // Funding configurator: max allowed gap (bps) between the venues'
    // execution prices at entry; wider gaps refuse.
    #[serde(default = "default_funding_max_entry_spread")]
    pub funding_max_entry_spread_bps: f64,
    // Funding auto-exit: exit when the live inter-venue spread (percent)
    // falls below this value. None disables the gate.
    #[serde(default)]
    pub funding_auto_exit_spread_pct: Option<f64>,
    // Funding liquidation-distance trim: when EITHER leg comes within this
    // percent of liquidation, trim BOTH legs by the same qty to stay
    // neutral (close both when the remainder would drop below one step).
    // None disables the block. This is the funding instance of the
    // universal trim block (same semantics, shared helper).
    #[serde(default)]
    pub funding_liq_trim_pct: Option<f64>,
    // Funding auto-rebalance: reduce the overweight leg back to neutral.
    #[serde(default)]
    pub funding_auto_rebalance: bool,
    // Funding opportunity rotation: exit the current position for a
    // higher-yield one only when the net-of-friction improvement exceeds
    // this (bps). None disables rotation.
    #[serde(default)]
    pub funding_rotation_threshold_bps: Option<f64>,
    // Same-exchange spot-vs-perp basis (cash-and-carry) switch. When true the
    // funding scanner also emits long-spot / short-perp basis opps per venue
    // with a live funding print and both books available. `serde(default)`
    // keeps old persisted rows loadable (missing key => enabled).
    #[serde(default = "default_funding_enable_basis")]
    pub funding_enable_basis: bool,
    // Minimum signed basis `(perp - spot) / spot * 10000` in bps for basis
    // opps. None disables the floor (funding APR still gates downstream).
    #[serde(default)]
    pub funding_min_basis_bps: Option<f64>,
    // Universal trim block: every bot configurator surfaces the same
    // threshold with the same trim-both/close-both semantics via the shared
    // helper. Prediction is defined-but-disabled (no execution path).
    #[serde(default)]
    pub futures_liq_trim_pct: Option<f64>,
    #[serde(default)]
    pub spot_liq_trim_pct: Option<f64>,
    #[serde(default)]
    pub prediction_liq_trim_pct: Option<f64>,
    // Spatial (spot)
    pub spot_trade_size_usd: f64,
    pub spot_max_slippage_bps: f64,
    pub spot_min_net_spread_bps: f64,
    // Triangular Arbitrage
    #[serde(default = "default_triangle_trade_size")]
    pub triangle_trade_size_usd: f64,
    #[serde(default = "default_triangle_min_profit")]
    pub triangle_min_profit_bps: f64,
    #[serde(default = "default_max_slippage")]
    pub triangle_max_slippage_bps: f64,
    // Passive limit unwinds/rebalances: one control set per strategy with a
    // live reduce path (futures/spread, funding, spot). Disabled by default
    // (opt-in). Prediction carries no instance: prediction adapters are
    // scaffolds with no execution path, so passive unwind is inert there.
    // `serde(default)` keeps rows persisted before this block loadable.
    #[serde(default)]
    pub futures_passive_unwind: PassiveUnwindConfig,
    #[serde(default)]
    pub funding_passive_unwind: PassiveUnwindConfig,
    #[serde(default)]
    pub spot_passive_unwind: PassiveUnwindConfig,
}

impl StrategyParamsDto {
    pub fn defaults_for(strategy_id: &str) -> Self {
        Self {
            strategy_id: strategy_id.to_string(),
            target_profit_bps: 1.0,
            window_percentile: 50,
            sliding_window: "24h".into(),
            sliding_window_mode: "manual".into(),
            sliding_window_secs: 86_400,
            step_size_mult: 10.0,
            execution_mode: "SINGLE_WAY".into(),
            max_step_usd: 2500.0,
            max_ticker_equity_pct: 20.0,
            user_target_leverage: 5,
            auto_min_volume_usd: None,
            auto_min_price_usd: None,
            auto_min_event_volume_usd: None,
            funding_trade_size_usd: 25000.0,
            funding_min_apr_pct: 8.0,
            funding_holding_days: 14,
            futures_max_slippage_bps: default_max_slippage(),
            funding_max_slippage_bps: default_max_slippage(),
            prediction_max_slippage_bps: default_max_slippage(),
            funding_max_entry_spread_bps: default_funding_max_entry_spread(),
            funding_auto_exit_spread_pct: None,
            funding_liq_trim_pct: None,
            funding_auto_rebalance: false,
            funding_rotation_threshold_bps: None,
            funding_enable_basis: true,
            funding_min_basis_bps: None,
            futures_liq_trim_pct: None,
            spot_liq_trim_pct: None,
            prediction_liq_trim_pct: None,
            spot_trade_size_usd: 1000.0,
            spot_max_slippage_bps: 5.0,
            spot_min_net_spread_bps: 15.0,
            triangle_trade_size_usd: default_triangle_trade_size(),
            triangle_min_profit_bps: default_triangle_min_profit(),
            triangle_max_slippage_bps: default_max_slippage(),
            futures_passive_unwind: PassiveUnwindConfig::default(),
            funding_passive_unwind: PassiveUnwindConfig::default(),
            spot_passive_unwind: PassiveUnwindConfig::default(),
        }
    }
}

impl Default for TickerStrategyConfigDto {
    fn default() -> Self {
        Self {
            symbol: "BTC-USDC".into(),
            target_profit_bps: 1.0,
            window_percentile: 50,
            max_accumulated_units: 3,
            current_accumulated_units: 0,
            execution_mode: "SINGLE_WAY".into(),
            unit_size: 0.005,
            user_target_leverage: 5,
            sliding_window: "24h".into(),
        }
    }
}

fn default_hsm_url() -> String {
    "http://127.0.0.1:8787/sign".into()
}
fn default_hmac_token() -> String {
    String::new()
}
fn default_max_slippage() -> f64 {
    15.0
}
fn default_funding_enable_basis() -> bool {
    true
}
fn default_funding_max_entry_spread() -> f64 {
    50.0
}
fn default_min_depth_ratio() -> f64 {
    3.0
}
fn default_triangle_trade_size() -> f64 {
    10000.0
}
fn default_triangle_min_profit() -> f64 {
    2.0
}
fn default_hyperliquid_url() -> String {
    "https://api.hyperliquid.xyz".into()
}
fn default_lighter_account_index() -> String {
    "0".into()
}
fn default_lighter_url() -> String {
    "https://mainnet.zklighter.elliot.ai".into()
}
fn default_vanta_url() -> String {
    "https://api.orderly.org".into()
}
fn default_robinhood_rpc_url() -> String {
    "https://rpc.robinhood.network".into()
}
fn default_robinhood_router() -> String {
    "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba".into()
}
fn default_polymarket_url() -> String {
    "https://clob.polymarket.com".into()
}
fn default_kalshi_url() -> String {
    "https://trading-api.kalshi.com".into()
}
fn default_engine_rpc_port() -> u16 {
    8787
}
fn default_engine_ws_port() -> u16 {
    8789
}
fn default_bind_address() -> String {
    "127.0.0.1".into()
}
fn default_trading_mode() -> String {
    "paper".into()
}
fn default_paper_starting_balance() -> f64 {
    100000.0
}
fn default_fast_warmup_enabled() -> bool {
    true
}
fn default_referral_lighter() -> String {
    "https://app.lighter.xyz/?referral=SOLO&source=none".into()
}
fn default_referral_robinhood() -> String {
    "https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none".into()
}
fn default_sponsor_link() -> String {
    "https://buymeacoffee.com/dav1d1010".into()
}
fn default_app_version() -> String {
    "v1.2.0".into()
}
fn default_git_repo() -> String {
    "https://github.com/Dav1D1010/Arbitrageur".into()
}
fn default_true() -> bool {
    true
}
fn default_kraken_spot_url() -> String {
    "https://api.kraken.com".into()
}
fn default_kraken_futures_url() -> String {
    "https://futures.kraken.com".into()
}
fn default_arcus_url() -> String {
    "https://api.arcus.exchange".into()
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct VenueBalanceInfo {
    pub venue: String,
    #[serde(default)]
    pub equity_usd: f64,
    #[serde(default)]
    pub available_margin_usd: f64,
    #[serde(default)]
    pub margin_rate_pct: f64,
    pub status: String, // "HEALTHY", "LOW_MARGIN", "DEPLETED"
    pub recommended_action: Option<String>,
    #[serde(default)]
    pub is_live: bool,
    #[serde(default)]
    pub account_id: Option<String>,
}

// Intentionally no Debug: this DTO transiently holds credential material
// submitted by the UI and must never be printable.
#[derive(Clone, Serialize, Deserialize)]
pub struct EngineSettingsDto {
    #[serde(default = "default_hsm_url")]
    pub hsm_url: String,
    #[serde(default = "default_hmac_token")]
    pub hmac_token: String,
    #[serde(default = "default_min_depth_ratio")]
    pub min_depth_ratio: f64,

    // Networking & Ports
    #[serde(default = "default_engine_rpc_port")]
    pub engine_rpc_port: u16,
    #[serde(default = "default_engine_ws_port")]
    pub engine_ws_port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,

    // Execution & Risk Modes
    #[serde(default = "default_trading_mode")]
    pub trading_mode: String, // "paper" or "real"
    #[serde(default = "default_paper_starting_balance")]
    pub paper_starting_balance_usd: f64,
    #[serde(default = "default_fast_warmup_enabled")]
    pub fast_warmup_enabled: bool,

    // Exchange Enablement Toggles
    #[serde(default = "default_true")]
    pub hyperliquid_enabled: bool,
    #[serde(default = "default_true")]
    pub lighter_enabled: bool,
    #[serde(default = "default_true")]
    pub vanta_enabled: bool,
    #[serde(default = "default_true")]
    pub robinhood_enabled: bool,
    #[serde(default = "default_true")]
    pub arcus_enabled: bool,
    #[serde(default)]
    pub kraken_enabled: bool,
    #[serde(default)]
    pub kraken_spot_enabled: bool,
    #[serde(default)]
    pub kraken_futures_enabled: bool,
    #[serde(default)]
    pub polymarket_enabled: bool,
    #[serde(default)]
    pub kalshi_enabled: bool,
    #[serde(default)]
    pub decibel_enabled: bool,
    #[serde(default)]
    pub ibkr_enabled: bool,
    #[serde(default)]
    pub ig_enabled: bool,
    #[serde(default)]
    pub binance_enabled: bool,
    #[serde(default)]
    pub okx_enabled: bool,
    #[serde(default)]
    pub aster_enabled: bool,
    #[serde(default)]
    pub coinbase_enabled: bool,

    // Kraken Spot & Futures
    #[serde(default)]
    pub kraken_spot_api_key: String,
    #[serde(default)]
    pub kraken_spot_secret: String,
    #[serde(default = "default_kraken_spot_url")]
    pub kraken_spot_url: String,
    #[serde(default)]
    pub kraken_futures_api_key: String,
    #[serde(default)]
    pub kraken_futures_secret: String,
    #[serde(default = "default_kraken_futures_url")]
    pub kraken_futures_url: String,

    // Hyperliquid
    #[serde(default)]
    pub hyperliquid_wallet: String,
    #[serde(default)]
    pub hyperliquid_private_key: String,
    #[serde(default = "default_hyperliquid_url")]
    pub hyperliquid_url: String,
    #[serde(default)]
    pub hyperliquid_account_index: Option<u64>,
    #[serde(default)]
    pub hyperliquid_subaccount_index: Option<u64>,

    // Lighter zkDEX
    #[serde(default = "default_lighter_account_index")]
    pub lighter_account_index: String,
    #[serde(default)]
    pub lighter_api_key: String,
    #[serde(default = "default_lighter_url")]
    pub lighter_url: String,
    #[serde(default)]
    pub lighter_subaccount_index: Option<u64>,

    // Vanta.exchange (Orderly Network)
    #[serde(default)]
    pub vanta_account_id: String,
    #[serde(default)]
    pub vanta_orderly_key: String,
    #[serde(default)]
    pub vanta_orderly_secret: String,
    #[serde(default = "default_vanta_url")]
    pub vanta_url: String,
    #[serde(default)]
    pub vanta_subaccount_index: Option<u64>,

    // Robinhood EVM
    #[serde(default)]
    pub robinhood_wallet: String,
    #[serde(default)]
    pub robinhood_private_key: String,
    #[serde(default = "default_robinhood_rpc_url")]
    pub robinhood_rpc_url: String,
    #[serde(default = "default_robinhood_router")]
    pub robinhood_router: String,
    #[serde(default)]
    pub robinhood_subaccount_index: Option<u64>,

    // Polymarket CLOB
    #[serde(default)]
    pub polymarket_api_key: String,
    #[serde(default)]
    pub polymarket_secret: String,
    #[serde(default)]
    pub polymarket_passphrase: String,
    #[serde(default = "default_polymarket_url")]
    pub polymarket_url: String,

    // Kalshi Binary
    #[serde(default)]
    pub kalshi_api_key_id: String,
    #[serde(default)]
    pub kalshi_private_key: String,
    #[serde(default = "default_kalshi_url")]
    pub kalshi_url: String,

    // Arcus CLOB DEX
    #[serde(default)]
    pub arcus_api_key: String,
    #[serde(default)]
    pub arcus_secret: String,
    #[serde(default = "default_arcus_url")]
    pub arcus_url: String,


    // Vault-derived credential configuration state (venue IDs only, no secrets)
    #[serde(default)]
    pub configured_venues: Vec<String>,

    // Historical retention (durable via state_store kv; None = unlimited)
    #[serde(default)]
    pub spread_samples_max_days: Option<u32>,
    #[serde(default)]
    pub spread_samples_max_gb: Option<f64>,

    // Referrals, About & Sponsor Links
    #[serde(default = "default_referral_lighter")]
    pub referral_lighter: String,
    #[serde(default = "default_referral_robinhood")]
    pub referral_robinhood: String,
    #[serde(default = "default_sponsor_link")]
    pub sponsor_link: String,
    #[serde(default = "default_app_version")]
    pub version: String,
    #[serde(default = "default_git_repo")]
    pub git_repo: String,
}

impl Default for EngineSettingsDto {
    fn default() -> Self {
        Self {
            hsm_url: default_hsm_url(),
            hmac_token: default_hmac_token(),
            min_depth_ratio: default_min_depth_ratio(),
            engine_rpc_port: default_engine_rpc_port(),
            engine_ws_port: default_engine_ws_port(),
            bind_address: default_bind_address(),
            trading_mode: default_trading_mode(),
            paper_starting_balance_usd: default_paper_starting_balance(),
            fast_warmup_enabled: default_fast_warmup_enabled(),
            hyperliquid_enabled: false,
            lighter_enabled: false,
            vanta_enabled: false,
            robinhood_enabled: false,
            decibel_enabled: false,
            ibkr_enabled: false,
            ig_enabled: false,
            binance_enabled: false,
            okx_enabled: false,
            aster_enabled: false,
            coinbase_enabled: false,
            kraken_enabled: false,
            kraken_spot_enabled: false,
            kraken_futures_enabled: false,
            polymarket_enabled: false,
            kalshi_enabled: false,
            kraken_spot_api_key: "".into(),
            kraken_spot_secret: "".into(),
            kraken_spot_url: default_kraken_spot_url(),
            kraken_futures_api_key: "".into(),
            kraken_futures_secret: "".into(),
            kraken_futures_url: default_kraken_futures_url(),
            hyperliquid_wallet: "".into(),
            hyperliquid_private_key: "".into(),
            hyperliquid_url: default_hyperliquid_url(),
            hyperliquid_account_index: None,
            hyperliquid_subaccount_index: None,
            lighter_account_index: default_lighter_account_index(),
            lighter_api_key: "".into(),
            lighter_url: default_lighter_url(),
            lighter_subaccount_index: None,
            vanta_account_id: "".into(),
            vanta_orderly_key: "".into(),
            vanta_orderly_secret: "".into(),
            vanta_url: default_vanta_url(),
            vanta_subaccount_index: None,
            robinhood_wallet: "".into(),
            robinhood_private_key: "".into(),
            robinhood_rpc_url: default_robinhood_rpc_url(),
            robinhood_router: default_robinhood_router(),
            robinhood_subaccount_index: None,
            polymarket_api_key: "".into(),
            polymarket_secret: "".into(),
            polymarket_passphrase: "".into(),
            polymarket_url: default_polymarket_url(),
            kalshi_api_key_id: "".into(),
            kalshi_private_key: "".into(),
            kalshi_url: default_kalshi_url(),
            arcus_enabled: false,
            arcus_api_key: "".into(),
            arcus_secret: "".into(),
            arcus_url: default_arcus_url(),
            configured_venues: Vec::new(),
            spread_samples_max_days: None,
            spread_samples_max_gb: None,
            referral_lighter: default_referral_lighter(),
            referral_robinhood: default_referral_robinhood(),
            sponsor_link: default_sponsor_link(),
            version: default_app_version(),
            git_repo: default_git_repo(),
        }
    }
}

fn has_secret_value(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && !trimmed.chars().all(|c| c == '•')
}

fn has_index_value(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed != "0"
}

impl EngineSettingsDto {
    /// Extract submitted credential material into vault-bound records. A
    /// record is emitted only when at least one credential, public
    /// identifier, or index field is supplied for that venue.
    pub fn submitted_credentials(&self) -> Vec<ExchangeCredentials> {
        let mut out = Vec::new();

        if has_secret_value(&self.hmac_token) {
            out.push(ExchangeCredentials {
                venue: "remote_hsm".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: self.hmac_token.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: true,
            });
        }

        let hyperliquid_supplied = has_secret_value(&self.hyperliquid_wallet)
            || has_secret_value(&self.hyperliquid_private_key)
            || self.hyperliquid_account_index.is_some()
            || self.hyperliquid_subaccount_index.is_some();
        if hyperliquid_supplied {
            out.push(ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: self.hyperliquid_private_key.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: if has_secret_value(&self.hyperliquid_wallet) {
                    Some(self.hyperliquid_wallet.trim().to_string())
                } else {
                    None
                },
                account_index: self.hyperliquid_account_index,
                subaccount_index: self.hyperliquid_subaccount_index,
                api_key_index: None,
                private_key: None,
                enabled: self.hyperliquid_enabled,
            });
        }

        let lighter_account_index = self.lighter_account_index.trim().parse::<u64>().ok();
        let lighter_supplied = has_secret_value(&self.lighter_api_key)
            || has_index_value(&self.lighter_account_index)
            || self.lighter_subaccount_index.is_some();
        if lighter_supplied {
            out.push(ExchangeCredentials {
                venue: "lighter".into(),
                profile_id: None,
                api_key: self.lighter_api_key.trim().to_string(),
                api_secret: String::new(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: lighter_account_index,
                subaccount_index: self.lighter_subaccount_index,
                api_key_index: None,
                private_key: None,
                enabled: self.lighter_enabled,
            });
        }

        let vanta_supplied = has_secret_value(&self.vanta_account_id)
            || has_secret_value(&self.vanta_orderly_key)
            || has_secret_value(&self.vanta_orderly_secret)
            || self.vanta_subaccount_index.is_some();
        if vanta_supplied {
            out.push(ExchangeCredentials {
                venue: "vanta".into(),
                profile_id: None,
                api_key: self.vanta_orderly_key.trim().to_string(),
                api_secret: self.vanta_orderly_secret.trim().to_string(),
                passphrase: None,
                account_id: if has_secret_value(&self.vanta_account_id) {
                    Some(self.vanta_account_id.trim().to_string())
                } else {
                    None
                },
                wallet_address: None,
                account_index: None,
                subaccount_index: self.vanta_subaccount_index,
                api_key_index: None,
                private_key: None,
                enabled: self.vanta_enabled,
            });
        }

        let robinhood_supplied = has_secret_value(&self.robinhood_wallet)
            || has_secret_value(&self.robinhood_private_key)
            || self.robinhood_subaccount_index.is_some();
        if robinhood_supplied {
            out.push(ExchangeCredentials {
                venue: "robinhood".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: self.robinhood_private_key.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: if has_secret_value(&self.robinhood_wallet) {
                    Some(self.robinhood_wallet.trim().to_string())
                } else {
                    None
                },
                account_index: None,
                subaccount_index: self.robinhood_subaccount_index,
                api_key_index: None,
                private_key: None,
                enabled: self.robinhood_enabled,
            });
        }

        if has_secret_value(&self.arcus_api_key) || has_secret_value(&self.arcus_secret) {
            out.push(ExchangeCredentials {
                venue: "arcus".into(),
                profile_id: None,
                api_key: self.arcus_api_key.trim().to_string(),
                api_secret: self.arcus_secret.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: self.arcus_enabled,
            });
        }


        if has_secret_value(&self.kraken_spot_api_key) || has_secret_value(&self.kraken_spot_secret)
        {
            out.push(ExchangeCredentials {
                venue: "kraken_spot".into(),
                profile_id: None,
                api_key: self.kraken_spot_api_key.trim().to_string(),
                api_secret: self.kraken_spot_secret.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: self.kraken_enabled && self.kraken_spot_enabled,
            });
        }

        if has_secret_value(&self.kraken_futures_api_key)
            || has_secret_value(&self.kraken_futures_secret)
        {
            out.push(ExchangeCredentials {
                venue: "kraken_futures".into(),
                profile_id: None,
                api_key: self.kraken_futures_api_key.trim().to_string(),
                api_secret: self.kraken_futures_secret.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: self.kraken_enabled && self.kraken_futures_enabled,
            });
        }

        let polymarket_supplied = has_secret_value(&self.polymarket_api_key)
            || has_secret_value(&self.polymarket_secret)
            || has_secret_value(&self.polymarket_passphrase);
        if polymarket_supplied {
            out.push(ExchangeCredentials {
                venue: "polymarket".into(),
                profile_id: None,
                api_key: self.polymarket_api_key.trim().to_string(),
                api_secret: self.polymarket_secret.trim().to_string(),
                passphrase: if has_secret_value(&self.polymarket_passphrase) {
                    Some(self.polymarket_passphrase.trim().to_string())
                } else {
                    None
                },
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: self.polymarket_enabled,
            });
        }

        if has_secret_value(&self.kalshi_api_key_id) || has_secret_value(&self.kalshi_private_key) {
            out.push(ExchangeCredentials {
                venue: "kalshi".into(),
                profile_id: None,
                api_key: self.kalshi_api_key_id.trim().to_string(),
                api_secret: self.kalshi_private_key.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: self.kalshi_enabled,
            });
        }

        out
    }

    /// Remove every credential identifier/secret held in settings. URLs,
    /// router addresses, enable toggles, and risk/network settings are kept.
    pub fn clear_credential_material(&mut self) {
        self.hmac_token.zeroize();
        self.kraken_spot_api_key.zeroize();
        self.kraken_spot_secret.zeroize();
        self.kraken_futures_api_key.zeroize();
        self.kraken_futures_secret.zeroize();
        self.hyperliquid_wallet.zeroize();
        self.hyperliquid_private_key.zeroize();
        self.hyperliquid_account_index = None;
        self.hyperliquid_subaccount_index = None;
        self.lighter_account_index.zeroize();
        self.lighter_api_key.zeroize();
        self.lighter_subaccount_index = None;
        self.vanta_account_id.zeroize();
        self.vanta_orderly_key.zeroize();
        self.vanta_orderly_secret.zeroize();
        self.vanta_subaccount_index = None;
        self.robinhood_wallet.zeroize();
        self.robinhood_private_key.zeroize();
        self.robinhood_subaccount_index = None;
        self.arcus_api_key.zeroize();
        self.arcus_secret.zeroize();
        self.polymarket_api_key.zeroize();
        self.polymarket_secret.zeroize();
        self.polymarket_passphrase.zeroize();
        self.kalshi_api_key_id.zeroize();
        self.kalshi_private_key.zeroize();
    }
}

/// Cached 24h venue volumes: fetch instant + (venue_id -> asset -> volume USD).
type VolumeCache = Mutex<
    Option<(
        std::time::Instant,
        std::collections::HashMap<String, std::collections::HashMap<String, f64>>,
    )>,
>;

#[allow(dead_code)]
pub struct AppEngineState {
    pub ex_hl: Arc<SimulatedExchange>,
    pub ex_lighter: Arc<SimulatedExchange>,
    pub ex_vanta: Arc<SimulatedExchange>,
    pub ex_robinhood: Arc<SimulatedExchange>,
    pub ex_arcus: Arc<SimulatedExchange>,
    pub ex_aster: Arc<SimulatedExchange>,
    pub ex_binance: Arc<SimulatedExchange>,
    pub ex_okx: Arc<SimulatedExchange>,
    pub ex_coinbase: Arc<SimulatedExchange>,
    pub ex_kraken: Arc<SimulatedExchange>,
    pub ex_decibel: Arc<SimulatedExchange>,
    pub live_hl: Arc<HyperliquidAdapter>,
    pub live_lighter: Arc<LighterAdapter>,
    pub live_vanta: Arc<VantaAdapter>,
    pub live_robinhood: Arc<LighterAdapter>,
    pub live_arcus: Arc<ArcusAdapter>,
    pub live_aster: Arc<AsterAdapter>,
    pub live_binance: Arc<BinanceAdapter>,
    pub live_okx: Arc<OkxAdapter>,
    pub live_coinbase: Arc<CoinbaseAdapter>,
    pub live_kraken: Arc<KrakenAdapter>,
    pub live_decibel: Arc<DecibelAdapter>,
    // Long-tail perp venues (one shared adapter type, per-venue specs).
    pub live_polymarket: Arc<PolymarketAdapter>,
    pub live_kalshi: Arc<KalshiAdapter>,
    pub market_data_hub: Arc<MarketDataHub>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub delta_guard: RwLock<DeltaGuard>,
    pub coordinator: Arc<AtomicArbitrageCoordinator>,
    pub kalman_filter: RwLock<HedgeRatioKalmanFilter>,
    pub is_live_mode: RwLock<bool>,
    pub total_realized_profit: RwLock<Decimal>,
    pub total_realized_profit_live: RwLock<Decimal>,
    pub execution_logs: RwLock<Vec<ExecutionLogEntry>>,
    pub sim_step_counter: RwLock<usize>,
    pub settings: RwLock<EngineSettingsDto>,
    pub strategy_configs: RwLock<std::collections::HashMap<String, TickerStrategyConfigDto>>,
    /// Universal per-strategy parameters keyed by strategy_id ("futures" |
    /// "funding" | "spot"). These are the canonical strategy settings — the
    /// per-symbol strategy_configs map only retains ticker-specific runtime
    /// state (accumulated units, cached unit size).
    pub strategy_params: RwLock<std::collections::HashMap<String, StrategyParamsDto>>,
    pub vault: RwLock<crate::vault::SecureVault>,
    pub state_store: Arc<crate::state_store::StateStore>,
    pub virtual_ledger: RwLock<arbitrageur_execution::virtual_ledger::VirtualStrategyLedger>,
    pub is_paused: RwLock<bool>,
    pub execution_lock: tokio::sync::Mutex<()>,
    pub autotune_apply_lock: tokio::sync::Mutex<()>,
    /// Epoch hour of the last spread_samples prune pass.
    pub sample_prune_at: Mutex<i64>,
    /// Last margin-snapshot write per venue (epoch ms). Bounds the snapshot
    /// cadence: fills and heartbeats record at most one row per venue per
    /// `MARGIN_SNAPSHOT_MIN_INTERVAL_MS`.
    pub margin_snapshot_at: Mutex<std::collections::HashMap<String, i64>>,
    /// Exponentially-smoothed round-trip latency (ms) per venue, updated on
    /// each successful probe so the displayed value is a rolling average
    /// rather than a single jumpy sample. Entries are removed when a probe
    /// fails so the UI falls back to N/A instead of a stale number.
    pub latency_ema: Mutex<std::collections::HashMap<String, f64>>,
    /// When each venue was last latency-probed. Probes are throttled —
    /// re-measuring on every 3s status poll generates request volume that
    /// trips edge WAFs without meaningfully fresher data.
    pub latency_probe_at: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// 24h volume cache: venue_id -> asset -> volume USD (5-minute TTL).
    pub volume_cache: VolumeCache,
    /// Spread Range Monitor engine, installed by main() after construction
    /// (it needs a fully-built state to hydrate from).
    pub monitor: Mutex<Option<Arc<crate::monitor::SpreadMonitorEngine>>>,
    /// Autonomous convergence bot, installed by main() after construction.
    /// `None` until installed; IPC reports "not running" rather than a
    /// fabricated status.
    pub radar_bot: Mutex<Option<Arc<crate::auto_exec::OpportunityExecutor>>>,
    /// In-flight dedup for orderbook REST fetches, keyed by
    /// (venue, symbol). Concurrent callers share one request.
    pub orderbook_flights: Arc<crate::ws_primary::OrderbookSingleFlight>,
}

/// Canonical venue resolution: only exact trimmed lowercase IDs, documented
/// aliases, and the current display labels are accepted. Substring matches
/// such as "evilhyperliquid" are rejected.
pub(crate) fn canonical_venue_id(input: &str) -> Option<&'static str> {
    match input.trim().to_lowercase().as_str() {
        "hyperliquid"
        | "hl"
        | "hyperliquid (l1)"
        | "hyperliquid (l1 dex)"
        | "hyperliquid (l1, read-only)" => Some("hyperliquid"),
        "lighter" | "lgt" | "lighter (zkdex)" | "lighter (zkdex, read-only)" => Some("lighter"),
        "robinhood" | "rh" | "robinhood lighter" | "robinhood lighter (read-only)" => {
            Some("robinhood")
        }
        "vanta" | "vnt" | "vanta.exchange" | "vanta (orderly)" | "vanta (orderly, read-only)" => {
            Some("vanta")
        }
        "aster" | "ast" | "asterdex" | "aster (perp dex)" | "aster (perp dex, read-only)" => Some("aster"),
        "arcus" | "arc" | "arcus dex" | "arcus (fast clob)" | "arcus (perp dex)" => Some("arcus"),
        "binance" | "bnb" | "bin" | "binance futures" | "binance (perp)" => Some("binance"),
        "okx" | "okx swaps" | "okx (perp)" => Some("okx"),
        "coinbase" | "cb" | "coinbase exchange" => Some("coinbase"),
        "kraken" | "krk" | "kfut" | "kraken futures" | "kraken spot" => Some("kraken"),
        "decibel" | "dcb" | "decibel (aptos)" => Some("decibel"),
        _ => None,
    }
}

pub(crate) fn load_strategy_params(
    store: &crate::state_store::StateStore,
) -> std::collections::HashMap<String, StrategyParamsDto> {
    let mut map = std::collections::HashMap::new();
    if let Ok(rows) = store.strategy_params_json() {
        for (id, json) in rows {
            if !matches!(id.as_str(), "futures" | "funding" | "spot") {
                continue;
            }
            match serde_json::from_str::<StrategyParamsDto>(&json) {
                Ok(p) if p.strategy_id.eq_ignore_ascii_case(&id) => {
                    map.insert(id, p);
                }
                _ => {}
            }
        }
    }
    map
}

impl AppEngineState {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::new_with_vault_path(crate::vault::SecureVault::default_path())
    }

    pub fn new_persistent() -> anyhow::Result<Self> {
        let state_store = Arc::new(crate::state_store::StateStore::open(
            crate::state_store::StateStore::default_path(),
        )?);
        Ok(Self::new_inner(
            crate::vault::SecureVault::default_path(),
            state_store,
        ))
    }

    #[cfg(test)]
    pub fn new_inner_for_test(state_store: Arc<crate::state_store::StateStore>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arbitrageur_retention_vault_{}",
            rand::random::<u64>()
        ));
        Self::new_inner(dir.join("vault.enc"), state_store)
    }

    #[cfg(test)]
    pub fn new_with_vault_path(vault_path: PathBuf) -> Self {
        let state_store = Arc::new(
            crate::state_store::StateStore::open_in_memory()
                .expect("in-memory state store must open"),
        );
        Self::new_inner(vault_path, state_store)
    }

    fn new_inner(vault_path: PathBuf, state_store: Arc<crate::state_store::StateStore>) -> Self {
        let ex_hl = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid (L1)",
            2,
            dec!(2.0),
            dec!(100000.0),
        ));
        let ex_lighter = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter (zkDEX)",
            5,
            dec!(1.5),
            dec!(100000.0),
        ));
        let ex_vanta = Arc::new(SimulatedExchange::new(
            "vanta",
            "Vanta.exchange",
            4,
            dec!(3.0),
            dec!(100000.0),
        ));
        let ex_robinhood = Arc::new(SimulatedExchange::new(
            "robinhood",
            "Robinhood Lighter",
            8,
            dec!(2.5),
            dec!(100000.0),
        ));
        let ex_arcus = Arc::new(SimulatedExchange::new(
            "arcus",
            "Arcus",
            3,
            dec!(1.5),
            dec!(100000.0),
        ));
        let ex_aster = Arc::new(SimulatedExchange::new(
            "aster",
            "Aster",
            3,
            dec!(3.5),
            dec!(100000.0),
        ));
        let ex_binance = Arc::new(SimulatedExchange::new(
            "binance",
            "Binance",
            2,
            dec!(2.0),
            dec!(100000.0),
        ));
        let ex_okx = Arc::new(SimulatedExchange::new(
            "okx",
            "OKX",
            2,
            dec!(2.0),
            dec!(100000.0),
        ));
        let ex_coinbase = Arc::new(SimulatedExchange::new(
            "coinbase",
            "Coinbase",
            3,
            dec!(10.0),
            dec!(100000.0),
        ));
        let ex_kraken = Arc::new(SimulatedExchange::new(
            "kraken",
            "Kraken",
            3,
            dec!(2.0),
            dec!(100000.0),
        ));
        let ex_decibel = Arc::new(SimulatedExchange::new(
            "decibel",
            "Decibel",
            4,
            dec!(2.5),
            dec!(100000.0),
        ));

        // No synthetic market seeding. All market quotes and orderbooks are fetched dynamically
        // from live venue APIs once API credentials are configured by the user.

        let live_hl = Arc::new(HyperliquidAdapter::new(None, None, None));
        let live_lighter = Arc::new(LighterAdapter::new(None, None, None));
        let live_vanta = Arc::new(VantaAdapter::new(None, None, None, None));
        // Robinhood Chain serves the Lighter protocol at a different base
        // URL — reuse the Lighter adapter under the robinhood venue id.
        let live_robinhood = Arc::new(LighterAdapter::new_with_venue(
            Some("https://api.rh.lighter.xyz".to_string()),
            None,
            None,
            "robinhood",
        ));
        let live_arcus = Arc::new(ArcusAdapter::new(None, None, None));
        let live_aster = Arc::new(AsterAdapter::new(None, None, None));
        let live_binance = Arc::new(BinanceAdapter::new(None, None, None));
        let live_okx = Arc::new(OkxAdapter::new(None, None, None, None));
        let live_coinbase = Arc::new(CoinbaseAdapter::new(None, None, None, None));
        let live_kraken = Arc::new(KrakenAdapter::new(None, None, None, None));
        let live_decibel = Arc::new(DecibelAdapter::new(None, None, None));
        let live_polymarket = Arc::new(PolymarketAdapter::new(None, None));
        let live_kalshi = Arc::new(KalshiAdapter::new(None, None));
        let market_data_hub = MarketDataHub::new(1024, 400);

        let cb_config = CircuitBreakerConfig {
            max_daily_drawdown_usd: dec!(25000.0),
            max_daily_drawdown_pct: dec!(5.0),
            max_consecutive_errors: 5,
            max_spread_divergence_bps: dec!(250.0),
        };
        let cb = Arc::new(CircuitBreaker::new(cb_config, dec!(200000.0)));
        let delta_guard = RwLock::new(DeltaGuard::new(dec!(2500.0)));
        let router = SmartOrderRouter::new(dec!(15.0)); // 15 bps slippage tolerance
        let coordinator = Arc::new(AtomicArbitrageCoordinator::new(router, cb.clone()));
        let kalman_filter = RwLock::new(HedgeRatioKalmanFilter::new(1e-4, 1e-2));

        let mut initial_settings = EngineSettingsDto::default();
        if let Ok(Some(mode)) = state_store.kv_get("engine.trading_mode") {
            if mode == "real" || mode == "paper" {
                initial_settings.trading_mode = mode;
            }
        }
        if let Ok(Some(raw)) = state_store.kv_get("retention.max_days") {
            if let Ok(days) = raw.parse::<u32>() {
                if days > 0 {
                    initial_settings.spread_samples_max_days = Some(days);
                }
            }
        }
        if let Ok(Some(raw)) = state_store.kv_get("retention.max_gb") {
            if let Ok(gb) = raw.parse::<f64>() {
                if gb.is_finite() && gb > 0.0 {
                    initial_settings.spread_samples_max_gb = Some(gb);
                }
            }
        }
        let settings = RwLock::new(initial_settings);

        let initial_logs: Vec<ExecutionLogEntry> = Vec::new();
        let vault = RwLock::new(crate::vault::SecureVault::new_with_path(vault_path));
        let virtual_ledger =
            RwLock::new(arbitrageur_execution::virtual_ledger::VirtualStrategyLedger::new());
        let strategy_params_map = load_strategy_params(&state_store);

        Self {
            ex_hl,
            ex_lighter,
            ex_vanta,
            ex_robinhood,
            ex_arcus,
            ex_aster,
            ex_binance,
            ex_okx,
            ex_coinbase,
            ex_kraken,
            ex_decibel,
            live_hl,
            live_lighter,
            live_vanta,
            live_robinhood,
            live_arcus,
            live_aster,
            live_binance,
            live_okx,
            live_coinbase,
            live_kraken,
            live_decibel,
            live_polymarket,
            live_kalshi,
            market_data_hub,
            circuit_breaker: cb,
            delta_guard,
            coordinator,
            kalman_filter,
            is_live_mode: RwLock::new(false),
            total_realized_profit: RwLock::new(dec!(0)),
            total_realized_profit_live: RwLock::new(dec!(0)),
            execution_logs: RwLock::new(initial_logs),
            sim_step_counter: RwLock::new(8),
            settings,
            strategy_configs: RwLock::new(std::collections::HashMap::new()),
            strategy_params: RwLock::new(strategy_params_map),
            vault,
            state_store,
            virtual_ledger,
            is_paused: RwLock::new(false),
            execution_lock: tokio::sync::Mutex::new(()),
            autotune_apply_lock: tokio::sync::Mutex::new(()),
            sample_prune_at: Mutex::new(0),
            margin_snapshot_at: Mutex::new(std::collections::HashMap::new()),
            latency_ema: Mutex::new(std::collections::HashMap::new()),
            latency_probe_at: Mutex::new(std::collections::HashMap::new()),
            volume_cache: Mutex::new(None),
            monitor: Mutex::new(None),
            radar_bot: Mutex::new(None),
            orderbook_flights: Arc::new(crate::ws_primary::OrderbookSingleFlight::new()),
        }
    }

    /// Ensure market is loaded - live real market data is queried from active exchange APIs
    pub fn ensure_market(&self, _symbol: &str) {
        // No-op: Real market data is pulled directly from configured live exchange APIs only.
        // No synthetic markets are ever seeded or fabricated.
    }

    /// Synchronize credentials from the secure vault to live exchange adapters.
    /// Clears credentials if vault is locked.
    pub fn sync_vault_credentials_to_adapters(&self) {
        let mut vault = self.vault.write();
        if !vault.is_unlocked() {
            self.live_aster.update_credentials(None, None);
            self.live_hl.update_credentials(None, None);
            self.live_lighter.update_credentials(None, None);
            self.live_robinhood.update_credentials(None, None);
            self.live_vanta.update_credentials(None, None, None);
            self.live_binance.update_credentials(None, None);
            self.live_okx.update_credentials(None, None, None);
            self.live_coinbase.update_credentials(None, None, None);
            self.live_kraken.update_credentials(None, None);
            self.live_decibel.update_credentials(None, None, None, None);
            return;
        }

        if let Ok(data) = vault.get_data() {
            let get_active_cred = |venues: &[&str]| -> Option<&crate::vault::ExchangeCredentials> {
                for &v in venues {
                    if let Ok(Some(acc)) = self.state_store.default_account(v) {
                        if let Some(cred) = data.credentials.iter().find(|c| {
                            c.enabled && c.profile_id.as_deref() == Some(&acc.id)
                        }) {
                            return Some(cred);
                        }
                    }
                    let has_accounts = self
                        .state_store
                        .list_accounts(Some(v))
                        .map(|l| !l.is_empty())
                        .unwrap_or(false);
                    if has_accounts {
                        return None;
                    }
                }
                data.credentials.iter().find(|c| {
                    c.enabled && venues.iter().any(|v| c.venue.eq_ignore_ascii_case(v))
                })
            };

            let mut aster_key = None;
            let mut aster_secret = None;
            if let Some(cred) = get_active_cred(&["aster", "asterdex"]) {
                if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                    aster_key = Some(cred.api_key.clone());
                    aster_secret = Some(cred.api_secret.clone());
                }
            }

            let mut hl_wallet = None;
            let mut hl_pk = None;
            if let Some(cred) = get_active_cred(&["hyperliquid", "hl"]) {
                let wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
                let pk = cred.private_key.clone().filter(|k| !k.trim().is_empty());
                if wallet.is_some() && pk.is_some() {
                    hl_wallet = wallet;
                    hl_pk = pk;
                }
            }

            let mut lighter_idx = None;
            let mut lighter_key = None;
            if let Some(cred) = get_active_cred(&["lighter"]) {
                let key = if !cred.api_key.trim().is_empty() {
                    Some(cred.api_key.clone())
                } else {
                    cred.private_key.clone().filter(|k| !k.trim().is_empty())
                };
                if key.is_some() {
                    lighter_key = key;
                    lighter_idx = cred.account_index;
                }
            }

            let mut rh_idx = None;
            let mut rh_key = None;
            if let Some(cred) = get_active_cred(&["robinhood", "robinhood lighter", "rh"]) {
                let key = if !cred.api_key.trim().is_empty() {
                    Some(cred.api_key.clone())
                } else {
                    cred.private_key.clone().filter(|k| !k.trim().is_empty())
                };
                if key.is_some() {
                    rh_key = key;
                    rh_idx = cred.account_index;
                }
            }

            let mut vanta_acc = None;
            let mut vanta_key = None;
            let mut vanta_sec = None;
            if let Some(cred) = get_active_cred(&["vanta", "orderly"]) {
                if !cred.api_key.trim().is_empty() {
                    vanta_key = Some(cred.api_key.clone());
                }
                if !cred.api_secret.trim().is_empty() {
                    vanta_sec = Some(cred.api_secret.clone());
                }
                vanta_acc = cred.account_id.clone().filter(|a| !a.trim().is_empty())
                    .or_else(|| cred.wallet_address.clone().filter(|a| !a.trim().is_empty()));
            }

            let mut binance_key = None;
            let mut binance_sec = None;
            if let Some(cred) = get_active_cred(&["binance", "bnb"]) {
                if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                    binance_key = Some(cred.api_key.clone());
                    binance_sec = Some(cred.api_secret.clone());
                }
            }

            let mut okx_key = None;
            let mut okx_sec = None;
            let mut okx_pass = None;
            if let Some(cred) = get_active_cred(&["okx"]) {
                if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                    okx_key = Some(cred.api_key.clone());
                    okx_sec = Some(cred.api_secret.clone());
                    okx_pass = cred.passphrase.clone().filter(|p| !p.trim().is_empty());
                }
            }

            let mut cb_key = None;
            let mut cb_sec = None;
            let mut cb_pass = None;
            if let Some(cred) = get_active_cred(&["coinbase", "cb"]) {
                if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                    cb_key = Some(cred.api_key.clone());
                    cb_sec = Some(cred.api_secret.clone());
                    cb_pass = cred.passphrase.clone().filter(|p| !p.trim().is_empty());
                }
            }

            let mut kraken_key = None;
            let mut kraken_sec = None;
            if let Some(cred) = get_active_cred(&["kraken", "krk", "kraken_futures", "kraken_spot"]) {
                if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                    kraken_key = Some(cred.api_key.clone());
                    kraken_sec = Some(cred.api_secret.clone());
                }
            }

            let mut decibel_key = None;
            let mut decibel_node_key = None;
            let mut decibel_wallet = None;
            let mut decibel_pk = None;
            if let Some(cred) = get_active_cred(&["decibel", "dcb", "decibel (aptos)"]) {
                if !cred.api_key.trim().is_empty() {
                    decibel_key = Some(cred.api_key.clone());
                }
                decibel_node_key = cred.passphrase.clone().filter(|p| !p.trim().is_empty())
                    .or_else(|| if !cred.api_secret.trim().is_empty() { Some(cred.api_secret.clone()) } else { None });
                decibel_wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
                decibel_pk = cred.private_key.clone().filter(|k| !k.trim().is_empty());
            }

            self.live_aster.update_credentials(aster_key, aster_secret);
            self.live_hl.update_credentials(hl_wallet, hl_pk);
            self.live_lighter.update_credentials(lighter_idx, lighter_key);
            self.live_robinhood.update_credentials(rh_idx, rh_key);
            self.live_vanta.update_credentials(vanta_acc, vanta_key, vanta_sec);
            self.live_binance.update_credentials(binance_key, binance_sec);
            self.live_okx.update_credentials(okx_key, okx_sec, okx_pass);
            self.live_coinbase.update_credentials(cb_key, cb_sec, cb_pass);
            self.live_kraken.update_credentials(kraken_key, kraken_sec);
            self.live_decibel.update_credentials(
                decibel_key,
                decibel_node_key,
                decibel_wallet,
                decibel_pk,
            );



        } else {
            self.live_aster.update_credentials(None, None);
            self.live_hl.update_credentials(None, None);
            self.live_lighter.update_credentials(None, None);
            self.live_robinhood.update_credentials(None, None);
            self.live_vanta.update_credentials(None, None, None);
            self.live_binance.update_credentials(None, None);
            self.live_okx.update_credentials(None, None, None);
            self.live_coinbase.update_credentials(None, None, None);
            self.live_kraken.update_credentials(None, None);
            self.live_decibel.update_credentials(None, None, None, None);
        }
    }

    /// Returns admitted venue adapters. Hyperliquid, Lighter, Robinhood
    /// Lighter, Vanta (Orderly), Aster, Ondo Perps, GRVT, edgeX, and
    /// Variational are exposed as native market-data adapters. Public
    /// read-only feeds require enablement, not secrets. PopDEX stays out
    /// until its closed-beta REST shape is published (its adapter is
    /// credential-ready but its market-data paths fail closed).
    pub fn get_venues(
        &self,
    ) -> Vec<(
        &'static str,
        &'static str,
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    )> {
        let mut list: Vec<(
            &'static str,
            &'static str,
            Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
        )> = Vec::new();
        let s = self.settings.read();
        // A venue is on when its engine flag is set OR it owns an enabled
        // account — the account-zone switch is the same on/off as the flag.
        let account_enabled =
            |venue: &str| self.state_store.has_enabled_account(venue).unwrap_or(false);
        if s.hyperliquid_enabled || account_enabled("hyperliquid") {
            list.push((
                "hyperliquid",
                "Hyperliquid",
                self.live_hl.clone(),
            ));
        }
        if s.lighter_enabled || account_enabled("lighter") {
            list.push((
                "lighter",
                "Lighter",
                self.live_lighter.clone(),
            ));
        }
        if s.robinhood_enabled || account_enabled("robinhood") {
            list.push((
                "robinhood",
                "Robinhood Lighter",
                self.live_robinhood.clone(),
            ));
        }
        if s.vanta_enabled || account_enabled("vanta") {
            list.push((
                "vanta",
                "Vanta",
                self.live_vanta.clone(),
            ));
        }
        if s.aster_enabled || account_enabled("aster") {
            list.push((
                "aster",
                "Aster",
                self.live_aster.clone(),
            ));
        }
        if s.binance_enabled || account_enabled("binance") {
            list.push((
                "binance",
                "Binance",
                self.live_binance.clone(),
            ));
        }
        if s.okx_enabled || account_enabled("okx") {
            list.push((
                "okx",
                "OKX",
                self.live_okx.clone(),
            ));
        }
        if s.coinbase_enabled || account_enabled("coinbase") {
            list.push((
                "coinbase",
                "Coinbase",
                self.live_coinbase.clone(),
            ));
        }
        if s.kraken_enabled || s.kraken_futures_enabled || s.kraken_spot_enabled || account_enabled("kraken") {
            list.push((
                "kraken",
                "Kraken",
                self.live_kraken.clone(),
            ));
        }
        if s.decibel_enabled || account_enabled("decibel") {
            list.push((
                "decibel",
                "Decibel",
                self.live_decibel.clone(),
            ));
        }
        list
    }

    /// Resolves an exchange adapter by venue identifier or documented alias.
    /// Only canonical exact matches are accepted and only venues admitted by
    /// `get_venues` are reachable; disabled, unsupported, unknown names, and
    /// near-match substrings return None.
    pub fn get_adapter(
        &self,
        venue_id: &str,
    ) -> Option<Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>> {
        let canonical = canonical_venue_id(venue_id)?;
        self.get_venues()
            .into_iter()
            .find(|(vid, _, _)| *vid == canonical)
            .map(|(_, _, adapter)| adapter)
    }

    /// Resolves the paper execution exchange for a venue. A simulator is
    /// returned only when the venue is currently admitted by `get_venues`;
    /// no other venue has a paper exchange.
    pub fn get_paper_exchange(&self, venue_id: &str) -> Option<Arc<SimulatedExchange>> {
        let canonical = canonical_venue_id(venue_id)?;
        let admitted = self
            .get_venues()
            .into_iter()
            .any(|(vid, _, _)| vid == canonical);
        if !admitted {
            return None;
        }
        match canonical {
            "hyperliquid" => Some(self.ex_hl.clone()),
            "lighter" => Some(self.ex_lighter.clone()),
            "robinhood" => Some(self.ex_robinhood.clone()),
            "vanta" => Some(self.ex_vanta.clone()),
            "aster" => Some(self.ex_aster.clone()),
            "binance" => Some(self.ex_binance.clone()),
            "okx" => Some(self.ex_okx.clone()),
            "coinbase" => Some(self.ex_coinbase.clone()),
            "kraken" => Some(self.ex_kraken.clone()),
            "decibel" => Some(self.ex_decibel.clone()),
            _ => None,
        }
    }

    /// Sum of seed equity across all paper exchanges — the baseline that
    /// realized paper P&L is measured against. Each exchange tracks its own
    /// initial equity so a starting-balance reset moves the baseline too.
    pub fn paper_baseline_equity(&self) -> Decimal {
        self.paper_exchanges()
            .iter()
            .fold(dec!(0), |acc, (_, ex)| acc + ex.initial_equity())
    }

    /// All paper exchanges used for portfolio accounting, regardless of
    /// venue enablement toggles.
    pub fn paper_exchanges(&self) -> Vec<(&'static str, Arc<SimulatedExchange>)> {
        vec![
            ("hyperliquid", self.ex_hl.clone()),
            ("lighter", self.ex_lighter.clone()),
            ("robinhood", self.ex_robinhood.clone()),
            ("vanta", self.ex_vanta.clone()),
            ("aster", self.ex_aster.clone()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_params_old_rows_load_with_sizing_defaults() {
        let old = serde_json::json!({
            "strategyId": "futures",
            "targetProfitBps": 1.0,
            "windowPercentile": 50,
            "slidingWindow": "24h",
            "slidingWindowMode": "manual",
            "slidingWindowSecs": 86400,
            "maxAccumulatedUnits": 3,
            "executionMode": "SINGLE_WAY",
            "unitSizeUsd": 2500.0,
            "userTargetLeverage": 5,
            "fundingTradeSizeUsd": 25000.0,
            "fundingMinAprPct": 8.0,
            "fundingHoldingDays": 14,
            "spotTradeSizeUsd": 1000.0,
            "spotMaxSlippageBps": 5.0,
            "spotMinNetSpreadBps": 15.0
        });
        let p: StrategyParamsDto =
            serde_json::from_value(old).expect("old rows must still load");
        assert_eq!(p.step_size_mult, 10.0);
        assert_eq!(p.max_step_usd, 2500.0);
        assert_eq!(p.max_ticker_equity_pct, 20.0);
        assert_eq!(p.strategy_id, "futures");
        assert_eq!(p.futures_max_slippage_bps, 15.0);
        assert_eq!(p.funding_max_slippage_bps, 15.0);
        assert_eq!(p.prediction_max_slippage_bps, 15.0);
        assert_eq!(p.funding_max_entry_spread_bps, 50.0);
        assert_eq!(p.funding_auto_exit_spread_pct, None);
        assert_eq!(p.funding_liq_trim_pct, None);
        assert!(!p.funding_auto_rebalance);
        assert_eq!(p.funding_rotation_threshold_bps, None);
        assert_eq!(p.futures_liq_trim_pct, None);
        assert_eq!(p.spot_liq_trim_pct, None);
        assert_eq!(p.prediction_liq_trim_pct, None);
    }

    #[test]
    fn test_load_strategy_params_skips_malformed_and_mismatched_rows() {
        let store = crate::state_store::StateStore::open_in_memory().unwrap();
        store
            .upsert_strategy_params_json("futures", "not json at all")
            .unwrap();
        store
            .upsert_strategy_params_json(
                "funding",
                &serde_json::to_string(&StrategyParamsDto::defaults_for("spot")).unwrap(),
            )
            .unwrap();
        let valid = StrategyParamsDto {
            sliding_window_mode: "auto".into(),
            sliding_window_secs: 7_200,
            ..StrategyParamsDto::defaults_for("spot")
        };
        store
            .upsert_strategy_params_json("spot", &serde_json::to_string(&valid).unwrap())
            .unwrap();

        let loaded = load_strategy_params(&store);
        assert!(!loaded.contains_key("futures"), "malformed JSON ignored");
        assert!(
            !loaded.contains_key("funding"),
            "payload strategy_id must match the row key"
        );
        let spot = loaded.get("spot").expect("valid row loads");
        assert_eq!(spot.sliding_window_mode, "auto");
        assert_eq!(spot.sliding_window_secs, 7_200);
    }

    #[tokio::test]
    async fn test_app_state_initialization() {
        let state = AppEngineState::new();
        assert!(!*state.is_live_mode.read());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
        assert!(state.circuit_breaker.is_trading_allowed().is_ok());

        // Zero synthetic venue connectivity without configured credentials
        let unconfigured = state.get_venues();
        assert_eq!(unconfigured.len(), 0);
    }

    #[tokio::test]
    async fn test_circuit_breaker_trip_and_reset() {
        let state = AppEngineState::new();
        assert!(state.circuit_breaker.is_trading_allowed().is_ok());

        state.circuit_breaker.trip("TEST MANUAL TRIP");
        assert!(state.circuit_breaker.is_trading_allowed().is_err());

        state.circuit_breaker.reset();
        assert!(state.circuit_breaker.is_trading_allowed().is_ok());
    }

    #[tokio::test]
    async fn test_venues_resolution_and_adapter_lookup() {
        let state = AppEngineState::new();

        let venues_unconfigured = state.get_venues();
        assert_eq!(venues_unconfigured.len(), 0);
        assert!(state.get_adapter("hyperliquid").is_none());
        assert!(state.get_adapter("nonexistent").is_none());

        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
            s.vanta_enabled = true;
            s.robinhood_enabled = true;
            s.arcus_enabled = true;
            s.polymarket_enabled = true;
            s.kalshi_enabled = true;
            s.aster_enabled = true;
        }
        let venues_paper = state.get_venues();
        assert_eq!(venues_paper.len(), 5);
        assert_eq!(venues_paper[0].0, "hyperliquid");
        assert_eq!(venues_paper[1].0, "lighter");
        assert_eq!(venues_paper[2].0, "robinhood");
        assert_eq!(venues_paper[3].0, "vanta");
        assert_eq!(venues_paper[4].0, "aster");

        assert!(state.get_adapter("hyperliquid").is_some());
        assert!(state.get_adapter("lighter").is_some());
        assert!(state.get_adapter("robinhood").is_some());
        assert!(state.get_adapter("vanta").is_some());
        assert!(state.get_adapter("aster").is_some());
        assert!(state.get_adapter("arcus").is_none());
        // Removed venues without native trading API stay unresolvable.
        assert!(state.get_adapter("popdex").is_none());
        assert!(state.get_adapter("ondoperps").is_none());
        assert!(state.get_adapter("ondo").is_none());
        assert!(state.get_adapter("grvt").is_none());
        assert!(state.get_adapter("edgex").is_none());
        assert!(state.get_adapter("variational").is_none());
        assert!(state.get_adapter("dydx").is_none());
        assert!(state.get_adapter("polymarket").is_none());
        assert!(state.get_adapter("kalshi").is_none());
        assert!(state.get_adapter("nonexistent").is_none());
        assert!(state.get_paper_exchange("aster").is_some());
        assert!(state.get_paper_exchange("ondoperps").is_none());
        assert!(state.get_paper_exchange("grvt").is_none());
        assert!(state.get_paper_exchange("edgex").is_none());

        *state.is_live_mode.write() = true;
        // In live mode, enabled venues remain admitted for market data streaming,
        // while order execution is selectively gated by adapter capability & credentials.
        assert!(!state.get_venues().is_empty());
        assert!(state.get_adapter("hyperliquid").unwrap().supports_live_execution());
        assert!(state.get_adapter("aster").unwrap().supports_live_execution());
        assert!(state.get_adapter("vanta").unwrap().supports_live_execution());
        assert!(!state.get_adapter("hyperliquid").unwrap().has_live_credentials());
    }

    #[tokio::test]
    async fn test_canonical_venue_resolution_rejects_near_matches() {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
        }

        for alias in [
            "hyperliquid",
            "HL",
            "Hyperliquid (L1, Read-Only)",
            " hyperliquid ",
        ] {
            assert!(
                state.get_adapter(alias).is_some(),
                "alias '{}' must resolve",
                alias
            );
            assert!(
                state.get_paper_exchange(alias).is_some(),
                "paper alias '{}' must resolve",
                alias
            );
        }
        for alias in ["lighter", "LGT", "Lighter (zkDEX, Read-Only)"] {
            assert!(state.get_adapter(alias).is_some());
            assert!(state.get_paper_exchange(alias).is_some());
        }

        for bad in [
            "evilhyperliquid",
            "hyperliquid-evil",
            "lighter-attacker",
            "my lighter",
            "hyperliquidx",
            "vantax",
            "",
            "hl hl",
        ] {
            assert!(
                state.get_adapter(bad).is_none(),
                "'{}' must not resolve to an adapter",
                bad
            );
            assert!(
                state.get_paper_exchange(bad).is_none(),
                "'{}' must not resolve to a paper exchange",
                bad
            );
        }

        {
            let mut s = state.settings.write();
            s.lighter_enabled = false;
        }
        assert!(state.get_adapter("lighter").is_none());
        assert!(state.get_paper_exchange("lighter").is_none());
        assert!(state.get_adapter("hyperliquid").is_some());
    }

    #[tokio::test]
    async fn test_engine_settings_defaults() {
        let state = AppEngineState::new();
        let s = state.settings.read().clone();
        assert_eq!(s.min_depth_ratio, 3.0);
        assert!(!s.hsm_url.is_empty());
        assert!(s.hmac_token.is_empty());
        assert!(!s.robinhood_router.is_empty());
        assert_eq!(s.hyperliquid_url, "https://api.hyperliquid.xyz");
        assert_eq!(s.lighter_url, "https://mainnet.zklighter.elliot.ai");
        assert_eq!(s.vanta_url, "https://api.orderly.org");
        assert_eq!(s.polymarket_url, "https://clob.polymarket.com");
        assert_eq!(s.kalshi_url, "https://trading-api.kalshi.com");
    }
}
