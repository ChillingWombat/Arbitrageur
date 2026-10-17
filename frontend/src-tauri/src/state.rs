use crate::vault::ExchangeCredentials;
use arbitrageur_core::hub::MarketDataHub;
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::types::{dec, Decimal};
use arbitrageur_exchanges::alpaca::AlpacaAdapter;
use arbitrageur_exchanges::arcus::ArcusAdapter;
use arbitrageur_exchanges::aster::AsterAdapter;
use arbitrageur_exchanges::binance::BinanceAdapter;
use arbitrageur_exchanges::boros::{BorosAdapter, BorosYuMarket};
use arbitrageur_exchanges::coinbase::CoinbaseAdapter;
use arbitrageur_exchanges::decibel::DecibelAdapter;
use arbitrageur_exchanges::deribit::DeribitAdapter;
use arbitrageur_exchanges::dex_perps::{DexPerpAdapter, ADMITTED_DEX_PERP_VENUE_IDS};
use arbitrageur_exchanges::evm_amm::{
    EvmAmmAdapter, ADMITTED_EVM_AMM_VENUE_IDS, LIVE_EVM_AMM_VENUE_IDS,
};
use arbitrageur_exchanges::hyperliquid::HyperliquidAdapter;
use arbitrageur_exchanges::ibkr::IbkrAdapter;
use arbitrageur_exchanges::kraken::KrakenAdapter;
use arbitrageur_exchanges::lighter::LighterAdapter;
use arbitrageur_exchanges::okx::OkxAdapter;
use arbitrageur_exchanges::popdex::PopDexAdapter;
use arbitrageur_exchanges::vanta::VantaAdapter;
use arbitrageur_exchanges::variational::VariationalAdapter;
use arbitrageur_execution::coordinator::AtomicArbitrageCoordinator;
use arbitrageur_execution::passive_unwind::PassiveUnwindConfig;
use arbitrageur_execution::smart_routing::SmartOrderRouter;
use arbitrageur_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use arbitrageur_risk::delta_guard::DeltaGuard;
use arbitrageur_strategy::options_dispersion::LiveVolSurfaceCache;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroize;

pub use crate::state_store::ExecutionLogEntry;

/// Which venue endpoints the engine talks to. `Live` = production APIs with
/// real funds; `Testnet` = each venue's public testnet/demo API with a
/// separate credential set. There is no simulated environment — both modes
/// resolve real venue adapters, and a venue without a public testnet is
/// simply ineligible under `Testnet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradingEnvironment {
    Testnet,
    Live,
}

impl TradingEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Testnet => "testnet",
            Self::Live => "live",
        }
    }

    /// Persisted `trading_mode` value → environment. Accepts the retired
    /// `paper`/`real` spellings so pre-refactor stores migrate transparently;
    /// anything unrecognised fails safe to `Testnet`.
    pub fn from_mode(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "live" | "real" => Self::Live,
            _ => Self::Testnet,
        }
    }
}

/// Vault credentials resolved for one environment, extracted once then
/// applied to that environment's adapter set. Spec-driven venues keep their
/// full credential record (each adapter interprets fields per its own auth
/// model); named venues carry the already-shaped fields they consume.
#[derive(Default)]
struct EnvCredentials {
    aster: (Option<String>, Option<String>),
    hl_wallet: Option<String>,
    hl_pk: Option<String>,
    lighter_idx: Option<u64>,
    lighter_key: Option<String>,
    lighter_key_idx: Option<u8>,
    rh_idx: Option<u64>,
    rh_key: Option<String>,
    rh_key_idx: Option<u8>,
    vanta_acc: Option<String>,
    vanta_key: Option<String>,
    vanta_sec: Option<String>,
    arcus_key: Option<String>,
    arcus_sec: Option<String>,
    arcus_wallet: Option<String>,
    arcus_subaccount: Option<u64>,
    binance_key: Option<String>,
    binance_sec: Option<String>,
    okx_key: Option<String>,
    okx_sec: Option<String>,
    okx_pass: Option<String>,
    okx_url: Option<String>,
    cb_key: Option<String>,
    cb_sec: Option<String>,
    cb_pass: Option<String>,
    kraken_key: Option<String>,
    kraken_sec: Option<String>,
    alpaca_key: Option<String>,
    alpaca_sec: Option<String>,
    deribit_id: Option<String>,
    deribit_sec: Option<String>,
    decibel_key: Option<String>,
    decibel_node_key: Option<String>,
    decibel_wallet: Option<String>,
    decibel_pk: Option<String>,
    popdex_wallet: Option<String>,
    popdex_agent: Option<String>,
    ibkr_account: Option<String>,
    ibkr_url: Option<String>,
    dex_perp: std::collections::HashMap<&'static str, ExchangeCredentials>,
    evm_amm: std::collections::HashMap<&'static str, ExchangeCredentials>,
    /// Pendle Boros agent credentials: private_key = agent key,
    /// wallet_address = root account, account_id = sub-account.
    boros: Option<ExchangeCredentials>,
}

/// Every `Option<String>` field above is a copied secret (api keys,
/// secrets, passphrases, private keys, endpoint URLs that can embed
/// provider keys). Wipe them on drop — `ExchangeCredentials` values already
/// zeroize themselves. Credential sync rebuilds this struct on every pass;
/// without this, raw copies linger in freed heap until reuse.
impl Drop for EnvCredentials {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        fn z(field: &mut Option<String>) {
            if let Some(v) = field.as_mut() {
                v.zeroize();
            }
        }
        z(&mut self.aster.0);
        z(&mut self.aster.1);
        z(&mut self.hl_wallet);
        z(&mut self.hl_pk);
        z(&mut self.lighter_key);
        z(&mut self.rh_key);
        z(&mut self.vanta_acc);
        z(&mut self.vanta_key);
        z(&mut self.vanta_sec);
        z(&mut self.arcus_key);
        z(&mut self.arcus_sec);
        z(&mut self.arcus_wallet);
        z(&mut self.binance_key);
        z(&mut self.binance_sec);
        z(&mut self.okx_key);
        z(&mut self.okx_sec);
        z(&mut self.okx_pass);
        z(&mut self.okx_url);
        z(&mut self.cb_key);
        z(&mut self.cb_sec);
        z(&mut self.cb_pass);
        z(&mut self.kraken_key);
        z(&mut self.kraken_sec);
        z(&mut self.alpaca_key);
        z(&mut self.alpaca_sec);
        z(&mut self.deribit_id);
        z(&mut self.deribit_sec);
        z(&mut self.decibel_key);
        z(&mut self.decibel_node_key);
        z(&mut self.decibel_wallet);
        z(&mut self.decibel_pk);
        z(&mut self.popdex_wallet);
        z(&mut self.popdex_agent);
        z(&mut self.ibkr_account);
        z(&mut self.ibkr_url);
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TickerStrategyConfigDto {
    pub symbol: String,
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    /// Cap on concurrently-open matched pairs for this symbol — enforced at
    /// entry time against the durable signal ledger's `balanced` count.
    pub max_accumulated_units: u32,
    /// Legacy field: retained for serde compat with persisted configs. The
    /// authoritative open-pair count is derived from the signal ledger at
    /// gate time; this value is never read by execution paths.
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

/// Last-resort salvage for a persisted circuit-breaker snapshot whose
/// `Decimal` fields no longer deserialize (the serialization shape drifted
/// across versions). The scalar trip fields are format-stable: a stored
/// halt must survive an incompatible row — never silently re-armed.
/// Returns `None` when the row is not tripped or is not JSON at all.
fn salvage_tripped_snapshot(
    raw: &str,
) -> Option<arbitrageur_risk::circuit_breaker::CircuitBreakerSnapshot> {
    let v = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    if v.get("tripped").and_then(|t| t.as_bool()) != Some(true) {
        return None;
    }
    Some(arbitrageur_risk::circuit_breaker::CircuitBreakerSnapshot {
        tripped: true,
        trip_reason: v
            .get("trip_reason")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some("restored from unreadable snapshot".to_string())),
        tripped_at: v
            .get("tripped_at")
            .and_then(|t| t.as_str())
            .and_then(|t| t.parse::<chrono::DateTime<chrono::Utc>>().ok()),
        // Equity baselines are unknowable under schema drift; seeding
        // them at the construction default keeps the halt latched while
        // the next real equity update re-anchors the drawdown window.
        day_start_equity: dec!(200000.0),
        current_equity: dec!(200000.0),
        peak_equity: dec!(200000.0),
        day_key: Some(chrono::Utc::now().date_naive()),
        consecutive_errors: 0,
    })
}

/// Universal, strategy-scoped parameters — one record per strategy_id
/// ("futures" | "funding" | "spot"), shared across all tickers. Per-ticker
/// contract specifics (unit size in base units, accumulated grid units) stay in
/// TickerStrategyConfigDto keyed by symbol.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StrategyParamsDto {
    pub strategy_id: String,
    // Simple Spread (futures)
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    pub sliding_window: String,
    #[serde(default = "default_sliding_window_mode")]
    pub sliding_window_mode: String,
    #[serde(default = "default_sliding_window_secs")]
    pub sliding_window_secs: u64,
    /// Simple Spread sizing redesign: one grid step = min(mult ×
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
    // Funding auto-exit for spot/perp basis pairs: flatten when the live
    // perp-over-spot spread (percent, mid-based) falls below this value.
    // Applies only to pairs whose long leg is a `*-SPOT` instrument —
    // perp/perp carries have no price spread to converge. None disables.
    #[serde(default)]
    pub funding_auto_exit_spread_pct: Option<f64>,
    // Funding liquidation-distance trim: when EITHER leg comes within this
    // percent of liquidation, trim BOTH legs by the same qty to stay
    // neutral (close both when the remainder would drop below one step).
    // None disables the block. This is the funding instance of the
    // universal trim block (same semantics, shared helper).
    #[serde(default)]
    pub funding_liq_trim_pct: Option<f64>,
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
    // Per-strategy limit-exit selection relative to the global
    // `EngineSettingsDto.passive_unwind` policy:
    //   "inherit" → use the global policy (enabled or not);
    //   "custom"  → use this strategy's own PassiveUnwindConfig as-is;
    //   "off"     → never passive for this strategy — always market.
    // Empty (legacy rows saved before this field) auto-resolves:
    // an enabled strategy config means "custom", otherwise "inherit".
    #[serde(default)]
    pub futures_passive_mode: String,
    #[serde(default)]
    pub funding_passive_mode: String,
    #[serde(default)]
    pub spot_passive_mode: String,
}

impl StrategyParamsDto {
    pub fn defaults_for(strategy_id: &str) -> Self {
        Self {
            strategy_id: strategy_id.to_string(),
            target_profit_bps: 5.0,
            window_percentile: 80,
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
            funding_min_apr_pct: 25.0,
            funding_holding_days: 14,
            futures_max_slippage_bps: default_max_slippage(),
            funding_max_slippage_bps: default_max_slippage(),
            prediction_max_slippage_bps: default_max_slippage(),
            funding_max_entry_spread_bps: default_funding_max_entry_spread(),
            funding_auto_exit_spread_pct: None,
            funding_liq_trim_pct: None,
            funding_rotation_threshold_bps: None,
            funding_enable_basis: true,
            funding_min_basis_bps: None,
            futures_liq_trim_pct: None,
            spot_liq_trim_pct: None,
            prediction_liq_trim_pct: None,
            spot_trade_size_usd: 1000.0,
            spot_max_slippage_bps: 5.0,
            spot_min_net_spread_bps: 20.0,
            triangle_trade_size_usd: default_triangle_trade_size(),
            triangle_min_profit_bps: default_triangle_min_profit(),
            triangle_max_slippage_bps: default_max_slippage(),
            futures_passive_unwind: PassiveUnwindConfig::default(),
            funding_passive_unwind: PassiveUnwindConfig::default(),
            spot_passive_unwind: PassiveUnwindConfig::default(),
            futures_passive_mode: String::new(),
            funding_passive_mode: String::new(),
            spot_passive_mode: String::new(),
        }
    }
}

impl Default for TickerStrategyConfigDto {
    fn default() -> Self {
        Self {
            symbol: "BTC-USDC".into(),
            target_profit_bps: 5.0,
            window_percentile: 80,
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
    8.0
}
fn default_min_depth_ratio() -> f64 {
    3.0
}
fn default_usd_quote_mode() -> String {
    crate::stablecoin_fx::USD_QUOTE_MODE_FX_ADJUSTED.into()
}
fn default_stablecoin_warn_bps() -> f64 {
    50.0
}
fn default_stablecoin_halt_bps() -> f64 {
    100.0
}
fn default_triangle_trade_size() -> f64 {
    10000.0
}
fn default_triangle_min_profit() -> f64 {
    4.0
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
    "testnet".into()
}
fn default_fast_warmup_enabled() -> bool {
    true
}
fn default_cb_max_daily_drawdown_usd() -> f64 {
    25000.0
}
fn default_cb_max_daily_drawdown_pct() -> f64 {
    5.0
}
fn default_cb_max_consecutive_errors() -> u32 {
    5
}
fn default_cb_max_spread_divergence_bps() -> f64 {
    250.0
}
fn default_delta_max_unhedged_usd() -> f64 {
    2500.0
}
fn default_margin_warning_pct() -> f64 {
    70.0
}
fn default_margin_critical_pct() -> f64 {
    85.0
}
fn default_margin_min_liq_distance_pct() -> f64 {
    15.0
}
fn default_balance_stale_warn_secs() -> u64 {
    300
}
fn default_max_leverage() -> u32 {
    10
}
fn default_auto_fixed_yield_min_lock_days() -> u32 {
    7
}
fn default_auto_fixed_yield_min_gas_usd() -> f64 {
    25.0
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
    "https://github.com/ChillingWombat/Arbitrageur".into()
}
fn default_true() -> bool {
    true
}
/// The global passive-unwind policy defaults ON: per-strategy `custom`
/// configs keep `PassiveUnwindConfig::default()` (disabled) so the legacy
/// mode heuristic (`enabled` → "custom") still resolves correctly — only
/// this shared policy flips.
fn default_passive_unwind_policy() -> PassiveUnwindConfig {
    PassiveUnwindConfig {
        enabled: true,
        ..PassiveUnwindConfig::default()
    }
}

/// Overlay persisted execution-policy toggles onto `settings`. Operator
/// overrides (saved via Engine Settings) survive restarts; fail-closed —
/// an unreadable or invalid blob keeps the code defaults rather than
/// half-applying a corrupt record.
fn load_exec_policy_settings(
    state_store: &crate::state_store::StateStore,
    settings: &mut EngineSettingsDto,
) {
    if let Ok(Some(raw)) = state_store.kv_get("engine.passive_unwind") {
        match serde_json::from_str::<PassiveUnwindConfig>(&raw) {
            Ok(cfg) if cfg.validate().is_ok() => settings.passive_unwind = cfg,
            _ => {
                tracing::warn!("engine settings: ignoring invalid persisted passive_unwind blob")
            }
        }
    }
}
fn default_kraken_spot_url() -> String {
    "https://api.kraken.com".into()
}
fn default_kraken_futures_url() -> String {
    "https://futures.kraken.com".into()
}
fn default_arcus_url() -> String {
    "https://api.arcus.xyz".into()
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
    pub status: String, // "HEALTHY", "LOW_MARGIN", "DEPLETED", "EMPTY", "STALE", "LOCKED", "ERROR", "NO_CREDENTIALS"
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
    // Reserved — validated on save but no desktop listener consumes them.
    // The headless daemon (`arbitrageur serve`) takes its own --bind/--port
    // flags; these fields exist for a future embedded server surface.
    #[serde(default = "default_engine_rpc_port")]
    pub engine_rpc_port: u16,
    #[serde(default = "default_engine_ws_port")]
    pub engine_ws_port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,

    // Execution & Risk Modes
    #[serde(default = "default_trading_mode")]
    pub trading_mode: String, // "testnet" or "live" (legacy "paper"/"real" migrate on load)
    /// USD-family quote handling (System Settings → Execution):
    /// "separate" (BTC-USDT ≠ BTC-USDC, no merging), "parity" (merge,
    /// assume 1:1), or "fx_adjusted" (merge + convert quotes to USD via
    /// the live stablecoin rate table). Default fx_adjusted.
    #[serde(default = "default_usd_quote_mode")]
    pub usd_quote_mode: String,
    /// Stablecoin depeg warning threshold, |deviation| from $1.00 in bps.
    #[serde(default = "default_stablecoin_warn_bps")]
    pub stablecoin_warn_bps: f64,
    /// Stablecoin depeg halt threshold, bps — past this, pairs quoted in
    /// the coin are blocked (stablecoin-vs-USD books stay tradeable; that
    /// spread IS the depeg arbitrage).
    #[serde(default = "default_stablecoin_halt_bps")]
    pub stablecoin_halt_bps: f64,
    // Reserved — the warm-up cache reuse this describes is not implemented;
    // kept so saved settings stay schema-compatible.
    #[serde(default = "default_fast_warmup_enabled")]
    pub fast_warmup_enabled: bool,

    // Overall circuit-breaker + safety-net limits (new Safety settings section).
    // Live-applied on save: breaker limits update in place, delta guard
    // re-limits, margin thresholds feed the next risk snapshot.
    #[serde(default = "default_cb_max_daily_drawdown_usd")]
    pub cb_max_daily_drawdown_usd: f64,
    #[serde(default = "default_cb_max_daily_drawdown_pct")]
    pub cb_max_daily_drawdown_pct: f64,
    #[serde(default = "default_cb_max_consecutive_errors")]
    pub cb_max_consecutive_errors: u32,
    #[serde(default = "default_cb_max_spread_divergence_bps")]
    pub cb_max_spread_divergence_bps: f64,
    #[serde(default = "default_delta_max_unhedged_usd")]
    pub delta_max_unhedged_usd_per_asset: f64,
    #[serde(default = "default_margin_warning_pct")]
    pub margin_warning_usage_pct: f64,
    #[serde(default = "default_margin_critical_pct")]
    pub margin_critical_usage_pct: f64,
    #[serde(default = "default_margin_min_liq_distance_pct")]
    pub margin_min_liquidation_distance_pct: f64,
    /// Grace period before a repeatedly-unreadable venue balance escalates
    /// to a warning. Until then the last known-good figures keep totals and
    /// drawdown stable (a blip is not a loss).
    #[serde(default = "default_balance_stale_warn_secs")]
    pub balance_stale_warn_secs: u64,
    /// Global leverage ceiling (Risk Control tab). Every execution path
    /// takes min(strategy target, account max, venue max, this).
    #[serde(default = "default_max_leverage")]
    pub max_leverage: u32,
    /// Global Auto Position Exit policy (System Settings → Execution):
    /// the single shared limit-exit configuration. Strategies with
    /// `*_passive_mode = "inherit"` (the default) use this record; "custom"
    /// uses the strategy's own params; "off" always market-exits. The
    /// popup's Auto Limit mode also runs this policy. Enabled by default —
    /// the calm gates (spread/fast-vol ceilings) and try-count fallback are
    /// what decide limit vs market per attempt, not the toggle.
    #[serde(default = "default_passive_unwind_policy")]
    pub passive_unwind: PassiveUnwindConfig,
    /// Opt-in: allow AUTONOMOUS fixed-yield entries (Boros YU locks on
    /// Arbitrum mainnet). Default OFF — a live four-leg entry that locks
    /// collateral to maturity must never fire from an implicit default.
    /// Manual Execute stays available regardless; this flag only admits
    /// the `auto` path, which additionally gates on agent-approval
    /// validity, gas treasury, Boros margin, lock term, and post-cost
    /// return before dispatch.
    #[serde(default)]
    pub auto_fixed_yield_enabled: bool,
    /// Autonomous fixed-yield entries require at least this lock term —
    /// a sub-week lock's entry/exit costs dominate the carry.
    #[serde(default = "default_auto_fixed_yield_min_lock_days")]
    pub auto_fixed_yield_min_lock_days: u32,
    /// Autonomous fixed-yield entries require the Boros gas treasury to
    /// hold at least this USD balance — the venue sponsors tx gas from it.
    #[serde(default = "default_auto_fixed_yield_min_gas_usd")]
    pub auto_fixed_yield_min_gas_usd: f64,

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
    pub popdex_enabled: bool,
    #[serde(default)]
    pub coinbase_enabled: bool,
    /// Alpaca — US equities & crypto spot broker (live + real paper env).
    #[serde(default)]
    pub alpaca_enabled: bool,
    /// Deribit crypto options/perpetuals (BTC + ETH). Off by default — the
    /// options parity strategy is the only consumer.
    #[serde(default)]
    pub deribit_enabled: bool,

    // Deribit (OAuth2 client_credentials pair)
    #[serde(default)]
    pub deribit_client_id: String,
    #[serde(default)]
    pub deribit_client_secret: String,

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
    // Reserved — no code path reads a Robinhood chain RPC today; the
    // Robinhood venue adapter speaks the Lighter REST protocol instead.
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
            usd_quote_mode: default_usd_quote_mode(),
            stablecoin_warn_bps: default_stablecoin_warn_bps(),
            stablecoin_halt_bps: default_stablecoin_halt_bps(),
            fast_warmup_enabled: default_fast_warmup_enabled(),
            cb_max_daily_drawdown_usd: default_cb_max_daily_drawdown_usd(),
            cb_max_daily_drawdown_pct: default_cb_max_daily_drawdown_pct(),
            cb_max_consecutive_errors: default_cb_max_consecutive_errors(),
            cb_max_spread_divergence_bps: default_cb_max_spread_divergence_bps(),
            delta_max_unhedged_usd_per_asset: default_delta_max_unhedged_usd(),
            margin_warning_usage_pct: default_margin_warning_pct(),
            margin_critical_usage_pct: default_margin_critical_pct(),
            margin_min_liquidation_distance_pct: default_margin_min_liq_distance_pct(),
            balance_stale_warn_secs: default_balance_stale_warn_secs(),
            max_leverage: default_max_leverage(),
            passive_unwind: default_passive_unwind_policy(),
            auto_fixed_yield_enabled: false,
            auto_fixed_yield_min_lock_days: default_auto_fixed_yield_min_lock_days(),
            auto_fixed_yield_min_gas_usd: default_auto_fixed_yield_min_gas_usd(),
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
            popdex_enabled: false,
            coinbase_enabled: false,
            alpaca_enabled: false,
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
            deribit_enabled: false,
            deribit_client_id: "".into(),
            deribit_client_secret: "".into(),
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
            // 14-day raw telemetry retention by default — the DB grew
            // unbounded when this defaulted to None. Row caps still apply
            // unconditionally for operators who set the window to 0/None.
            spread_samples_max_days: Some(14),
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
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
                base_url: None,
                enabled: self.kraken_enabled && self.kraken_futures_enabled,
            });
        }

        if has_secret_value(&self.deribit_client_id)
            || has_secret_value(&self.deribit_client_secret)
        {
            out.push(ExchangeCredentials {
                venue: "deribit".into(),
                profile_id: None,
                api_key: self.deribit_client_id.trim().to_string(),
                api_secret: self.deribit_client_secret.trim().to_string(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,
                enabled: self.deribit_enabled,
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
                base_url: None,
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
                base_url: None,
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
        self.deribit_client_id.zeroize();
        self.deribit_client_secret.zeroize();
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

/// Cached venue market listings, keyed `{env}-{venue}` so a live/testnet
/// switch can never serve the other environment's instruments. Scan and
/// filter surfaces read this instead of fanning `fetch_markets` out on
/// every UI poll — listings are slow-moving metadata.
type MarketsCache = Mutex<
    std::collections::HashMap<
        String,
        (
            std::time::Instant,
            Vec<arbitrageur_core::market::MarketMetadata>,
        ),
    >,
>;

/// Cached venue mark prices (`fetch_market_prices`), same keying and
/// staleness contract as [`MarketsCache`].
type MarketPricesCache = Mutex<
    std::collections::HashMap<
        String,
        (
            std::time::Instant,
            std::collections::HashMap<String, Decimal>,
        ),
    >,
>;

/// Live telemetry for one supervised WS feed lane — written by feed.rs
/// supervisors, surfaced read-only on `get_system_status`. All timestamps
/// are wall-clock ms since epoch so the DTO maps field-for-field.
#[derive(Clone, Debug, Default)]
pub struct FeedLaneTelemetry {
    /// Total connection attempts since process start.
    pub attempts: u64,
    /// Consecutive ended/failed connection cycles (the supervisor's
    /// fail_streak, capped at its backoff depth).
    pub consecutive_failures: u32,
    /// Consecutive cycles that died inside 5s — the venue-side rejection
    /// signature that drives the extended ~5min backoff.
    pub instant_kills: u32,
    /// When the lane's socket last opened. `None` = never connected this
    /// boot — never fabricated.
    pub last_connected_ms: Option<i64>,
    /// When the lane last ended or failed.
    pub last_ended_ms: Option<i64>,
    /// Next scheduled attempt while backing off. `None` when not sleeping.
    pub backoff_until_ms: Option<i64>,
    /// Last connection-cycle error string; cleared on a clean end.
    pub last_error: Option<String>,
    /// Current phase: `connecting` | `connected` | `backoff` | `disabled`
    /// | `not-admitted` | `paused` | `stopped`.
    pub phase: String,
}

#[allow(dead_code)]
/// Cached Boros market-data snapshot (public REST, no credentials).
#[derive(Clone)]
pub struct BorosYuCache {
    pub fetched_at: std::time::Instant,
    pub markets: Arc<Vec<BorosYuMarket>>,
    /// Estimated gas cost in USD for the two YU order transactions
    /// (from `/v1/gas-price/current`, fallback ~$0.10).
    pub gas_usd: f64,
}

pub struct AppEngineState {
    pub live_hl: Arc<HyperliquidAdapter>,
    pub live_lighter: Arc<LighterAdapter>,
    pub live_vanta: Arc<VantaAdapter>,
    pub live_robinhood: Arc<LighterAdapter>,
    pub live_aster: Arc<AsterAdapter>,
    pub live_popdex: Arc<PopDexAdapter>,
    pub live_binance: Arc<BinanceAdapter>,
    pub live_okx: Arc<OkxAdapter>,
    pub live_coinbase: Arc<CoinbaseAdapter>,
    pub live_kraken: Arc<KrakenAdapter>,
    /// Alpaca — US equities + crypto spot broker (REST). Spot-only: no
    /// funding streams, no perp positions.
    pub live_alpaca: Arc<AlpacaAdapter>,
    /// Interactive Brokers (IBKR) — Client Portal Web API / TWS Gateway (live).
    pub live_ibkr: Arc<IbkrAdapter>,
    pub live_deribit: Arc<DeribitAdapter>,
    pub live_decibel: Arc<DecibelAdapter>,
    // Long-tail perp venues (one shared adapter type, per-venue specs).
    pub live_dex_perps: Vec<Arc<DexPerpAdapter>>,
    /// Generic EVM spot-AMM venues (Uniswap/PancakeSwap/Aerodrome family).
    /// Spec-driven like `live_dex_perps`; `EvmAmmAdapter::for_venue` takes a
    /// per-account RPC override via `set_base_url` during credential sync.
    pub live_evm_amms: Vec<Arc<EvmAmmAdapter>>,
    /// Pendle Boros (Arbitrum rate swaps) — the YU hedge venue for
    /// fixed-yield carries. Live-only: Boros has no testnet.
    pub live_boros: Arc<BorosAdapter>,
    /// Variational Omni (Arbitrum peer-to-pool perps) — market data only:
    /// its public API is a stats document; the trading API is not yet
    /// public. Live-only — no demo environment exists.
    pub live_variational: Arc<VariationalAdapter>,
    /// Arcus perp CLOB — market data only (public `/v1` reads; order
    /// routing needs Ed25519 signing the adapter does not yet implement,
    /// so order paths fail closed). Both environments use the same API
    /// surface on their own hosts.
    pub live_arcus: Arc<ArcusAdapter>,
    /// Testnet adapter set — `None` entries mean the venue has no public
    /// testnet and is ineligible under `TradingEnvironment::Testnet`.
    /// Endpoint overrides (`endpoints.*`) apply to the live set only;
    /// testnet adapters always use the built-in testnet defaults.
    pub tn_hl: Arc<HyperliquidAdapter>,
    pub tn_lighter: Arc<LighterAdapter>,
    pub tn_vanta: Arc<VantaAdapter>,
    pub tn_binance: Arc<BinanceAdapter>,
    pub tn_okx: Arc<OkxAdapter>,
    /// Alpaca paper-trading environment (paper-api.alpaca.markets) — a real
    /// venue sandbox, not a local simulation.
    pub tn_alpaca: Arc<AlpacaAdapter>,
    /// Interactive Brokers (IBKR) — paper-trading on local gateway (DU account).
    pub tn_ibkr: Arc<IbkrAdapter>,
    pub tn_deribit: Arc<DeribitAdapter>,
    pub tn_decibel: Arc<DecibelAdapter>,
    pub tn_popdex: Arc<PopDexAdapter>,
    /// Robinhood Chain's testnet Lighter deployment — a real demo venue
    /// (api.rh-testnet.lighter.xyz, signing chain id 300 shared with
    /// Lighter's own testnet).
    pub tn_robinhood: Arc<LighterAdapter>,
    /// Arcus testnet — same public `/v1` surface on the demo host.
    pub tn_arcus: Arc<ArcusAdapter>,
    pub market_data_hub: Arc<MarketDataHub>,
    /// Cached Boros YU market snapshot for the fixed-yield scan:
    /// (fetched_at, markets, est_gas_usd_per_leg_batch). Public data —
    /// valid in both environments since Boros has no testnet split.
    pub boros_yu_cache: Mutex<Option<BorosYuCache>>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub delta_guard: RwLock<DeltaGuard>,
    pub coordinator: Arc<AtomicArbitrageCoordinator>,
    pub kalman_filter: RwLock<HedgeRatioKalmanFilter>,
    /// Active venue environment — `Live` production endpoints or `Testnet`
    /// demo endpoints. Drives adapter selection in `get_venues`, account
    /// eligibility, and the `is_live` flag on persisted records (testnet
    /// fills are real venue flow but not real money).
    /// Shared so the circuit-breaker trip hook (registered before the
    /// struct exists) reports the *current* environment in alerts.
    pub environment: Arc<RwLock<TradingEnvironment>>,
    /// Which equity universe the breaker baseline tracks (None = not yet
    /// anchored). Paper<->live switches change the account universe, which is
    /// not a trading loss — the next risk snapshot re-anchors instead of
    /// tripping. Prevents the classic false halt: $200k baseline vs $3.5k
    /// live funds reading as a -$196k "drawdown".
    pub last_risk_live_universe: RwLock<Option<bool>>,
    /// Which set of supervised accounts the breaker baseline was anchored
    /// for (None = not yet anchored). Enabling/disabling a venue, or a
    /// venue losing its credentials, changes the accounted equity total
    /// without any trading loss: the next risk snapshot must re-anchor
    /// rather than read the step as drawdown and halt everything.
    pub last_risk_account_set: RwLock<Option<std::collections::HashSet<String>>>,
    /// Last known-good live account snapshot per venue, with read timestamp.
    /// A transiently-unreadable venue keeps contributing its cached figures
    /// to totals/drawdown (a blip is not a loss) until the Safety-tab grace
    /// period expires, after which it escalates to ERROR.
    pub last_good_accounts: Mutex<std::collections::HashMap<String, (AccountSnapshot, i64)>>,
    /// Per-venue last-observed equity in USD basis, keyed `{env}-{venue}` —
    /// the portfolio-equity input to the drawdown breaker. Updated by every
    /// lane that reads an account (risk overview, margin poller) so the
    /// breaker is fed even while nobody polls the UI.
    pub breaker_venue_equity: Mutex<std::collections::HashMap<String, Decimal>>,
    /// Frozen-FX anchor for the drawdown breaker: non-USD balances are
    /// revalued at the UTC-day-start FX rather than the live price, so a
    /// coin-margined account's USD-equity swing from index beta alone does
    /// not read as a trading drawdown. Coin-denominated PnL lands inside
    /// equity itself and still moves the measure. Persisted in kv
    /// `cb.fx_anchor`; rolls on the same UTC-day boundary as the breaker's
    /// own day_key.
    pub breaker_fx_anchor: Mutex<crate::execution::BreakerFxAnchor>,
    /// Latest per-account collateral-FX adjustment applied to the breaker
    /// feed, keyed `{env}-{account}` like `breaker_venue_equity`. Surfaced
    /// on the risk overview so the operator can see how much of the raw
    /// equity move was index repricing rather than trading PnL.
    pub breaker_fx_adj: Mutex<std::collections::HashMap<String, Decimal>>,
    /// Last-observed USD FX per `{env}-{venue}|{ccy}` for non-USD account
    /// balances — the per-poll counterpart of `breaker_fx_anchor`. The
    /// equity-attribution checker uses it to estimate how much of an
    /// equity delta is collateral repricing (vs a real external flow);
    /// persisted (`equity.fx_prev`) so a restart's first poll still
    /// attributes instead of emitting one misclassified event.
    pub equity_fx_prev: Mutex<std::collections::HashMap<String, Decimal>>,
    /// Last-observed Σ position unrealized-PnL (USD) per `{env}-{venue}` —
    /// the mark-to-market counterpart of `equity_fx_prev`. Coin-margined
    /// venues holding option books swing equity by thousands of dollars on
    /// mark noise alone; tracking ΔuPnL lets the attribution checker explain
    /// that component instead of misclassifying it as an external flow.
    /// Persisted (`equity.upnl_prev`). Option positions are excluded — their
    /// equity contribution is mark VALUE, not uPnL; `equity_optval_prev`
    /// tracks `qty × mark` per option leg instead.
    pub equity_upnl_prev: Mutex<std::collections::HashMap<String, f64>>,
    /// Last-observed mark value (USD) per option position,
    /// `{env}-{venue}|{symbol}|{side} -> qty × mark`. Option equity
    /// contribution is the position's mark value — it reprices every poll
    /// without necessarily moving `floating_profit_loss`, so it needs its
    /// own tracker to attribute option-book mark noise. Persisted
    /// (`equity.optval_prev`).
    pub equity_optval_prev: Mutex<std::collections::HashMap<String, f64>>,
    /// Last-observed balance per `{env}-{venue}|{ccy}` (native units).
    /// Diagnostic only — logged as `dbal` next to `explained` so an
    /// unexplained equity delta can be read as a real balance flow
    /// (funding/settlement → deposit/withdrawal is honest) vs a valuation
    /// term the estimator still misses. Persisted (`equity.bal_prev`).
    pub equity_bal_prev: Mutex<std::collections::HashMap<String, f64>>,
    /// Serializes the equity-attribution check: `get_venue_balances` is
    /// called from several pollers, and two concurrent passes read the
    /// same baseline and double-record one delta as two events.
    pub equity_check_lock: Mutex<()>,
    /// Last options-bot tick per substrategy — mode, open cycles, scan
    /// counts. Read by `options_bot_status`; runtime-only.
    pub options_tick_status:
        Mutex<std::collections::HashMap<String, crate::options_bot::OptionsBotTickStatus>>,
    /// Live telemetry for each supervised WS feed lane, keyed by venue.
    /// Written by feed.rs supervisors; surfaced read-only on
    /// `get_system_status` so a silently-dead lane is observable instead of
    /// just aging out of the hub.
    pub feed_lanes: Mutex<std::collections::BTreeMap<String, FeedLaneTelemetry>>,
    pub total_realized_profit: RwLock<Decimal>,
    pub total_realized_profit_live: RwLock<Decimal>,
    /// Cumulative option premium cashflow (bought − sold at entry) per env.
    /// Kept out of `total_realized_profit`: premium paid at entry is
    /// financing that converts into leg PnL only at close/expiry.
    pub total_premium_outlay: RwLock<Decimal>,
    pub total_premium_outlay_live: RwLock<Decimal>,
    pub execution_logs: RwLock<Vec<ExecutionLogEntry>>,
    pub sim_step_counter: RwLock<usize>,
    pub settings: RwLock<EngineSettingsDto>,
    pub strategy_configs: RwLock<std::collections::HashMap<String, TickerStrategyConfigDto>>,
    /// Universal per-strategy parameters keyed by strategy_id ("futures" |
    /// "funding" | "spot"). These are the canonical strategy settings — the
    /// per-symbol strategy_configs map holds ticker-level caps and runtime
    /// state, persisted under `ticker_config.{symbol}` kv keys.
    pub strategy_params: RwLock<std::collections::HashMap<String, StrategyParamsDto>>,
    pub vault: RwLock<crate::vault::SecureVault>,
    /// Stronghold custody for the shared EVM wallet key — `Some` only
    /// while the vault is unlocked and the snapshot opened. Provisioned
    /// venue signers call into it for `secp256k1` signatures; the key
    /// itself never enters this process's heap. Dropped on manual lock
    /// (signers get wiped by the credential sync anyway); auto-lock keeps
    /// it loaded so unattended trading keeps signing, matching the
    /// "provisioned credentials survive auto-lock" contract.
    pub evm_stronghold: RwLock<Option<crate::stronghold::EvmStronghold>>,
    pub state_store: Arc<crate::state_store::StateStore>,
    pub virtual_ledger: RwLock<arbitrageur_execution::virtual_ledger::VirtualStrategyLedger>,
    pub margin_rebalance_daemon:
        RwLock<arbitrageur_risk::collateral_rebalance::MarginRebalanceDaemonState>,
    pub is_paused: RwLock<bool>,
    pub is_stopped: RwLock<bool>,
    pub execution_lock: tokio::sync::Mutex<()>,
    /// Serializes reconciliation passes so the periodic supervisor and any
    /// manual trigger never run overlapping venue polls.
    pub reconcile_lock: tokio::sync::Mutex<()>,
    /// Test-only adapter registry consulted before `get_venues` — lets
    /// reconciliation tests inject scripted venue responses without
    /// touching the live adapter fields.
    #[cfg(test)]
    pub test_adapters: std::sync::RwLock<
        std::collections::HashMap<String, Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>>,
    >,
    /// Test-only simulated-venue registry: concrete `SimulatedExchange`
    /// handles for venues whose hub-seeded books must stay reachable through
    /// the adapter layer once their freshness window lapses. `get_venues`
    /// prefers these over the live adapter fields under cfg(test), so scans
    /// and dispatch paths never touch the network in tests.
    /// Test-only concrete Deribit override — options parity needs the
    /// concrete type (option chain, book summaries), so a stub-backed
    /// adapter is injected here rather than through the dyn registry.
    #[cfg(test)]
    pub test_deribit: std::sync::RwLock<Option<Arc<DeribitAdapter>>>,
    /// Test-only concrete Boros override — the fixed-yield executor calls
    /// `live_boros.market_by_id` (concrete) plus dyn order methods, so a
    /// stub-backed `BorosAdapter` is injected here rather than through the
    /// dyn registry. `get_adapter("boros")` admission still flows through
    /// `test_sims`/`test_adapters` like every other venue.
    #[cfg(test)]
    pub test_boros: std::sync::RwLock<Option<Arc<BorosAdapter>>>,
    #[cfg(test)]
    pub test_sims: std::sync::RwLock<
        std::collections::HashMap<String, Arc<arbitrageur_exchanges::simulated::SimulatedExchange>>,
    >,
    pub autotune_apply_lock: tokio::sync::Mutex<()>,
    /// Legs (`{signal_id}:{side}`) whose terminal fill has already had its
    /// one-shot fee-enrichment `fetch_order` this process — submit-response
    /// fills carry `fee_usd = 0` because venue acks never report fees, and
    /// each unpriced leg gets exactly one poll per boot.
    pub fee_enrich_done: Mutex<std::collections::HashSet<String>>,
    /// Epoch hour of the last spread_samples prune pass.
    pub sample_prune_at: Mutex<i64>,
    /// Last liquidation-trim dispatch per signal id — cooldown so the 5s
    /// supervisor does not bleed a pair step-by-step within one minute.
    pub liq_trim_at: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// Throttles the vault-locked risk-overview pause warnings — one per
    /// 5 min while locked instead of one per venue per poll cycle.
    pub risk_locked_warn_at: Mutex<Option<std::time::Instant>>,
    /// Per-venue throttle for stale-figure warnings in the risk overview:
    /// a sustained venue outage is one anomaly, not one per poll tick.
    pub risk_stale_warn_at: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// Failed vault-unlock attempts and the next permitted try. Exponential
    /// backoff throttles online password guessing (the KDF still bounds
    /// offline attempts); in-memory is sufficient — a restart resets the
    /// counter but the KDF cost floor remains.
    pub vault_unlock_backoff: Mutex<(u32, std::time::Instant)>,
    /// In-flight MFA enrollment secret, held only between `begin` and
    /// `confirm` calls. Never persisted — a lock or restart drops it and
    /// enrollment restarts, so a half-configured factor can't wedge the
    /// vault in a state the operator can't reproduce.
    pub mfa_pending: Mutex<Option<zeroize::Zeroizing<Vec<u8>>>>,
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
    /// Market listings cache (`{env}-{venue}` key). Poll-frequency callers
    /// must go through `cached_markets` — a direct `fetch_markets` per scan
    /// tick turns the UI refresh loop into a per-venue request storm.
    pub markets_cache: MarketsCache,
    /// Venue mark-price cache (`{env}-{venue}` key) via `cached_market_prices`.
    pub market_prices_cache: MarketPricesCache,
    /// Last-failure timestamps for cached venue reads, keyed
    /// `{env}-{venue}-{kind}`. A failed fetch used to retry on every poll
    /// tick (a guaranteed-fail venue got hammered at UI cadence); failures
    /// cool down briefly so a recovering venue is retried, not hammered.
    pub venue_fetch_failures: Mutex<std::collections::HashMap<String, std::time::Instant>>,
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
    /// Per-venue async fetch lock for live account reads — concurrent
    /// pollers (venue balances, risk overview, external positions) share
    /// one REST call instead of each hammering e.g. Orderly's
    /// /v1/positions (429s at a few req/3s).
    pub account_fetch_locks: Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// USD-family stablecoin rate table + depeg monitor. Drives the
    /// `fx_adjusted` quote mode: price conversion into USD, watchlist
    /// pairing, sizing, and the halt gate on depegged quote currencies.
    pub stablecoin_fx: Arc<crate::stablecoin_fx::StablecoinFx>,
    /// Live continuous volatility surface cache & online SABR parameter calibrator.
    /// Ingests live Deribit / OKX / Binance options surface ticks and provides
    /// analytical SABR smiles + Greeks to DeltaGuard and OptionsBot.
    pub vol_surface: Arc<LiveVolSurfaceCache>,
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
        "aster" | "ast" | "asterdex" | "aster (perp dex)" | "aster (perp dex, read-only)" => {
            Some("aster")
        }
        "popdex" | "pop" => Some("popdex"),
        "arcus" | "arc" | "arcus dex" | "arcus (fast clob)" | "arcus (perp dex)" => Some("arcus"),
        "binance" | "bnb" | "bin" | "binance futures" | "binance (perp)" => Some("binance"),
        "okx" | "okx swaps" | "okx (perp)" => Some("okx"),
        "coinbase" | "cb" | "coinbase exchange" => Some("coinbase"),
        "kraken" | "krk" | "kfut" | "kraken futures" | "kraken spot" => Some("kraken"),
        "alpaca" | "alp" | "alpaca markets" | "alpaca crypto" => Some("alpaca"),
        "ibkr" | "ib" | "interactive brokers" | "interactive_brokers" => Some("ibkr"),
        "deribit" | "dbit" | "deribit options" => Some("deribit"),
        "decibel" | "dcb" | "decibel (aptos)" => Some("decibel"),
        // Long-tail perp venues: common display/alias spellings first, then
        // every canonical spec id resolves to itself.
        "edge x" | "edgex exchange" | "edgex perps" => Some("edgex"),
        "ondo" | "ondo perpetuals" | "ondo finance" => Some("ondoperps"),
        "gravity" | "grvt exchange" => Some("grvt"),
        "lyra" | "derive (lyra)" => Some("derive"),
        "apex omni" | "apex pro" => Some("apex"),
        "x10" | "extended exchange" => Some("extended"),
        "dydx chain" | "dydx v4" => Some("dydx"),
        "helix (injective)" | "injective helix" => Some("helix"),
        "boros" | "pendle boros" | "pendle" | "boros (pendle)" | "boros (arbitrum)" => {
            Some("boros")
        }
        "variational" | "var" | "variational omni" | "omni" | "variational (omni)" => {
            Some("variational")
        }
        "alpha x" => Some("alphax"),
        "antarctic exchange" => Some("antarctic"),
        "eve dex" => Some("evedex"),
        "gate dex" => Some("gatedex"),
        "io trader" => Some("iotrader"),
        // EVM spot-AMM venues: display aliases, then canonical ids.
        "uniswap v3" | "uni v3" | "uniswap v3 (ethereum)" => Some("uniswap"),
        "uniswap v2" | "uni v2" | "uniswap v2 (ethereum)" => Some("uniswap_v2"),
        "pancakeswap v2" | "pcs v2" | "pancakeswap (v2)" => Some("pancakeswap"),
        "pancakeswap v3" | "pcs v3" | "pancakeswap (v3)" => Some("pancakeswap_v3"),
        "sushi" | "sushiswap v2" => Some("sushiswap"),
        "quickswap v2" | "quickswap (polygon)" => Some("quickswap"),
        "camelot v2" | "camelot dex" => Some("camelot"),
        "aerodrome finance" | "aerodrome slipstream" => Some("aerodrome"),
        "velodrome v2" | "velodrome finance" => Some("velodrome"),
        "ramses exchange" | "ramses v2" => Some("ramses"),
        "pharaoh exchange" | "pharaoh v2" => Some("pharaoh"),
        id if ADMITTED_DEX_PERP_VENUE_IDS.contains(&id) => ADMITTED_DEX_PERP_VENUE_IDS
            .iter()
            .copied()
            .find(|v| *v == id),
        id if ADMITTED_EVM_AMM_VENUE_IDS.contains(&id) => ADMITTED_EVM_AMM_VENUE_IDS
            .iter()
            .copied()
            .find(|v| *v == id),
        _ => None,
    }
}

/// Whether `AppEngineState` constructs a real adapter for `venue_id` in the
/// active environment — independent of enablement. Named adapter fields are
/// listed explicitly; the spec-driven DexPerp/EVM-AMM families delegate to
/// their admitted rosters. A canonical venue id that fails this check can
/// never be admitted by `get_venues`: its status is UNSUPPORTED regardless
/// of flags, and credential material stored for it provisions nothing.
///
/// Drift discipline: adding a `live_*`/`tn_*` field without adding its id
/// here makes the venue read UNSUPPORTED — visible, fail-closed. The
/// reverse (an id here with no constructed adapter) is caught by the
/// venue-status regression test, which asserts every UNADAPTED-status row
/// has no adapter.
pub(crate) fn venue_has_adapter(venue_id: &str) -> bool {
    let Some(vid) = canonical_venue_id(venue_id) else {
        return false;
    };
    matches!(
        vid,
        "hyperliquid"
            | "lighter"
            | "robinhood"
            | "vanta"
            | "arcus"
            | "aster"
            | "popdex"
            | "binance"
            | "okx"
            | "coinbase"
            | "kraken"
            | "alpaca"
            | "ibkr"
            | "deribit"
            | "decibel"
            | "boros"
            | "variational"
    ) || ADMITTED_DEX_PERP_VENUE_IDS.contains(&vid)
        || ADMITTED_EVM_AMM_VENUE_IDS.contains(&vid)
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

/// Restore per-ticker strategy configs saved under `ticker_config.{symbol}`
/// kv keys. Rows that fail to parse, or whose embedded symbol disagrees with
/// the key, are skipped rather than failing boot — a corrupt config must not
/// strand the engine.
pub(crate) fn load_ticker_strategy_configs(
    store: &crate::state_store::StateStore,
) -> std::collections::HashMap<String, TickerStrategyConfigDto> {
    let mut map = std::collections::HashMap::new();
    if let Ok(keys) = store.kv_keys_with_prefix("ticker_config.") {
        for key in keys {
            let Some(raw) = store.kv_get(&key).ok().flatten() else {
                continue;
            };
            let Ok(cfg) = serde_json::from_str::<TickerStrategyConfigDto>(&raw) else {
                continue;
            };
            if cfg.symbol.is_empty() || cfg.max_accumulated_units < 1 {
                continue;
            }
            map.insert(cfg.symbol.clone(), cfg);
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
        // No synthetic market seeding. All market quotes and orderbooks are fetched dynamically
        // from live venue APIs once API credentials are configured by the user.

        let mut initial_settings = EngineSettingsDto::default();
        if let Ok(Some(mode)) = state_store.kv_get("engine.trading_mode") {
            // Retired spellings migrate transparently: paper→testnet, real→live.
            let normalized = TradingEnvironment::from_mode(&mode).as_str();
            initial_settings.trading_mode = normalized.to_string();
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
        // Overall circuit-breaker + safety-net limits persist in kv so a
        // restart keeps the operator's Safety tab values.
        {
            let get_f = |k: &str| {
                state_store
                    .kv_get(k)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<f64>().ok())
            };
            let get_u = |k: &str| {
                state_store
                    .kv_get(k)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<u32>().ok())
            };
            if let Some(v) = get_f("risk.cb_max_dd_usd") {
                if v.is_finite() && v > 0.0 && v <= 1_000_000_000_000.0 {
                    initial_settings.cb_max_daily_drawdown_usd = v;
                }
            }
            if let Some(v) = get_f("risk.cb_max_dd_pct") {
                if v.is_finite() && v > 0.0 && v <= 100.0 {
                    initial_settings.cb_max_daily_drawdown_pct = v;
                }
            }
            if let Some(v) = get_u("risk.cb_max_errors") {
                if (1..=1000).contains(&v) {
                    initial_settings.cb_max_consecutive_errors = v;
                }
            }
            if let Some(v) = get_f("risk.cb_max_spread_bps") {
                if v.is_finite() && v > 0.0 && v <= 100_000.0 {
                    initial_settings.cb_max_spread_divergence_bps = v;
                }
            }
            if let Some(v) = get_f("risk.delta_max_unhedged") {
                if v.is_finite() && v > 0.0 && v <= 1_000_000_000.0 {
                    initial_settings.delta_max_unhedged_usd_per_asset = v;
                }
            }
            if let Some(v) = get_f("risk.margin_warn") {
                if v.is_finite() && (0.0..100.0).contains(&v) {
                    initial_settings.margin_warning_usage_pct = v;
                }
            }
            if let Some(v) = get_f("risk.margin_crit") {
                if v.is_finite() && v > 0.0 && v <= 100.0 {
                    initial_settings.margin_critical_usage_pct = v;
                }
            }
            if let Some(v) = get_f("risk.margin_min_liq") {
                if v.is_finite() && v > 0.0 && v <= 100.0 {
                    initial_settings.margin_min_liquidation_distance_pct = v;
                }
            }
            if let Some(v) = get_u("risk.stale_warn_secs") {
                if (30..=3600).contains(&v) {
                    initial_settings.balance_stale_warn_secs = v as u64;
                }
            }
            if let Some(v) = get_u("risk.max_leverage") {
                if (1..=100).contains(&v) {
                    initial_settings.max_leverage = v;
                }
            }
            // Autonomous fixed-yield opt-in + its safety bounds.
            if let Ok(Some(v)) = state_store.kv_get("engine.auto_fixed_yield_enabled") {
                initial_settings.auto_fixed_yield_enabled = v == "1";
            }
            if let Some(v) = get_u("engine.auto_fixed_yield_min_lock_days") {
                if (1..=365).contains(&v) {
                    initial_settings.auto_fixed_yield_min_lock_days = v;
                }
            }
            if let Some(v) = get_f("engine.auto_fixed_yield_min_gas_usd") {
                if v.is_finite() && (0.0..=10_000.0).contains(&v) {
                    initial_settings.auto_fixed_yield_min_gas_usd = v;
                }
            }
            // USD-quote policy + depeg thresholds (Execution tab). Unknown
            // mode spellings and inverted/degenerate thresholds fall back
            // to code defaults rather than persist a corrupt policy.
            if let Ok(Some(mode)) = state_store.kv_get("engine.usd_quote_mode") {
                let m = mode.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "separate" | "parity" | "fx_adjusted") {
                    initial_settings.usd_quote_mode = m;
                }
            }
            if let Some(v) = get_f("risk.stablecoin_warn_bps") {
                if v.is_finite() && v > 0.0 && v <= 10_000.0 {
                    initial_settings.stablecoin_warn_bps = v;
                }
            }
            if let Some(v) = get_f("risk.stablecoin_halt_bps") {
                if v.is_finite() && v > 0.0 && v <= 10_000.0 {
                    initial_settings.stablecoin_halt_bps = v;
                }
            }
            if initial_settings.stablecoin_warn_bps >= initial_settings.stablecoin_halt_bps {
                initial_settings.stablecoin_warn_bps = default_stablecoin_warn_bps();
                initial_settings.stablecoin_halt_bps = default_stablecoin_halt_bps();
            }
            for (key, slot) in [
                (
                    "endpoints.hyperliquid_url",
                    &mut initial_settings.hyperliquid_url,
                ),
                ("endpoints.lighter_url", &mut initial_settings.lighter_url),
                ("endpoints.vanta_url", &mut initial_settings.vanta_url),
                (
                    "endpoints.kraken_spot_url",
                    &mut initial_settings.kraken_spot_url,
                ),
                (
                    "endpoints.kraken_futures_url",
                    &mut initial_settings.kraken_futures_url,
                ),
                ("endpoints.arcus_url", &mut initial_settings.arcus_url),
                (
                    "endpoints.polymarket_url",
                    &mut initial_settings.polymarket_url,
                ),
                ("endpoints.kalshi_url", &mut initial_settings.kalshi_url),
            ] {
                if let Ok(Some(v)) = state_store.kv_get(key) {
                    let v = v.trim().to_string();
                    if !v.is_empty() {
                        *slot = v;
                    }
                }
            }
            if initial_settings.margin_warning_usage_pct
                >= initial_settings.margin_critical_usage_pct
            {
                initial_settings.margin_warning_usage_pct = 70.0;
                initial_settings.margin_critical_usage_pct = 85.0;
            }
        }
        load_exec_policy_settings(&state_store, &mut initial_settings);
        // Venue endpoint overrides come from Engine Settings (Network & VPS)
        // and persist under `endpoints.*` kv keys — blank falls back to each
        // adapter's built-in default. NOTE: adapters are constructed once
        // here, so an edited endpoint applies on the NEXT app start.
        let endpoint = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        let live_hl = Arc::new(HyperliquidAdapter::new(
            endpoint(&initial_settings.hyperliquid_url),
            None,
            None,
        ));
        let live_lighter = Arc::new(LighterAdapter::new(
            endpoint(&initial_settings.lighter_url),
            None,
            None,
        ));
        let live_vanta = Arc::new(VantaAdapter::new(
            endpoint(&initial_settings.vanta_url),
            None,
            None,
            None,
        ));
        // Robinhood Chain serves the Lighter protocol at a different base
        // URL — reuse the Lighter adapter under the robinhood venue id.
        let live_robinhood = Arc::new(LighterAdapter::new_with_venue(
            Some("https://api.rh.lighter.xyz".to_string()),
            None,
            None,
            "robinhood",
        ));
        let live_aster = Arc::new(AsterAdapter::new(None, None, None));
        // PopDEX's network is environment-driven like every other venue:
        // live_* always mainnet, tn_* always testnet — no per-venue toggle.
        let live_popdex = Arc::new(PopDexAdapter::new(None));
        let live_binance = Arc::new(BinanceAdapter::new(None, None, None));
        let live_okx = Arc::new(OkxAdapter::new(None, None, None, None));
        let live_coinbase = Arc::new(CoinbaseAdapter::new(None, None, None, None));
        let live_alpaca = Arc::new(AlpacaAdapter::new(None, None, None, None));
        // Alpaca paper trading is a real venue sandbox (separate host +
        // separate key namespace) — testnet admits it honestly.
        let tn_alpaca = Arc::new(AlpacaAdapter::for_paper(None, None));
        let live_ibkr = Arc::new(IbkrAdapter::new(None, None));
        let tn_ibkr = Arc::new(IbkrAdapter::for_paper(None, None));
        let live_kraken = Arc::new(KrakenAdapter::new(
            endpoint(&initial_settings.kraken_futures_url),
            endpoint(&initial_settings.kraken_spot_url),
            None,
            None,
        ));
        let live_deribit = Arc::new(DeribitAdapter::new(None, None, None));
        let live_decibel = Arc::new(DecibelAdapter::new(None, None, None));
        // Long-tail perp venues share one spec-driven adapter. No
        // endpoint overrides exist for these yet — `None` keeps each
        // spec's default base URL; the `base_url` injection point stays
        // open for future venue-testnet endpoints.
        let live_dex_perps: Vec<Arc<DexPerpAdapter>> = ADMITTED_DEX_PERP_VENUE_IDS
            .iter()
            .filter_map(|id| DexPerpAdapter::for_venue(id, None).map(Arc::new))
            .collect();
        // EVM spot AMMs: spec default public RPC until a per-account
        // `base_url` credential override repoints the adapter.
        let live_evm_amms: Vec<Arc<EvmAmmAdapter>> = ADMITTED_EVM_AMM_VENUE_IDS
            .iter()
            .filter_map(|id| EvmAmmAdapter::for_venue(id, None).map(Arc::new))
            .collect();
        let live_boros = Arc::new(BorosAdapter::new(None));
        let live_variational = Arc::new(VariationalAdapter::new(None));
        // Arcus public reads are unauthenticated — credentials arrive via
        // `update_credentials` for the (not yet implemented) signed paths.
        let live_arcus = Arc::new(ArcusAdapter::new(
            endpoint(&initial_settings.arcus_url),
            None,
            None,
        ));

        // Testnet adapter set — public testnet/demo endpoints only.
        // Venues absent here (Coinbase, Aster, Polymarket, the DexPerp
        // long-tail, and the EVM AMMs) have no usable public testnet and
        // stay ineligible under Testnet mode rather than being simulated.
        // Endpoint overrides are live-only; a testnet adapter always uses
        // the venue's canonical demo host.
        let tn_hl = Arc::new(HyperliquidAdapter::for_testnet(None, None));
        let tn_lighter = Arc::new(LighterAdapter::for_testnet(None, None));
        let tn_vanta = Arc::new(VantaAdapter::new(
            Some("https://testnet-api.orderly.org".to_string()),
            None,
            None,
            None,
        ));
        let tn_binance = Arc::new(BinanceAdapter::new(
            Some("https://testnet.binancefuture.com".to_string()),
            None,
            None,
        ));
        // OKX demo trading shares the production host — the
        // `x-simulated-trading: 1` header routes it to the demo namespace,
        // which requires keys minted under a demo account.
        let tn_okx = Arc::new(OkxAdapter::for_testnet(None, None, None, None));
        let tn_decibel = Arc::new(DecibelAdapter::for_testnet(None, None));
        let tn_popdex = Arc::new(PopDexAdapter::new(Some("testnet".to_string())));
        let tn_deribit = Arc::new(DeribitAdapter::for_testnet(None, None));
        // Robinhood Chain runs a real testnet Lighter deployment —
        // api.rh-testnet.lighter.xyz (verified live), signing chain id 300
        // shared with Lighter's own testnet per the official SDK.
        let tn_robinhood = Arc::new(LighterAdapter::new_with_venue(
            Some("https://api.rh-testnet.lighter.xyz".to_string()),
            None,
            None,
            "robinhood",
        ));
        // Arcus testnet exposes the identical public /v1 surface on the
        // demo host (verified: /v1/markets, /v1/l2OrderBook, /v1/mids).
        let tn_arcus = Arc::new(ArcusAdapter::new(
            Some("https://api.testnet.arcus.xyz".to_string()),
            None,
            None,
        ));

        let market_data_hub = MarketDataHub::new(1024, 400);
        // The hub's slot key follows the persisted USD-quote policy: under
        // `separate`, quote variants occupy distinct slots and never share
        // a cached book.
        market_data_hub.set_merge_usd_quotes(
            crate::stablecoin_fx::normalize_usd_quote_mode(&initial_settings.usd_quote_mode)
                != crate::stablecoin_fx::USD_QUOTE_MODE_SEPARATE,
        );

        let cb_config = CircuitBreakerConfig {
            max_daily_drawdown_usd: Decimal::from_f64_retain(
                initial_settings.cb_max_daily_drawdown_usd,
            )
            .unwrap_or(dec!(25000.0)),
            max_daily_drawdown_pct: Decimal::from_f64_retain(
                initial_settings.cb_max_daily_drawdown_pct,
            )
            .unwrap_or(dec!(5.0)),
            max_consecutive_errors: initial_settings.cb_max_consecutive_errors,
            max_spread_divergence_bps: Decimal::from_f64_retain(
                initial_settings.cb_max_spread_divergence_bps,
            )
            .unwrap_or(dec!(250.0)),
        };
        let cb = Arc::new(CircuitBreaker::new(cb_config, dec!(200000.0)));
        // Durable halt state: a tripped breaker must survive a restart —
        // otherwise a crash mid-drawdown silently re-arms trading. The kv
        // snapshot also restores the UTC-day baseline so a same-day restart
        // does not reset the day's drawdown window.
        if let Ok(Some(raw)) = state_store.kv_get("cb.snapshot") {
            match serde_json::from_str::<arbitrageur_risk::circuit_breaker::CircuitBreakerSnapshot>(
                &raw,
            ) {
                Ok(snap) => {
                    if snap.tripped {
                        tracing::warn!(
                            "circuit breaker restored TRIPPED: {}",
                            snap.trip_reason.as_deref().unwrap_or("unknown")
                        );
                    }
                    cb.restore(&snap);
                }
                Err(e) => {
                    if let Some(snap) = salvage_tripped_snapshot(&raw) {
                        tracing::warn!(
                            "circuit breaker snapshot partially unreadable ({e}); \
                             preserving TRIPPED state: {}",
                            snap.trip_reason.as_deref().unwrap_or("unknown")
                        );
                        cb.restore(&snap);
                    } else {
                        tracing::warn!(
                            "circuit breaker snapshot unreadable, starting fresh: {e}; raw={}",
                            &raw[..raw.len().min(200)]
                        );
                    }
                }
            }
        }
        {
            let store = state_store.clone();
            cb.set_persist_hook(std::sync::Arc::new(move |snap| {
                if let Ok(json) = serde_json::to_string(snap) {
                    let _ = store.kv_set("cb.snapshot", &json);
                }
            }));
        }
        // Frozen-FX day anchor for collateral-beta isolation (see
        // `breaker_fx_anchor` field docs). Missing/corrupt state simply
        // re-seeds on the first feed of the day — never blocks startup.
        let initial_fx_anchor = state_store
            .kv_get("cb.fx_anchor")
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<crate::execution::BreakerFxAnchor>(&raw).ok())
            .unwrap_or_default();
        // Shared with the breaker's trip hook (below) so out-of-band
        // alerts carry the live environment even after a runtime switch.
        let environment = Arc::new(RwLock::new(TradingEnvironment::from_mode(
            &initial_settings.trading_mode,
        )));
        crate::alerting::init(&cb, environment.clone());
        // An operator pause is a halt decision — it must survive a
        // restart (a crashed/closed app must not resume trading on its
        // own). Read before the struct literal moves `state_store`.
        let engine_paused_initial = state_store
            .kv_get("engine.paused")
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        let engine_stopped_initial = state_store
            .kv_get("engine.stopped")
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        let delta_guard = RwLock::new(DeltaGuard::new(
            Decimal::from_f64_retain(initial_settings.delta_max_unhedged_usd_per_asset)
                .unwrap_or(dec!(2500.0)),
        ));
        let router = SmartOrderRouter::new(dec!(15.0)); // 15 bps slippage tolerance
        let coordinator = Arc::new(AtomicArbitrageCoordinator::new(router, cb.clone()));
        let kalman_filter = RwLock::new(HedgeRatioKalmanFilter::new(1e-4, 1e-2));
        let settings = RwLock::new(initial_settings);

        // Reload the persisted execution log so the Log Monitor keeps history
        // across restarts (failures included since v21). A store failure
        // starts empty rather than bricking startup.
        let mut initial_logs: Vec<ExecutionLogEntry> =
            state_store.load_execution_logs(5000).unwrap_or_default();
        initial_logs.reverse();
        let vault = RwLock::new(crate::vault::SecureVault::new_with_path(vault_path));
        let evm_stronghold = RwLock::new(None);
        // The virtual cycle ledger is durable (schema v30): hydrate all lots
        // so open lots resume close-matching/exposure accounting across a
        // restart and closed lots keep strategy metrics continuous. A store
        // failure starts empty rather than bricking startup.
        let virtual_ledger =
            RwLock::new(arbitrageur_execution::virtual_ledger::VirtualStrategyLedger::new());
        if let Ok(lots) = state_store.load_cycle_lots() {
            virtual_ledger.write().set_lots(lots);
        }
        // Boot reconcile between the durable signal ledger and durable lots.
        // Two drift classes:
        //  (a) Open signal with filled Long+Short legs but no lot — positions
        //      opened before v30, or a crash between lot-open and its persist.
        //      Reconstruct the lot from the signal's authoritative legs so the
        //      eventual flatten can realize PnL + resolve predictions.
        //  (b) Open lot with no open signal on its symbol/venues — the signal
        //      closed while a crash window separated the two persists. Left
        //      Open it would misbind a *future* cycle's exit fills (stale
        //      entry prices → wrong PnL). Mark it Closed with unknown
        //      realized fields — an honest gap, never a fabricated outcome.
        {
            use crate::state_store::LegSide;
            let open_signals = state_store.list_open_signals(None).unwrap_or_default();
            // Lazy full-history list: needed only when a rebuild candidate
            // has family children (a mid-hold trim's fills reduce what the
            // rebuilt lot should carry — `flat-`/`trim-` children may be
            // Closed already, so the open-only list can't see them).
            let mut all_signals_cache: Option<Vec<crate::state_store::SignalExecution>> = None;
            let mut ledger = virtual_ledger.write();
            // Family children are not roots: a `trim-` child also carries a
            // Long and a Short leg — but on swapped venues — so it would
            // otherwise spawn a phantom `rehydrated-` lot on inverted venues.
            for sig in open_signals.iter().filter(|s| {
                s.status.counts_as_open()
                    && !s.id.starts_with("flat-")
                    && !s.id.starts_with("trim-")
            }) {
                let venue_of = |side: LegSide| {
                    sig.legs
                        .iter()
                        .find(|l| l.side == side)
                        .and_then(|l| canonical_venue_id(&l.venue))
                };
                let leg_of = |side: LegSide| sig.legs.iter().find(|l| l.side == side);
                let (Some(long_leg), Some(short_leg), Some(long_v), Some(short_v)) = (
                    leg_of(LegSide::Long),
                    leg_of(LegSide::Short),
                    venue_of(LegSide::Long),
                    venue_of(LegSide::Short),
                ) else {
                    continue;
                };
                let (Some(long_px), Some(short_px)) = (long_leg.fill_price, short_leg.fill_price)
                else {
                    continue;
                };
                // Residual quantities after family children: a mid-hold
                // trim's fills already reduced the physical position, so a
                // rebuilt lot must carry the remainder — otherwise the final
                // flatten's fills could never satisfy `exit.qty >= lot.qty`
                // and the lot would dead-close without an outcome.
                if all_signals_cache.is_none() {
                    all_signals_cache = state_store.list_signals(None, None).ok();
                }
                let closed_qty = |venue: &str, closing: LegSide| -> f64 {
                    let Some(all) = &all_signals_cache else {
                        return 0.0;
                    };
                    all.iter()
                        .filter(|c| crate::state_store::is_flatten_child_id(&c.id, &sig.id))
                        .flat_map(|c| c.legs.iter())
                        .filter(|l| {
                            l.side == closing && canonical_venue_id(&l.venue) == Some(venue)
                        })
                        .map(|l| l.filled_qty)
                        .sum()
                };
                let rem_long = (long_leg.filled_qty - closed_qty(long_v, LegSide::Short)).max(0.0);
                let rem_short =
                    (short_leg.filled_qty - closed_qty(short_v, LegSide::Long)).max(0.0);
                // Cross-instrument legs fill in their own venue units —
                // match them in shared units (qty ÷ leg scale), then hand
                // the lot each leg's own-unit remainder so its per-leg
                // PnL and close-matching stay venue-true.
                let matched_g =
                    (rem_long / long_leg.unit_scale).min(rem_short / short_leg.unit_scale);
                let rem_long = matched_g * long_leg.unit_scale;
                let rem_short = matched_g * short_leg.unit_scale;
                if !(matched_g > 0.0 && long_px > 0.0 && short_px > 0.0) {
                    continue;
                }
                let want_strategy = crate::attribution::ledger_strategy_id(&sig.strategy_id);
                let has_lot = ledger.lots.values().any(|lot| {
                    lot.status == arbitrageur_execution::virtual_ledger::CycleStatus::Open
                        && lot.symbol == sig.symbol
                        && canonical_venue_id(&lot.venue_long) == Some(long_v)
                        && canonical_venue_id(&lot.venue_short) == Some(short_v)
                });
                if has_lot {
                    continue;
                }
                let entry_fees: f64 = sig
                    .legs
                    .iter()
                    .map(|l| l.fee_usd)
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .sum();
                let id = format!("rehydrated-{}", sig.id);
                ledger.open_cycle(
                    id.clone(),
                    want_strategy,
                    sig.symbol.clone(),
                    long_v.to_string(),
                    short_v.to_string(),
                    0.0,
                    rem_long,
                    long_px,
                    rem_short,
                    short_px,
                    entry_fees,
                    sig.environment.clone(),
                );
                if let Some(lot) = ledger.lots.get(&id) {
                    if let Err(e) = state_store.upsert_cycle_lot(lot) {
                        tracing::warn!("persist rehydrated cycle lot {id} failed: {e}");
                    }
                }
            }
            // Direction (b): stale open lots whose symbol has no open signal.
            let open_symbols: std::collections::HashSet<String> = open_signals
                .iter()
                .filter(|s| s.status.counts_as_open())
                .map(|s| s.symbol.clone())
                .collect();
            let stale_ids: Vec<String> = ledger
                .lots
                .values()
                .filter(|lot| {
                    lot.status == arbitrageur_execution::virtual_ledger::CycleStatus::Open
                        && !open_symbols.contains(&lot.symbol)
                })
                .map(|lot| lot.id.clone())
                .collect();
            for id in stale_ids {
                if let Some(lot) = ledger.lots.get_mut(&id) {
                    lot.status = arbitrageur_execution::virtual_ledger::CycleStatus::Closed;
                    let snapshot = lot.clone();
                    if let Err(e) = state_store.upsert_cycle_lot(&snapshot) {
                        tracing::warn!("persist stale-closed cycle lot {id} failed: {e}");
                    }
                }
            }
        }
        // Cumulative realized-PnL counters persist per environment so the
        // header "Net Profit" survives a restart (previously session-scoped).
        let load_pnl = |key: &str| {
            state_store
                .kv_get(key)
                .ok()
                .flatten()
                .and_then(|v| Decimal::from_str_exact(&v).ok())
                .unwrap_or(dec!(0))
        };
        let mut initial_pnl_testnet = load_pnl("pnl.total.testnet");
        let mut initial_pnl_live = load_pnl("pnl.total.live");
        // Option premium cashflow is tracked apart from realized PnL —
        // premium paid at entry is financing, not loss; the trade's true
        // PnL lands through close-path leg deltas.
        let mut initial_premium_testnet = load_pnl("pnl.premium_outlay.testnet");
        let mut initial_premium_live = load_pnl("pnl.premium_outlay.live");
        // One-time rebalance: option entries dispatched before the counter
        // split booked their premium cashflow into `pnl.total.{env}` — the
        // close path also books full leg deltas, so leaving it there would
        // double-count. Move the still-open families' premium (recomputed
        // from durable leg fills — ledger prices are USD) into the outlay
        // counter. Closed families are untouched: their history can't be
        // unambiguously reconstructed.
        if state_store
            .kv_get("pnl.premium_outlay.migrated_v1")
            .ok()
            .flatten()
            .is_none()
        {
            let mut moved = std::collections::HashMap::<String, Decimal>::new();
            if let Ok(open) = state_store.list_open_signals(None) {
                for sig in &open {
                    if !sig.strategy_id.starts_with("options")
                        || sig.id.starts_with("flat-")
                        || sig.id.starts_with("trim-")
                    {
                        continue;
                    }
                    if sig.environment != "testnet" && sig.environment != "live" {
                        continue;
                    }
                    let premium: Decimal = sig
                        .legs
                        .iter()
                        .filter(|l| crate::state_store::symbol_is_fixed_contract(&l.symbol))
                        .filter_map(|l| l.fill_price.map(|px| (l.side, px, l.filled_qty)))
                        .filter(|(_, px, q)| *px > 0.0 && *q > 0.0)
                        .map(|(side, px, q)| {
                            let usd = Decimal::from_f64_retain(px * q).unwrap_or(dec!(0));
                            match side {
                                crate::state_store::LegSide::Short => usd,
                                crate::state_store::LegSide::Long => -usd,
                            }
                        })
                        .sum();
                    if premium != dec!(0) {
                        *moved.entry(sig.environment.clone()).or_default() += premium;
                    }
                }
            }
            for (env, premium) in moved {
                let from_key = format!("pnl.total.{env}");
                let to_key = format!("pnl.premium_outlay.{env}");
                let cur: Decimal = state_store
                    .kv_get(&from_key)
                    .ok()
                    .flatten()
                    .and_then(|v| Decimal::from_str_exact(&v).ok())
                    .unwrap_or(dec!(0));
                let cur_to: Decimal = state_store
                    .kv_get(&to_key)
                    .ok()
                    .flatten()
                    .and_then(|v| Decimal::from_str_exact(&v).ok())
                    .unwrap_or(dec!(0));
                let _ = state_store.kv_set(&from_key, &(cur - premium).to_string());
                let _ = state_store.kv_set(&to_key, &(cur_to + premium).to_string());
                if env == "testnet" {
                    initial_pnl_testnet -= premium;
                    initial_premium_testnet += premium;
                } else {
                    initial_pnl_live -= premium;
                    initial_premium_live += premium;
                }
                tracing::info!(
                    "premium-outlay migration: moved {premium} USD of open option premium out of pnl.total.{env}"
                );
            }
            let _ = state_store.kv_set(
                "pnl.premium_outlay.migrated_v1",
                &chrono::Utc::now().to_rfc3339(),
            );
        }

        // Reconcile pnl.total.{env} to true realized trading profit by excluding
        // open options capital outlays (which are financing tracked under pnl.premium_outlay)
        // and counting only transaction fees and non-options trading profits.
        if state_store
            .kv_get("pnl.premium_outlay.migrated_v3")
            .ok()
            .flatten()
            .is_none()
        {
            if let Ok(all_logs) = state_store.load_execution_logs(100_000) {
                let mut true_realized_testnet = Decimal::ZERO;
                let mut true_realized_live = Decimal::ZERO;
                let mut has_testnet = false;
                let mut has_live = false;
                for l in &all_logs {
                    let net = Decimal::from_str_exact(&l.net_profit.to_string()).unwrap_or(dec!(0));
                    let fee = Decimal::from_str_exact(&l.fees.to_string()).unwrap_or(dec!(0));
                    let entry_realized = if l.strategy_id.starts_with("options") {
                        -fee
                    } else {
                        net
                    };
                    if l.is_live {
                        has_live = true;
                        true_realized_live += entry_realized;
                    } else {
                        has_testnet = true;
                        true_realized_testnet += entry_realized;
                    }
                }
                if has_testnet {
                    initial_pnl_testnet = true_realized_testnet;
                    let _ =
                        state_store.kv_set("pnl.total.testnet", &true_realized_testnet.to_string());
                    tracing::info!("pnl.total.testnet recalibrated to true realized PnL: {true_realized_testnet}");
                }
                if has_live {
                    initial_pnl_live = true_realized_live;
                    let _ = state_store.kv_set("pnl.total.live", &true_realized_live.to_string());
                    tracing::info!(
                        "pnl.total.live recalibrated to true realized PnL: {true_realized_live}"
                    );
                }
            }
            let _ = state_store.kv_set(
                "pnl.premium_outlay.migrated_v3",
                &chrono::Utc::now().to_rfc3339(),
            );
        }
        // `sim_step_counter` feeds durable ids (`spatial-{n}` lots,
        // `funding-{n}` lots, `exec-*-{n}` log rows) — resuming at 8 after a
        // restart would collide with persisted ids and `INSERT OR REPLACE`
        // would silently overwrite them. Resume from kv, floored above the
        // durable log table so the very first post-v30 boot (kv absent)
        // cannot collide either.
        let initial_step = {
            let persisted = state_store
                .kv_get("engine.sim_step")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<usize>().ok());
            let log_floor = state_store.execution_log_count().unwrap_or(0) as usize + 8;
            persisted.unwrap_or(8).max(log_floor)
        };
        let strategy_params_map = load_strategy_params(&state_store);
        let ticker_configs_map = load_ticker_strategy_configs(&state_store);

        Self {
            live_hl,
            live_lighter,
            live_vanta,
            live_robinhood,
            live_aster,
            live_popdex,
            live_binance,
            live_okx,
            live_coinbase,
            live_kraken,
            live_alpaca,
            live_ibkr,
            tn_alpaca,
            tn_ibkr,
            live_deribit,
            live_decibel,
            live_dex_perps,
            live_evm_amms,
            live_boros,
            live_variational,
            live_arcus,
            tn_hl,
            tn_lighter,
            tn_vanta,
            tn_binance,
            tn_okx,
            tn_deribit,
            tn_decibel,
            tn_popdex,
            tn_robinhood,
            tn_arcus,
            market_data_hub,
            boros_yu_cache: Mutex::new(None),
            circuit_breaker: cb,
            delta_guard,
            coordinator,
            kalman_filter,
            environment,
            last_risk_live_universe: RwLock::new(None),
            last_risk_account_set: RwLock::new(None),
            last_good_accounts: Mutex::new(std::collections::HashMap::new()),
            breaker_venue_equity: Mutex::new(std::collections::HashMap::new()),
            breaker_fx_anchor: Mutex::new(initial_fx_anchor),
            breaker_fx_adj: Mutex::new(std::collections::HashMap::new()),
            equity_fx_prev: Mutex::new(
                state_store
                    .kv_get("equity.fx_prev")
                    .ok()
                    .flatten()
                    .and_then(|raw| {
                        serde_json::from_str::<std::collections::HashMap<String, Decimal>>(&raw)
                            .ok()
                    })
                    .unwrap_or_default(),
            ),
            equity_upnl_prev: Mutex::new(
                state_store
                    .kv_get("equity.upnl_prev")
                    .ok()
                    .flatten()
                    .and_then(|raw| {
                        serde_json::from_str::<std::collections::HashMap<String, f64>>(&raw).ok()
                    })
                    .unwrap_or_default(),
            ),
            equity_optval_prev: Mutex::new(
                state_store
                    .kv_get("equity.optval_prev")
                    .ok()
                    .flatten()
                    .and_then(|raw| {
                        serde_json::from_str::<std::collections::HashMap<String, f64>>(&raw).ok()
                    })
                    .unwrap_or_default(),
            ),
            equity_bal_prev: Mutex::new(
                state_store
                    .kv_get("equity.bal_prev")
                    .ok()
                    .flatten()
                    .and_then(|raw| {
                        serde_json::from_str::<std::collections::HashMap<String, f64>>(&raw).ok()
                    })
                    .unwrap_or_default(),
            ),
            equity_check_lock: Mutex::new(()),
            options_tick_status: Mutex::new(std::collections::HashMap::new()),
            feed_lanes: Mutex::new(std::collections::BTreeMap::new()),
            total_realized_profit: RwLock::new(initial_pnl_testnet),
            total_realized_profit_live: RwLock::new(initial_pnl_live),
            total_premium_outlay: RwLock::new(initial_premium_testnet),
            total_premium_outlay_live: RwLock::new(initial_premium_live),
            execution_logs: RwLock::new(initial_logs),
            sim_step_counter: RwLock::new(initial_step),
            settings,
            strategy_configs: RwLock::new(ticker_configs_map),
            strategy_params: RwLock::new(strategy_params_map),
            vault,
            evm_stronghold,
            state_store,
            virtual_ledger,
            margin_rebalance_daemon: RwLock::new(
                arbitrageur_risk::collateral_rebalance::MarginRebalanceDaemonState::default(),
            ),
            is_paused: RwLock::new(engine_paused_initial),
            is_stopped: RwLock::new(engine_stopped_initial),
            execution_lock: tokio::sync::Mutex::new(()),
            reconcile_lock: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            test_adapters: std::sync::RwLock::new(std::collections::HashMap::new()),
            #[cfg(test)]
            test_deribit: std::sync::RwLock::new(None),
            #[cfg(test)]
            test_boros: std::sync::RwLock::new(None),
            #[cfg(test)]
            test_sims: std::sync::RwLock::new(std::collections::HashMap::new()),
            autotune_apply_lock: tokio::sync::Mutex::new(()),
            fee_enrich_done: Mutex::new(std::collections::HashSet::new()),
            sample_prune_at: Mutex::new(0),
            liq_trim_at: Mutex::new(std::collections::HashMap::new()),
            risk_locked_warn_at: Mutex::new(None),
            risk_stale_warn_at: Mutex::new(std::collections::HashMap::new()),
            vault_unlock_backoff: Mutex::new((0, std::time::Instant::now())),
            mfa_pending: Mutex::new(None),
            margin_snapshot_at: Mutex::new(std::collections::HashMap::new()),
            latency_ema: Mutex::new(std::collections::HashMap::new()),
            latency_probe_at: Mutex::new(std::collections::HashMap::new()),
            volume_cache: Mutex::new(None),
            markets_cache: Mutex::new(std::collections::HashMap::new()),
            market_prices_cache: Mutex::new(std::collections::HashMap::new()),
            venue_fetch_failures: Mutex::new(std::collections::HashMap::new()),
            monitor: Mutex::new(None),
            radar_bot: Mutex::new(None),
            orderbook_flights: Arc::new(crate::ws_primary::OrderbookSingleFlight::new()),
            account_fetch_locks: Mutex::new(std::collections::HashMap::new()),
            stablecoin_fx: crate::stablecoin_fx::StablecoinFx::new(),
            vol_surface: Arc::new(LiveVolSurfaceCache::new()),
        }
    }

    /* ------------------------------------------------------------------ */
    /* USD-stablecoin quote policy                                         */
    /* ------------------------------------------------------------------ */

    /// Whether USD-family quotes merge into one canonical base under the
    /// active mode (parity/fx_adjusted). `separate` keeps BASE-QUOTE
    /// identity end to end.
    pub fn usd_merge_quotes(&self) -> bool {
        crate::stablecoin_fx::normalize_usd_quote_mode(&self.settings.read().usd_quote_mode)
            != crate::stablecoin_fx::USD_QUOTE_MODE_SEPARATE
    }

    /// Price→USD conversion factor for a book quoted in `quote` on `venue`.
    /// 1.0 outside `fx_adjusted` mode; fx mode prefers the venue-local rate
    /// and falls back to the global table → parity baseline.
    pub fn fx_price_factor(&self, venue: Option<&str>, quote: &str) -> f64 {
        let mode =
            crate::stablecoin_fx::normalize_usd_quote_mode(&self.settings.read().usd_quote_mode);
        if mode != crate::stablecoin_fx::USD_QUOTE_MODE_FX_ADJUSTED {
            return 1.0;
        }
        self.stablecoin_fx
            .fx_factor(venue, quote, chrono::Utc::now().timestamp_millis())
    }

    /// The quote currency a (venue, symbol) book is priced in: explicit
    /// symbol suffix first, then the monitor's refreshed quote map, then
    /// the USD baseline.
    pub fn book_quote(&self, venue: &str, symbol: &str) -> String {
        if let Some(q) = arbitrageur_core::symbol::symbol_quote(symbol) {
            return q;
        }
        self.monitor
            .lock()
            .as_ref()
            .and_then(|m| m.book_quote(venue, symbol, self.usd_merge_quotes()))
            .unwrap_or_else(|| "USD".into())
    }

    /// The currency a venue reports account equity/margin in — the modal
    /// quote across its listed markets, learned by the monitor. Falls back
    /// to "USD" when no lane has refreshed yet.
    pub fn venue_settlement_quote(&self, venue: &str) -> String {
        self.monitor
            .lock()
            .as_ref()
            .and_then(|m| m.settlement_quote(venue))
            .unwrap_or_else(|| "USD".into())
    }

    /// Re-denominate an account snapshot into the USD basis under
    /// `fx_adjusted` mode: equity/margin fields scale by the venue's
    /// settlement-quote rate, position prices/PnL by each symbol's own
    /// quote, and USD-family balance rows by that coin's rate. Identity
    /// outside fx mode. Call at consume sites only — the
    /// `last_good_accounts` cache stays in venue units.
    pub fn normalize_account_usd(
        &self,
        venue: &str,
        acc: &mut arbitrageur_core::position::AccountSnapshot,
    ) {
        use rust_decimal::Decimal;
        let fx = |quote: &str| -> Decimal {
            Decimal::from_f64_retain(self.fx_price_factor(Some(venue), quote))
                .unwrap_or(Decimal::ONE)
        };
        let settle = self.venue_settlement_quote(venue);
        let sf = fx(&settle);
        acc.equity *= sf;
        acc.free_margin *= sf;
        acc.maintenance_margin *= sf;
        for pos in acc.positions.iter_mut() {
            let pq = arbitrageur_core::symbol::symbol_quote(&pos.symbol)
                .unwrap_or_else(|| settle.clone());
            let f = fx(&pq);
            pos.entry_price *= f;
            pos.mark_price *= f;
            pos.unrealized_pnl *= f;
            pos.realized_pnl *= f;
            pos.initial_margin *= f;
            pos.maintenance_margin *= f;
            if let Some(liq) = pos.liquidation_price.as_mut() {
                *liq *= f;
            }
        }
        for bal in acc.balances.iter_mut() {
            if arbitrageur_core::symbol::is_usd_family(&bal.currency) {
                let f = fx(&bal.currency);
                bal.total *= f;
                bal.free *= f;
                bal.locked *= f;
            }
        }
    }

    /// Depeg gate for execution paths: refuse books priced in a halted
    /// stablecoin — except books whose base is itself USD-family, which
    /// ARE the depeg arbitrage and must stay tradeable. Fail-open for
    /// non-family/unknown quotes (they were never merged into USD pairs).
    pub fn stablecoin_gate(&self, base: &str, quote: &str) -> Result<(), String> {
        if !arbitrageur_core::symbol::is_usd_family(quote)
            || arbitrageur_core::symbol::is_usd_family(&arbitrageur_core::symbol::canonical_base(
                base,
            ))
        {
            return Ok(());
        }
        let halt_bps = self.settings.read().stablecoin_halt_bps;
        let now = chrono::Utc::now().timestamp_millis();
        if self.stablecoin_fx.is_halted(quote, now, halt_bps) {
            let dev = self.stablecoin_fx.deviation_bps(quote, now).unwrap_or(0.0);
            return Err(format!(
                "{quote} is depegged ({dev:.0}bps from USD, halt at {halt_bps:.0}bps) — trades quoted in it are blocked"
            ));
        }
        Ok(())
    }

    /// Ensure market is loaded - live real market data is queried from active exchange APIs
    pub fn ensure_market(&self, _symbol: &str) {
        // No-op: Real market data is pulled directly from configured live exchange APIs only.
        // No synthetic markets are ever seeded or fabricated.
    }

    /// Next durable cycle index. Persisted under `engine.sim_step` so
    /// `spatial-{n}`/`funding-{n}` lot ids and `exec-*-{n}` log ids never
    /// collide with pre-restart rows (a reset would silently overwrite
    /// durable lots and audit logs via `INSERT OR REPLACE`). A kv write
    /// failure degrades to in-memory monotonicity — still collision-safe
    /// within this boot.
    pub fn next_sim_step(&self) -> usize {
        let next = {
            let mut step = self.sim_step_counter.write();
            *step += 1;
            *step
        };
        if let Err(e) = self
            .state_store
            .kv_set("engine.sim_step", &next.to_string())
        {
            tracing::warn!("persist sim_step failed: {e}");
        }
        next
    }

    /// Open the Stronghold snapshot that holds the shared EVM wallet key.
    /// The snapshot password derives from the vault session key, so this
    /// only succeeds while the vault is unlocked; a failed open logs a
    /// warning and leaves `None` — credential records then keep their
    /// vault-held keys working (the sentinel path only engages when a
    /// stronghold exists to provision signers from).
    pub fn open_evm_stronghold(&self) {
        let (key, path) = {
            let mut vault = self.vault.write();
            if !vault.is_unlocked() {
                return;
            }
            let Some(key) = vault.stronghold_session_key() else {
                return;
            };
            (key, vault.stronghold_snapshot_path())
        };
        match crate::stronghold::EvmStronghold::open(&path, &key) {
            Ok(sh) => {
                *self.evm_stronghold.write() = Some(sh);
            }
            Err(e) => {
                tracing::warn!("EVM stronghold open failed ({e}) — shared wallet stays vault-held");
                *self.evm_stronghold.write() = None;
            }
        }
    }

    /// Drop the stronghold — manual vault lock. Adapter signers are wiped
    /// by the credential sync that follows a lock, so dropping first is
    /// safe and immediate (the guarded key memory dies with it).
    pub fn close_evm_stronghold(&self) {
        *self.evm_stronghold.write() = None;
    }

    /// Synchronize credentials from the secure vault to exchange adapters in
    /// BOTH environments. An account row's `environment` decides which adapter
    /// set receives its credentials — live keys never provision testnet
    /// adapters and vice versa. An explicit operator lock clears adapter
    /// credentials (panic-halt); timer-based auto-lock only forgets vault
    /// secrets — provisioned credentials keep working so an unattended
    /// deployment stays live.
    pub fn sync_vault_credentials_to_adapters(&self) {
        let mut vault = self.vault.write();
        if !vault.is_unlocked() {
            if vault.is_auto_locked() {
                return;
            }
            self.clear_adapter_credentials();
            return;
        }

        if let Ok(data) = vault.get_data() {
            let live = self.resolve_env_credentials(data, TradingEnvironment::Live);
            self.apply_live_credentials(&live);
            let testnet = self.resolve_env_credentials(data, TradingEnvironment::Testnet);
            self.apply_testnet_credentials(&testnet);
        } else {
            self.clear_adapter_credentials();
        }
    }

    /// Strip credentials from every adapter in both environments.
    fn clear_adapter_credentials(&self) {
        self.live_aster.update_credentials(None, None);
        let _ = self.live_popdex.update_credentials(None, None, None);
        self.live_hl.update_credentials(None, None);
        self.live_lighter.update_credentials(None, None);
        self.live_lighter.set_api_key_index(None);
        self.live_robinhood.update_credentials(None, None);
        self.live_robinhood.set_api_key_index(None);
        self.live_vanta.update_credentials(None, None, None);
        self.live_binance.update_credentials(None, None);
        self.live_okx.update_credentials(None, None, None);
        let _ = self.live_okx.set_base_url(None);
        self.live_coinbase.update_credentials(None, None, None);
        self.live_kraken.update_credentials(None, None);
        self.live_alpaca.update_credentials(None, None);
        self.live_ibkr.update_credentials(None, None);
        self.live_deribit.update_credentials(None, None);
        self.live_decibel.update_credentials(None, None, None, None);
        for adapter in &self.live_dex_perps {
            let _ = adapter.update_credentials(None, None, None, None, None, None);
        }
        for adapter in &self.live_evm_amms {
            let _ = adapter.update_credentials(None, None, None, None, None, None);
            let _ = adapter.set_base_url(None);
        }
        let _ = self.live_boros.update_credentials(None, None, None);
        self.live_arcus.update_credentials(None, None, None, None);
        self.tn_hl.update_credentials(None, None);
        self.tn_lighter.update_credentials(None, None);
        self.tn_lighter.set_api_key_index(None);
        self.tn_vanta.update_credentials(None, None, None);
        self.tn_binance.update_credentials(None, None);
        self.tn_okx.update_credentials(None, None, None);
        let _ = self.tn_okx.set_base_url(None);
        self.tn_alpaca.update_credentials(None, None);
        self.tn_ibkr.update_credentials(None, None);
        self.tn_deribit.update_credentials(None, None);
        self.tn_decibel.update_credentials(None, None, None, None);
        let _ = self.tn_popdex.update_credentials(None, None, None);
        self.tn_robinhood.update_credentials(None, None);
        self.tn_robinhood.set_api_key_index(None);
        self.tn_arcus.update_credentials(None, None, None, None);
        // Kalshi's adapter is a prediction stub — no credential setter exists
        // to clear; its demo endpoint carries no secrets.
    }

    /// Resolve vault credentials for one environment. An account row's
    /// `environment` picks which adapter set its secrets reach; legacy
    /// unbound venue records (no profile) are treated as live credentials —
    /// they predate the split and never provision a testnet adapter.
    fn resolve_env_credentials(
        &self,
        data: &crate::vault::VaultData,
        env: TradingEnvironment,
    ) -> EnvCredentials {
        let env_str = env.as_str();
        let get_active_cred = |venues: &[&str]| -> Option<&crate::vault::ExchangeCredentials> {
            for &v in venues {
                if let Ok(Some(acc)) = self.state_store.default_account(v, env_str) {
                    if let Some(cred) = data
                        .credentials
                        .iter()
                        .find(|c| c.enabled && c.profile_id.as_deref() == Some(&acc.id))
                    {
                        return Some(cred);
                    }
                }
                let has_accounts = self
                    .state_store
                    .list_accounts(Some(v))
                    .map(|l| l.iter().any(|a| a.environment == env_str))
                    .unwrap_or(false);
                if has_accounts {
                    return None;
                }
            }
            // Legacy unbound venue-keyed records: live credentials only.
            if env == TradingEnvironment::Live {
                data.credentials
                    .iter()
                    .find(|c| c.enabled && venues.iter().any(|v| c.venue.eq_ignore_ascii_case(v)))
            } else {
                None
            }
        };

        let mut creds = EnvCredentials::default();
        if let Some(cred) = get_active_cred(&["aster", "asterdex"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.aster = (Some(cred.api_key.clone()), Some(cred.api_secret.clone()));
            }
        }
        if let Some(cred) = get_active_cred(&["hyperliquid", "hl"]) {
            let wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
            let pk = cred.private_key.clone().filter(|k| !k.trim().is_empty());
            if wallet.is_some() && pk.is_some() {
                creds.hl_wallet = wallet;
                creds.hl_pk = pk;
            }
        }
        if let Some(cred) = get_active_cred(&["lighter"]) {
            let key = if !cred.api_key.trim().is_empty() {
                Some(cred.api_key.clone())
            } else {
                cred.private_key.clone().filter(|k| !k.trim().is_empty())
            };
            if key.is_some() {
                creds.lighter_key = key;
                creds.lighter_idx = cred.account_index;
                creds.lighter_key_idx = cred.api_key_index.and_then(|v| u8::try_from(v).ok());
                if cred.api_key_index.is_some() && creds.lighter_key_idx.is_none() {
                    tracing::warn!(
                        "lighter api_key_index {:?} out of range 0-254; live orders will fail closed until corrected",
                        cred.api_key_index
                    );
                }
            }
        }
        if let Some(cred) = get_active_cred(&["robinhood", "robinhood lighter", "rh"]) {
            let key = if !cred.api_key.trim().is_empty() {
                Some(cred.api_key.clone())
            } else {
                cred.private_key.clone().filter(|k| !k.trim().is_empty())
            };
            if key.is_some() {
                creds.rh_key = key;
                creds.rh_idx = cred.account_index;
                creds.rh_key_idx = cred.api_key_index.and_then(|v| u8::try_from(v).ok());
                if cred.api_key_index.is_some() && creds.rh_key_idx.is_none() {
                    tracing::warn!(
                        "robinhood api_key_index {:?} out of range 0-254; live orders will fail closed until corrected",
                        cred.api_key_index
                    );
                }
            }
        }
        if let Some(cred) = get_active_cred(&["vanta", "orderly"]) {
            if !cred.api_key.trim().is_empty() {
                creds.vanta_key = Some(cred.api_key.clone());
            }
            if !cred.api_secret.trim().is_empty() {
                creds.vanta_sec = Some(cred.api_secret.clone());
            }
            creds.vanta_acc = cred
                .account_id
                .clone()
                .filter(|a| !a.trim().is_empty())
                .or_else(|| cred.wallet_address.clone().filter(|a| !a.trim().is_empty()));
        }
        if let Some(cred) = get_active_cred(&["binance", "bnb"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.binance_key = Some(cred.api_key.clone());
                creds.binance_sec = Some(cred.api_secret.clone());
            }
        }
        if let Some(cred) = get_active_cred(&["arcus"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.arcus_key = Some(cred.api_key.clone());
                creds.arcus_sec = Some(cred.api_secret.clone());
            }
            creds.arcus_wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
            creds.arcus_subaccount = cred.subaccount_index;
        }
        if let Some(cred) = get_active_cred(&["okx"]) {
            // Regional-domain override applies to public endpoints too —
            // an app.okx.com account's instrument set differs from the
            // global site's, so quotes must come from the same entity.
            creds.okx_url = cred.base_url.clone().filter(|u| !u.trim().is_empty());
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.okx_key = Some(cred.api_key.clone());
                creds.okx_sec = Some(cred.api_secret.clone());
                creds.okx_pass = cred.passphrase.clone().filter(|p| !p.trim().is_empty());
            }
        }
        if let Some(cred) = get_active_cred(&["coinbase", "cb"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.cb_key = Some(cred.api_key.clone());
                creds.cb_sec = Some(cred.api_secret.clone());
                creds.cb_pass = cred.passphrase.clone().filter(|p| !p.trim().is_empty());
            }
        }
        if let Some(cred) = get_active_cred(&["kraken", "krk", "kraken_futures", "kraken_spot"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.kraken_key = Some(cred.api_key.clone());
                creds.kraken_sec = Some(cred.api_secret.clone());
            }
        }
        if let Some(cred) = get_active_cred(&["alpaca"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.alpaca_key = Some(cred.api_key.clone());
                creds.alpaca_sec = Some(cred.api_secret.clone());
            }
        }
        if let Some(cred) =
            get_active_cred(&["ibkr", "interactive_brokers", "interactive brokers", "ib"])
        {
            creds.ibkr_account = cred.account_id.clone().filter(|a| !a.trim().is_empty());
            creds.ibkr_url = cred.base_url.clone().filter(|u| !u.trim().is_empty());
        }
        if let Some(cred) = get_active_cred(&["deribit", "dbit"]) {
            if !cred.api_key.trim().is_empty() && !cred.api_secret.trim().is_empty() {
                creds.deribit_id = Some(cred.api_key.clone());
                creds.deribit_sec = Some(cred.api_secret.clone());
            }
        }
        if let Some(cred) = get_active_cred(&["decibel", "dcb", "decibel (aptos)"]) {
            if !cred.api_key.trim().is_empty() {
                creds.decibel_key = Some(cred.api_key.clone());
            }
            creds.decibel_node_key = cred
                .passphrase
                .clone()
                .filter(|p| !p.trim().is_empty())
                .or_else(|| {
                    if !cred.api_secret.trim().is_empty() {
                        Some(cred.api_secret.clone())
                    } else {
                        None
                    }
                });
            creds.decibel_wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
            creds.decibel_pk = cred.private_key.clone().filter(|k| !k.trim().is_empty());
        }
        if let Some(cred) = get_active_cred(&["popdex", "pop"]) {
            creds.popdex_wallet = cred.wallet_address.clone().filter(|w| !w.trim().is_empty());
            creds.popdex_agent = cred.private_key.clone().filter(|k| !k.trim().is_empty());
        }
        for vid in ADMITTED_DEX_PERP_VENUE_IDS {
            if let Some(cred) = get_active_cred(&[vid]) {
                creds.dex_perp.insert(vid, cred.clone());
            }
        }
        for vid in ADMITTED_EVM_AMM_VENUE_IDS {
            if let Some(cred) = get_active_cred(&[vid]) {
                creds.evm_amm.insert(vid, cred.clone());
            }
        }
        if let Some(cred) = get_active_cred(&["boros", "pendle", "pendle boros"]) {
            creds.boros = Some(cred.clone());
        }
        creds
    }

    /// Provision resolved credentials onto the production adapter set.
    fn apply_live_credentials(&self, c: &EnvCredentials) {
        self.live_aster
            .update_credentials(c.aster.0.clone(), c.aster.1.clone());
        // PopDEX: main account + delegated agent key from the vault. The
        // agent key alone enables trading; the main key is never held
        // (one-time approveAgent runs externally per docs). Network stays
        // at the constructor's mainnet binding — the trading environment
        // toggle is the only network selector.
        if let Err(e) = self.live_popdex.update_credentials(
            None,
            c.popdex_wallet.clone(),
            c.popdex_agent.clone(),
        ) {
            tracing::warn!("PopDEX credential sync refused: {e}");
        }
        self.live_hl
            .update_credentials(c.hl_wallet.clone(), c.hl_pk.clone());
        self.live_lighter
            .update_credentials(c.lighter_idx, c.lighter_key.clone());
        self.live_lighter.set_api_key_index(c.lighter_key_idx);
        self.live_robinhood
            .update_credentials(c.rh_idx, c.rh_key.clone());
        self.live_robinhood.set_api_key_index(c.rh_key_idx);
        self.live_vanta.update_credentials(
            c.vanta_acc.clone(),
            c.vanta_key.clone(),
            c.vanta_sec.clone(),
        );
        self.live_arcus.update_credentials(
            c.arcus_key.clone(),
            c.arcus_sec.clone(),
            c.arcus_wallet.clone(),
            c.arcus_subaccount,
        );
        self.live_binance
            .update_credentials(c.binance_key.clone(), c.binance_sec.clone());
        // Endpoint identity precedes credentials: a regional-entity key
        // must never be provisioned onto the wrong host, and a rejected
        // override withholds the credentials entirely (fail closed).
        if let Err(e) = self.live_okx.set_base_url(c.okx_url.clone()) {
            tracing::warn!("OKX base URL override rejected ({e}) — credentials withheld");
            self.live_okx.update_credentials(None, None, None);
        } else {
            self.live_okx.update_credentials(
                c.okx_key.clone(),
                c.okx_sec.clone(),
                c.okx_pass.clone(),
            );
        }
        self.live_coinbase.update_credentials(
            c.cb_key.clone(),
            c.cb_sec.clone(),
            c.cb_pass.clone(),
        );
        self.live_kraken
            .update_credentials(c.kraken_key.clone(), c.kraken_sec.clone());
        self.live_alpaca
            .update_credentials(c.alpaca_key.clone(), c.alpaca_sec.clone());
        self.live_ibkr
            .update_credentials(c.ibkr_account.clone(), c.ibkr_url.clone());
        self.live_deribit
            .update_credentials(c.deribit_id.clone(), c.deribit_sec.clone());
        self.live_decibel.update_credentials(
            c.decibel_key.clone(),
            c.decibel_node_key.clone(),
            c.decibel_wallet.clone(),
            c.decibel_pk.clone(),
        );
        // Long-tail perp venues share the generic credential mapping;
        // each adapter interprets the fields per its own auth model and
        // refuses incomplete material (fail-closed).
        for adapter in &self.live_dex_perps {
            let vid = adapter.spec_id();
            let cred = c.dex_perp.get(vid);
            let nonempty = |s: &String| (!s.trim().is_empty()).then(|| s.clone());
            let nonopt = |o: &Option<String>| o.clone().filter(|s| !s.trim().is_empty());
            let res = adapter.update_credentials(
                cred.and_then(|c| nonempty(&c.api_key)),
                cred.and_then(|c| nonempty(&c.api_secret)),
                cred.and_then(|c| nonopt(&c.passphrase)),
                cred.and_then(|c| nonopt(&c.account_id)),
                cred.and_then(|c| nonopt(&c.wallet_address)),
                cred.and_then(|c| nonopt(&c.private_key)),
            );
            if let Err(e) = res {
                tracing::warn!("{vid} credential sync refused: {e}");
            }
        }
        // Boros: agent key + root address + sub-account id. All three are
        // required; a partial record fails the whole bind (fail closed).
        {
            let cred = c.boros.as_ref();
            let nonopt = |o: &Option<String>| o.clone().filter(|s| !s.trim().is_empty());
            if let Err(e) = self.live_boros.update_credentials(
                cred.and_then(|c| nonopt(&c.private_key)),
                cred.and_then(|c| nonopt(&c.wallet_address)),
                cred.and_then(|c| nonopt(&c.account_id)),
            ) {
                tracing::warn!("Boros credential sync refused: {e}");
            }
        }
        // EVM AMMs: wallet_address + private_key carry the agent wallet;
        // the account `base_url` field doubles as the RPC override so a
        // private endpoint (Alchemy/Infura/self-hosted) replaces the
        // public default. The override is chain-scoped: setting it on any
        // venue of a chain applies it to every same-chain venue lacking its
        // own override (same-chain AMMs share the RPC endpoint).
        let mut chain_rpc: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();
        for adapter in &self.live_evm_amms {
            if let Some(u) = c
                .evm_amm
                .get(adapter.spec_id())
                .and_then(|cr| cr.base_url.clone())
                .filter(|u| !u.trim().is_empty())
            {
                chain_rpc.entry(adapter.chain_id()).or_insert(u);
            }
        }
        for adapter in &self.live_evm_amms {
            let vid = adapter.spec_id();
            let cred = c.evm_amm.get(vid);
            let rpc_override = cred
                .and_then(|c| c.base_url.clone())
                .filter(|u| !u.trim().is_empty())
                .or_else(|| chain_rpc.get(&adapter.chain_id()).cloned());
            // A rejected RPC override withholds this adapter's
            // credentials (fail closed) but never blocks other venues.
            if let Err(e) = adapter.set_base_url(rpc_override) {
                tracing::warn!("{vid} RPC override rejected ({e}) — credentials withheld");
                let _ = adapter.update_credentials(None, None, None, None, None, None);
                continue;
            }
            let nonopt = |o: &Option<String>| o.clone().filter(|s| !s.trim().is_empty());
            // Market-data-only venues never receive a private key —
            // even if a stale vault record still holds one. The wallet
            // address still flows so balance reads can resolve an owner.
            let private_key = if LIVE_EVM_AMM_VENUE_IDS.contains(&vid) {
                cred.and_then(|c| nonopt(&c.private_key))
            } else {
                None
            };
            let res = adapter.update_credentials(
                None,
                None,
                None,
                None,
                cred.and_then(|c| nonopt(&c.wallet_address)),
                private_key,
            );
            if let Err(e) = res {
                tracing::warn!("{vid} credential sync refused: {e}");
            }
            // A sentinel `private_key` parked the adapter waiting for an
            // externally-custodied signer — build one from the loaded
            // stronghold and inject it. No stronghold (or a bad record)
            // leaves the adapter signerless = fail closed.
            if adapter.expects_external_signer() {
                let provisioned = self
                    .evm_stronghold
                    .read()
                    .as_ref()
                    .map(|sh| sh.signer(Some(adapter.chain_id())));
                match provisioned {
                    Some(Ok(signer)) => {
                        if let Err(e) = adapter.provision_external_signer(signer) {
                            tracing::warn!("{vid} external signer refused: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!("{vid} external signer build failed: {e}");
                    }
                    None => tracing::warn!(
                        "{vid} credential record references external key custody but no stronghold is loaded"
                    ),
                }
            }
        }
    }

    /// Provision resolved testnet credentials onto the demo adapter set.
    /// Same extraction rules; testnet accounts provision `tn_*` adapters
    /// only. `popdex` is pinned to its testnet network by construction —
    /// the trading environment is the only network selector.
    fn apply_testnet_credentials(&self, c: &EnvCredentials) {
        self.tn_hl
            .update_credentials(c.hl_wallet.clone(), c.hl_pk.clone());
        self.tn_lighter
            .update_credentials(c.lighter_idx, c.lighter_key.clone());
        self.tn_lighter.set_api_key_index(c.lighter_key_idx);
        self.tn_vanta.update_credentials(
            c.vanta_acc.clone(),
            c.vanta_key.clone(),
            c.vanta_sec.clone(),
        );
        // Robinhood testnet uses the same Lighter api-key tuple as live —
        // env-scoped account rows keep the namespaces separate.
        self.tn_robinhood
            .update_credentials(c.rh_idx, c.rh_key.clone());
        self.tn_robinhood.set_api_key_index(c.rh_key_idx);
        self.tn_arcus.update_credentials(
            c.arcus_key.clone(),
            c.arcus_sec.clone(),
            c.arcus_wallet.clone(),
            c.arcus_subaccount,
        );
        self.tn_binance
            .update_credentials(c.binance_key.clone(), c.binance_sec.clone());
        if let Err(e) = self.tn_okx.set_base_url(c.okx_url.clone()) {
            tracing::warn!("OKX demo base URL override rejected ({e}) — credentials withheld");
            self.tn_okx.update_credentials(None, None, None);
        } else {
            self.tn_okx.update_credentials(
                c.okx_key.clone(),
                c.okx_sec.clone(),
                c.okx_pass.clone(),
            );
        }
        // Alpaca paper keys are a separate namespace minted in the paper
        // dashboard — the same record resolves to the paper host via the
        // env-scoped account, never reusing a live key pair.
        self.tn_alpaca
            .update_credentials(c.alpaca_key.clone(), c.alpaca_sec.clone());
        self.tn_ibkr
            .update_credentials(c.ibkr_account.clone(), c.ibkr_url.clone());
        self.tn_deribit
            .update_credentials(c.deribit_id.clone(), c.deribit_sec.clone());
        self.tn_decibel.update_credentials(
            c.decibel_key.clone(),
            c.decibel_node_key.clone(),
            c.decibel_wallet.clone(),
            c.decibel_pk.clone(),
        );
        if let Err(e) = self.tn_popdex.update_credentials(
            Some("testnet".to_string()),
            c.popdex_wallet.clone(),
            c.popdex_agent.clone(),
        ) {
            tracing::warn!("PopDEX testnet credential sync refused: {e}");
        }
    }

    /// Active trading environment — `Live` production endpoints or `Testnet`
    /// demo endpoints. Everything that resolves venue adapters, account
    /// eligibility, or `is_live` record flags reads this.
    pub fn env(&self) -> TradingEnvironment {
        *self.environment.read()
    }

    /// `true` in Live environment — marks records/balances as real money.
    /// Testnet runs the identical code path against demo endpoints and is
    /// deliberately *not* `is_live` on persisted records.
    pub fn is_live(&self) -> bool {
        self.env() == TradingEnvironment::Live
    }

    pub fn set_environment(&self, env: TradingEnvironment) {
        *self.environment.write() = env;
    }

    /// Mark the breaker's equity universe as already anchored, so the next
    /// risk snapshot *measures* drawdown against the current baseline instead
    /// of re-anchoring. Callers that set a baseline by hand (tests, operators
    /// resetting after review) use this to opt out of the automatic re-anchor
    /// that a paper<->live switch or an account-set change would otherwise
    /// trigger.
    pub fn mark_risk_universe_anchored(&self) {
        *self.last_risk_live_universe.write() = Some(self.is_live());
        *self.last_risk_account_set.write() =
            Some(crate::execution::admitted_risk_account_keys(self));
    }

    /// Admitted venue adapters for one environment. Hyperliquid, Lighter,
    /// Robinhood Lighter, Vanta (Orderly), Aster, PopDEX, Binance, OKX,
    /// Coinbase, Kraken, and Decibel can be enabled for their implemented
    /// market/account paths, and the long-tail perp venues (`live_dex_perps`)
    /// admit when they own an enabled account. Admission is a read/submit
    /// boundary only — it never implies live strategy-entry readiness.
    ///
    /// Under `Testnet`, only venues with a public demo endpoint are eligible
    /// (the `tn_*` set): Coinbase/Aster/Arcus/Robinhood/Polymarket and the
    /// spec-driven long-tail have no usable testnet and are never admitted —
    /// there is no simulated fallback.
    pub fn venues_for_env(
        &self,
        env: TradingEnvironment,
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
        let env_str = env.as_str();
        // A venue is on when its engine flag is set OR it owns an enabled
        // account — the account-zone switch is the same on/off as the flag.
        // Account eligibility is environment-scoped.
        let account_enabled = |venue: &str| {
            self.state_store
                .has_enabled_account(venue, env_str)
                .unwrap_or(false)
        };
        if env == TradingEnvironment::Live {
            if s.hyperliquid_enabled || account_enabled("hyperliquid") {
                list.push(("hyperliquid", "Hyperliquid", self.live_hl.clone()));
            }
            if s.lighter_enabled || account_enabled("lighter") {
                list.push(("lighter", "Lighter", self.live_lighter.clone()));
            }
            if s.robinhood_enabled || account_enabled("robinhood") {
                list.push((
                    "robinhood",
                    "Robinhood Lighter",
                    self.live_robinhood.clone(),
                ));
            }
            if s.vanta_enabled || account_enabled("vanta") {
                list.push(("vanta", "Vanta", self.live_vanta.clone()));
            }
            // Arcus market data is unauthenticated — an enabled venue reads
            // the real CLOB; order paths fail closed until signing lands.
            if s.arcus_enabled || account_enabled("arcus") {
                list.push(("arcus", "Arcus", self.live_arcus.clone()));
            }
            if s.aster_enabled || account_enabled("aster") {
                list.push(("aster", "Aster", self.live_aster.clone()));
            }
            if s.popdex_enabled || account_enabled("popdex") {
                list.push(("popdex", "PopDEX", self.live_popdex.clone()));
            }
            if s.binance_enabled || account_enabled("binance") {
                list.push(("binance", "Binance", self.live_binance.clone()));
            }
            if s.okx_enabled || account_enabled("okx") {
                list.push(("okx", "OKX", self.live_okx.clone()));
            }
            if s.coinbase_enabled || account_enabled("coinbase") {
                list.push(("coinbase", "Coinbase", self.live_coinbase.clone()));
            }
            if s.kraken_enabled
                || s.kraken_futures_enabled
                || s.kraken_spot_enabled
                || account_enabled("kraken")
            {
                list.push(("kraken", "Kraken", self.live_kraken.clone()));
            }
            if s.alpaca_enabled || account_enabled("alpaca") {
                list.push(("alpaca", "Alpaca", self.live_alpaca.clone()));
            }
            if s.ibkr_enabled || account_enabled("ibkr") {
                list.push(("ibkr", "Interactive Brokers", self.live_ibkr.clone()));
            }
            if s.decibel_enabled || account_enabled("decibel") {
                list.push(("decibel", "Decibel", self.live_decibel.clone()));
            }
            if s.deribit_enabled || account_enabled("deribit") {
                list.push(("deribit", "Deribit", self.live_deribit.clone()));
            }
            // Long-tail perp venues have no engine-settings flags — an enabled
            // account is the only admission switch. Capability flags on the
            // adapter still gate every downstream live path.
            for adapter in &self.live_dex_perps {
                if account_enabled(adapter.spec_id()) {
                    list.push((adapter.spec_id(), adapter.spec_name(), adapter.clone()));
                }
            }
            for adapter in &self.live_evm_amms {
                if account_enabled(adapter.spec_id()) {
                    list.push((adapter.spec_id(), adapter.spec_name(), adapter.clone()));
                }
            }
            // Pendle Boros joins only with an enabled account (agent key +
            // root address + accountId) — market data for scanning is
            // fetched separately and needs no credentials.
            if account_enabled("boros") {
                list.push(("boros", "Pendle Boros", self.live_boros.clone()));
            }
            // Variational Omni needs no credentials — a (credential-less)
            // enabled account is the admission switch, like the DexPerp
            // market-data venues.
            if account_enabled("variational") {
                list.push((
                    "variational",
                    "Variational Omni",
                    self.live_variational.clone(),
                ));
            }
        } else {
            // Testnet environment — only venues with public demo endpoints.
            if s.hyperliquid_enabled || account_enabled("hyperliquid") {
                list.push(("hyperliquid", "Hyperliquid", self.tn_hl.clone()));
            }
            if s.lighter_enabled || account_enabled("lighter") {
                list.push(("lighter", "Lighter", self.tn_lighter.clone()));
            }
            if s.robinhood_enabled || account_enabled("robinhood") {
                list.push(("robinhood", "Robinhood Lighter", self.tn_robinhood.clone()));
            }
            if s.vanta_enabled || account_enabled("vanta") {
                list.push(("vanta", "Vanta", self.tn_vanta.clone()));
            }
            if s.arcus_enabled || account_enabled("arcus") {
                list.push(("arcus", "Arcus", self.tn_arcus.clone()));
            }
            if s.popdex_enabled || account_enabled("popdex") {
                list.push(("popdex", "PopDEX", self.tn_popdex.clone()));
            }
            if s.binance_enabled || account_enabled("binance") {
                list.push(("binance", "Binance", self.tn_binance.clone()));
            }
            if s.okx_enabled || account_enabled("okx") {
                list.push(("okx", "OKX", self.tn_okx.clone()));
            }
            if s.alpaca_enabled || account_enabled("alpaca") {
                list.push(("alpaca", "Alpaca", self.tn_alpaca.clone()));
            }
            if s.ibkr_enabled || account_enabled("ibkr") {
                list.push(("ibkr", "Interactive Brokers", self.tn_ibkr.clone()));
            }
            if s.decibel_enabled || account_enabled("decibel") {
                list.push(("decibel", "Decibel", self.tn_decibel.clone()));
            }
            if s.deribit_enabled || account_enabled("deribit") {
                list.push(("deribit", "Deribit", self.tn_deribit.clone()));
            }
            // Prediction venues (kalshi/polymarket) never ride this list —
            // they resolve through the prediction paths, not the perp
            // venue set, in either environment.
        }
        // Tests may inject simulated/scripted adapters per venue — route
        // every consumer (scans, dispatch, monitors) through them so seeded
        // books never resolve to real venue feeds once their freshness
        // window lapses. test_adapters (scripted mocks) win over test_sims.
        #[cfg(test)]
        for entry in &mut list {
            if let Some(a) = self
                .test_adapters
                .read()
                .ok()
                .and_then(|m| m.get(entry.0).cloned())
            {
                entry.2 = a;
            } else if let Some(sim) = self
                .test_sims
                .read()
                .ok()
                .and_then(|m| m.get(entry.0).cloned())
            {
                entry.2 = sim;
            }
        }
        list
    }

    /// Admitted venue adapters for the *active* environment.
    pub fn get_venues(
        &self,
    ) -> Vec<(
        &'static str,
        &'static str,
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    )> {
        self.venues_for_env(self.env())
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
        #[cfg(test)]
        {
            if let Some(a) = self
                .test_adapters
                .read()
                .ok()
                .and_then(|m| m.get(canonical).cloned())
            {
                return Some(a);
            }
            if let Some(sim) = self
                .test_sims
                .read()
                .ok()
                .and_then(|m| m.get(canonical).cloned())
            {
                return Some(sim);
            }
        }
        self.get_venues()
            .into_iter()
            .find(|(vid, _, _)| *vid == canonical)
            .map(|(_, _, adapter)| adapter)
    }

    /// Market listings for one admitted venue, served from the env-scoped
    /// cache when fresh. Poll-frequency callers (scans, strategy tickers,
    /// universe views) must use this instead of `adapter.fetch_markets()` —
    /// several UI surfaces poll on a ~3s cadence and an uncached per-venue
    /// fetch turns every tick into a request storm.
    ///
    /// A failed refresh serves the stale snapshot if one exists (a
    /// connectivity blip doesn't empty the universe); the error propagates
    /// only when nothing was ever learned for this environment.
    pub(crate) async fn cached_markets(
        &self,
        vid: &str,
        adapter: &Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(300);
        const FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
        let key = format!("{}-{vid}", self.env().as_str());
        let fail_key = format!("{key}-markets");
        let stale = self.markets_cache.lock().get(&key).cloned();
        if let Some((at, ref markets)) = stale {
            if at.elapsed() < TTL {
                return Ok(markets.clone());
            }
        }
        if let Some(at) = self.venue_fetch_failures.lock().get(&fail_key) {
            if at.elapsed() < FAIL_COOLDOWN {
                match stale {
                    Some((_, markets)) => return Ok(markets),
                    None => anyhow::bail!("{vid}: markets fetch cooling down after failure"),
                }
            }
        }
        match adapter.fetch_markets().await {
            Ok(markets) => {
                self.venue_fetch_failures.lock().remove(&fail_key);
                self.markets_cache
                    .lock()
                    .insert(key, (std::time::Instant::now(), markets.clone()));
                Ok(markets)
            }
            Err(e) => {
                self.venue_fetch_failures
                    .lock()
                    .insert(fail_key, std::time::Instant::now());
                match stale {
                    Some((_, markets)) => {
                        tracing::warn!("{vid}: markets refresh failed, serving stale cache: {e}");
                        Ok(markets)
                    }
                    None => {
                        tracing::warn!("{vid}: markets fetch failed: {e}");
                        Err(e)
                    }
                }
            }
        }
    }

    /// Drop every cached market/price snapshot and failure stamp for the
    /// active environment. Called by forced watchlist refreshes — a caller
    /// requesting one just changed listing-relevant config, so the cached
    /// answer is exactly what it no longer trusts. Env-scoped: the other
    /// environment's caches stay warm.
    pub(crate) fn invalidate_market_caches(&self) {
        let prefix = format!("{}-", self.env().as_str());
        let mut markets = self.markets_cache.lock();
        markets.retain(|k, _| !k.starts_with(&prefix));
        let mut prices = self.market_prices_cache.lock();
        prices.retain(|k, _| !k.starts_with(&prefix));
        let mut fails = self.venue_fetch_failures.lock();
        fails.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Venue mark prices through the same env-scoped cache contract as
    /// [`Self::cached_markets`]. Prices move faster than listings, so the
    /// TTL is shorter; callers must only need threshold-grade marks (any
    /// consumer pricing orders uses the order book, never this).
    pub(crate) async fn cached_market_prices(
        &self,
        vid: &str,
        adapter: &Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    ) -> anyhow::Result<std::collections::HashMap<String, Decimal>> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(60);
        const FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
        let key = format!("{}-{vid}", self.env().as_str());
        let fail_key = format!("{key}-prices");
        let stale = self.market_prices_cache.lock().get(&key).cloned();
        if let Some((at, ref prices)) = stale {
            if at.elapsed() < TTL {
                return Ok(prices.clone());
            }
        }
        if let Some(at) = self.venue_fetch_failures.lock().get(&fail_key) {
            if at.elapsed() < FAIL_COOLDOWN {
                match stale {
                    Some((_, prices)) => return Ok(prices),
                    None => anyhow::bail!("{vid}: market prices fetch cooling down after failure"),
                }
            }
        }
        match adapter.fetch_market_prices().await {
            Ok(prices) => {
                self.venue_fetch_failures.lock().remove(&fail_key);
                self.market_prices_cache
                    .lock()
                    .insert(key, (std::time::Instant::now(), prices.clone()));
                Ok(prices)
            }
            Err(e) => {
                self.venue_fetch_failures
                    .lock()
                    .insert(fail_key, std::time::Instant::now());
                match stale {
                    Some((_, prices)) => {
                        tracing::warn!(
                            "{vid}: market prices refresh failed, serving stale cache: {e}"
                        );
                        Ok(prices)
                    }
                    None => {
                        tracing::warn!("{vid}: market prices fetch failed: {e}");
                        Err(e)
                    }
                }
            }
        }
    }

    /// Install a zero-latency `SimulatedExchange` as a venue's adapter for
    /// tests. Registers the concrete handle in `test_sims` so book-seeding
    /// helpers can mirror hub writes into it, keeping every book-resolution
    /// path deterministic regardless of wall-clock freshness windows.
    /// Returns the concrete sim so tests can also script orders/fills.
    #[cfg(test)]
    pub fn install_test_sim(
        &self,
        venue_id: &str,
    ) -> Option<Arc<arbitrageur_exchanges::simulated::SimulatedExchange>> {
        let canonical = canonical_venue_id(venue_id)?;
        // Match the old built-in sims' fee schedule so PnL assertions that
        // assume nonzero taker fees keep their meaning.
        let fee_bps = match canonical {
            "lighter" => dec!(1.5),
            "vanta" => dec!(3.0),
            "robinhood" => dec!(2.5),
            _ => dec!(2.0),
        };
        let sim = Arc::new(arbitrageur_exchanges::simulated::SimulatedExchange::new(
            canonical,
            canonical,
            0,
            fee_bps,
            rust_decimal::Decimal::new(100_000, 0),
        ));
        self.test_sims
            .write()
            .ok()?
            .insert(canonical.to_string(), sim.clone());
        Some(sim)
    }

    /// Test-only access to an installed sim handle for seeding books/orders.
    /// There is no paper exchange in production — under Testnet mode the
    /// `tn_*` adapters talk to real venue demo endpoints.
    #[cfg(test)]
    pub fn test_sim(
        &self,
        venue_id: &str,
    ) -> Option<Arc<arbitrageur_exchanges::simulated::SimulatedExchange>> {
        let canonical = canonical_venue_id(venue_id)?;
        self.test_sims.read().ok()?.get(canonical).cloned()
    }

    /// Test-only alias: the installed sims stand in for what used to be the
    /// paper exchange set. Production code never calls this.
    #[cfg(test)]
    pub fn paper_exchanges(
        &self,
    ) -> Vec<(
        String,
        Arc<arbitrageur_exchanges::simulated::SimulatedExchange>,
    )> {
        self.test_sims
            .read()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Test-only alias for `test_sim` under the old name.
    #[cfg(test)]
    pub fn get_paper_exchange(
        &self,
        venue_id: &str,
    ) -> Option<Arc<arbitrageur_exchanges::simulated::SimulatedExchange>> {
        self.test_sim(venue_id)
    }

    /// Test-only baseline: sum of starting equities across installed sims
    /// (the figure `total_realized_profit` is diffed against in sim tests).
    #[cfg(test)]
    pub fn paper_baseline_equity(&self) -> Decimal {
        self.test_sims
            .read()
            .map(|m| m.values().map(|s| s.initial_equity()).sum::<Decimal>())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cb_salvage_preserves_trip_across_decimal_schema_drift() {
        // Old-format row: decimals serialized as maps — the strict
        // CircuitBreakerSnapshot parse fails, but the halt must survive.
        let drifted = serde_json::json!({
            "tripped": true,
            "trip_reason": "daily drawdown 6.2% exceeded 5.0%",
            "tripped_at": "2026-09-20T14:33:00Z",
            "day_start_equity": {"mantissa": 11_500_000, "exponent": -2},
            "current_equity": {"mantissa": 10_800_000, "exponent": -2},
            "peak_equity": {"mantissa": 11_600_000, "exponent": -2},
            "day_key": "2026-09-20",
            "consecutive_errors": 7
        })
        .to_string();
        // Confirm the strict parse really does fail for this shape.
        assert!(
            serde_json::from_str::<arbitrageur_risk::circuit_breaker::CircuitBreakerSnapshot>(
                &drifted
            )
            .is_err()
        );
        let snap = salvage_tripped_snapshot(&drifted).expect("trip must survive");
        assert!(snap.tripped);
        assert_eq!(
            snap.trip_reason.as_deref(),
            Some("daily drawdown 6.2% exceeded 5.0%")
        );
        assert!(snap.tripped_at.is_some());

        // A not-tripped drifted row stays None — nothing to preserve.
        let quiet = serde_json::json!({
            "tripped": false,
            "day_start_equity": {"mantissa": 1, "exponent": 0}
        })
        .to_string();
        assert!(salvage_tripped_snapshot(&quiet).is_none());

        // Garbage is not a trip.
        assert!(salvage_tripped_snapshot("not json").is_none());
        assert!(salvage_tripped_snapshot("{}").is_none());
    }

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
        let p: StrategyParamsDto = serde_json::from_value(old).expect("old rows must still load");
        assert_eq!(p.step_size_mult, 10.0);
        assert_eq!(p.max_step_usd, 2500.0);
        assert_eq!(p.max_ticker_equity_pct, 20.0);
        assert_eq!(p.strategy_id, "futures");
        assert_eq!(p.futures_max_slippage_bps, 15.0);
        assert_eq!(p.funding_max_slippage_bps, 15.0);
        assert_eq!(p.prediction_max_slippage_bps, 15.0);
        assert_eq!(p.funding_max_entry_spread_bps, 8.0);
        assert_eq!(p.funding_auto_exit_spread_pct, None);
        assert_eq!(p.funding_liq_trim_pct, None);
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
        assert!(!state.is_live());
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
            s.popdex_enabled = true;
        }
        // Testnet environment (default): only venues with a public demo
        // endpoint are admitted — aster/polymarket/kalshi have none and
        // stay ineligible rather than being simulated. Robinhood resolves
        // to its testnet Lighter deployment; Arcus to its testnet /v1 API.
        let venues_testnet = state.get_venues();
        assert_eq!(venues_testnet.len(), 6);
        assert_eq!(venues_testnet[0].0, "hyperliquid");
        assert_eq!(venues_testnet[1].0, "lighter");
        assert_eq!(venues_testnet[2].0, "robinhood");
        assert_eq!(venues_testnet[3].0, "vanta");
        assert_eq!(venues_testnet[4].0, "arcus");
        assert_eq!(venues_testnet[5].0, "popdex");

        assert!(state.get_adapter("hyperliquid").is_some());
        assert!(state.get_adapter("lighter").is_some());
        assert!(state.get_adapter("robinhood").is_some());
        assert!(state.get_adapter("vanta").is_some());
        assert!(state.get_adapter("aster").is_none());
        assert!(state.get_adapter("arcus").is_some());
        // Removed venues without native trading API stay unresolvable.
        assert!(state.get_adapter("popdex").is_some());
        assert!(state.get_adapter("ondoperps").is_none());
        assert!(state.get_adapter("ondo").is_none());
        assert!(state.get_adapter("grvt").is_none());
        assert!(state.get_adapter("edgex").is_none());
        assert!(state.get_adapter("variational").is_none());
        assert!(state.get_adapter("dydx").is_none());
        assert!(state.get_adapter("polymarket").is_none());
        assert!(state.get_adapter("kalshi").is_none());
        assert!(state.get_adapter("nonexistent").is_none());
        // Test sims resolve only after explicit install.
        assert!(state.get_paper_exchange("aster").is_none());
        assert!(state.get_paper_exchange("ondoperps").is_none());
        assert!(state.get_paper_exchange("grvt").is_none());
        assert!(state.get_paper_exchange("edgex").is_none());

        state.set_environment(crate::state::TradingEnvironment::Live);
        // In live mode, enabled venues remain admitted for market data streaming,
        // while order execution is selectively gated by adapter capability & credentials.
        assert!(!state.get_venues().is_empty());
        assert!(state
            .get_adapter("hyperliquid")
            .unwrap()
            .supports_live_execution());
        assert!(state
            .get_adapter("aster")
            .unwrap()
            .supports_live_execution());
        assert!(state
            .get_adapter("vanta")
            .unwrap()
            .supports_live_execution());
        assert!(!state
            .get_adapter("hyperliquid")
            .unwrap()
            .has_live_credentials());
    }

    #[tokio::test]
    async fn test_canonical_venue_resolution_rejects_near_matches() {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
        }
        state.install_test_sim("hyperliquid");
        state.install_test_sim("lighter");

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
                "sim alias '{}' must resolve",
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
                "'{}' must not resolve to a sim exchange",
                bad
            );
        }

        // Sims are installed per venue — disabling the flag removes the
        // adapter but the registered sim handle stays resolvable (tests
        // control admission via install_test_sim, not venue flags).
        {
            let mut s = state.settings.write();
            s.lighter_enabled = false;
        }
        assert!(state.get_adapter("lighter").is_some());
        assert!(state.get_paper_exchange("lighter").is_some());
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
        // #43: raw telemetry retains 14 days by default — None let the
        // state DB grow without bound.
        assert_eq!(s.spread_samples_max_days, Some(14));
    }

    #[test]
    fn test_exec_policy_settings_defaults_on_and_persisted_overrides() {
        let store = crate::state_store::StateStore::open_in_memory().unwrap();

        // No persisted keys → code default: passive unwind ON.
        let mut s = EngineSettingsDto::default();
        assert!(s.passive_unwind.enabled);
        load_exec_policy_settings(&store, &mut s);
        assert!(s.passive_unwind.enabled);

        // Persisted operator overrides win over the defaults.
        let cfg = PassiveUnwindConfig {
            enabled: false,
            timeout_secs: 42,
            ..PassiveUnwindConfig::default()
        };
        store
            .kv_set(
                "engine.passive_unwind",
                &serde_json::to_string(&cfg).unwrap(),
            )
            .unwrap();
        let mut s = EngineSettingsDto::default();
        load_exec_policy_settings(&store, &mut s);
        assert!(!s.passive_unwind.enabled);
        assert_eq!(s.passive_unwind.timeout_secs, 42);

        // Corrupt/invalid blobs are ignored fail-closed (defaults kept).
        store
            .kv_set(
                "engine.passive_unwind",
                "{\"enabled\":false,\"maxTries\":99}",
            )
            .unwrap();
        let mut s = EngineSettingsDto::default();
        load_exec_policy_settings(&store, &mut s);
        assert!(s.passive_unwind.enabled);
    }

    #[test]
    fn usd_quote_policy_defaults_and_kv_roundtrip() {
        // Fresh defaults: FX-adjusted mode, 50bps warn / 100bps halt.
        let state = AppEngineState::new();
        {
            let s = state.settings.read();
            assert_eq!(s.usd_quote_mode, "fx_adjusted");
            assert_eq!(s.stablecoin_warn_bps, 50.0);
            assert_eq!(s.stablecoin_halt_bps, 100.0);
        }
        assert!(state.usd_merge_quotes());

        // Persisted overrides reload on the next boot over the same store.
        let store = state.state_store.clone();
        store.kv_set("engine.usd_quote_mode", "separate").unwrap();
        store.kv_set("risk.stablecoin_warn_bps", "25").unwrap();
        store.kv_set("risk.stablecoin_halt_bps", "75").unwrap();
        let reloaded =
            AppEngineState::new_inner(PathBuf::from("/nonexistent-vault"), store.clone());
        {
            let s = reloaded.settings.read();
            assert_eq!(s.usd_quote_mode, "separate");
            assert_eq!(s.stablecoin_warn_bps, 25.0);
            assert_eq!(s.stablecoin_halt_bps, 75.0);
        }
        assert!(!reloaded.usd_merge_quotes());

        // Corrupt/inverted values fall back to code defaults.
        store.kv_set("engine.usd_quote_mode", "bogus").unwrap();
        store.kv_set("risk.stablecoin_warn_bps", "500").unwrap();
        store.kv_set("risk.stablecoin_halt_bps", "100").unwrap();
        let reloaded =
            AppEngineState::new_inner(PathBuf::from("/nonexistent-vault"), store.clone());
        let s = reloaded.settings.read();
        assert_eq!(s.usd_quote_mode, "fx_adjusted");
        assert_eq!(s.stablecoin_warn_bps, 50.0);
        assert_eq!(s.stablecoin_halt_bps, 100.0);
    }

    #[test]
    fn stablecoin_gate_blocks_depeg_exempts_stable_arb() {
        let state = AppEngineState::new();
        let now = chrono::Utc::now().timestamp_millis();
        // USDT at $0.98 — 200bps deviation, past the default 100bps halt.
        state.stablecoin_fx.record_global("USDT", 0.98, "test", now);
        // Ordinary books priced in the halted coin are refused.
        assert!(state.stablecoin_gate("BTC", "USDT").is_err());
        assert!(state.stablecoin_gate("BTC-USDT", "USDT").is_err());
        // Stable-vs-stable / stable-vs-USD books ARE the depeg arbitrage —
        // they stay tradeable.
        assert!(state.stablecoin_gate("USDT", "USDC").is_ok());
        assert!(state.stablecoin_gate("USDC-USDT", "USDT").is_ok());
        // Non-family quotes are never gated (they never merged into USD).
        assert!(state.stablecoin_gate("BTC", "EUR").is_ok());
        // A fine quote doesn't gate either.
        assert!(state.stablecoin_gate("BTC", "USDC").is_ok());
        // Below the halt threshold the gate re-opens.
        state
            .stablecoin_fx
            .record_global("USDT", 0.999, "test", now);
        assert!(state.stablecoin_gate("BTC", "USDT").is_ok());
    }

    #[test]
    fn normalize_account_usd_scales_settlement_and_positions() {
        use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
        use arbitrageur_core::types::Side;

        let state = AppEngineState::new();
        // The monitor's settlement map is what venue_settlement_quote reads —
        // inject a lane-less venue settling in USDT.
        let engine = crate::monitor::SpreadMonitorEngine::new(&state);
        engine.set_settlement_quote("binance", "USDT");
        *state.monitor.lock() = Some(engine);
        let now = chrono::Utc::now().timestamp_millis();
        // Venue-local rate (single-venue truth): USDT = $0.98 on binance.
        state
            .stablecoin_fx
            .record_venue("binance", "USDT", 0.98, now);

        let mut acc = AccountSnapshot {
            venue: "binance".into(),
            equity: dec!(1000),
            free_margin: dec!(500),
            maintenance_margin: dec!(100),
            margin_usage_pct: dec!(10),
            balances: vec![
                Balance {
                    venue: "binance".into(),
                    currency: "USDT".into(),
                    total: dec!(1000),
                    free: dec!(1000),
                    locked: dec!(0),
                },
                // Non-USD balances stay in native units — converting them
                // needs an asset price, not a stablecoin rate.
                Balance {
                    venue: "binance".into(),
                    currency: "BTC".into(),
                    total: dec!(2),
                    free: dec!(2),
                    locked: dec!(0),
                },
            ],
            positions: vec![Position {
                venue: "binance".into(),
                symbol: "BTC-USDT".into(),
                side: Side::Buy,
                quantity: dec!(1),
                entry_price: dec!(100),
                mark_price: dec!(100),
                unrealized_pnl: dec!(10),
                realized_pnl: dec!(0),
                liquidation_price: Some(dec!(50)),
                leverage: dec!(1),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: chrono::Utc::now(),
            }],
            timestamp: chrono::Utc::now(),
        };
        state.normalize_account_usd("binance", &mut acc);
        // The rate crosses f64 → Decimal: compare within a cent of a cent.
        let close = |a: Decimal, b: f64| {
            (a - Decimal::from_f64_retain(b).unwrap()).abs()
                < Decimal::from_f64_retain(1e-6).unwrap()
        };
        assert!(close(acc.equity, 980.0), "equity {}", acc.equity);
        assert!(close(acc.free_margin, 490.0));
        assert!(close(acc.maintenance_margin, 98.0));
        // Position prices scale by the symbol's own quote (USDT).
        assert!(close(acc.positions[0].mark_price, 98.0));
        assert!(close(acc.positions[0].unrealized_pnl, 9.8));
        assert_eq!(
            acc.positions[0].liquidation_price.map(|l| close(l, 49.0)),
            Some(true)
        );
        assert!(close(acc.balances[0].total, 980.0));
        assert_eq!(acc.balances[1].total, dec!(2));
        // Unitless ratios stay untouched.
        assert_eq!(acc.margin_usage_pct, dec!(10));

        // Separate mode disables conversion entirely.
        state.settings.write().usd_quote_mode = "separate".into();
        let mut acc2 = acc.clone();
        // reset to face values
        acc2.equity = dec!(1000);
        state.normalize_account_usd("binance", &mut acc2);
        assert_eq!(acc2.equity, dec!(1000));
    }

    #[test]
    fn production_adapter_resolution_never_yields_a_simulation() {
        // SimulatedExchange exists only behind #[cfg(test)] registries and
        // the explicit `simulate` CLI subcommand — the production venue
        // registry must resolve to the pinned concrete adapters (endpoint-
        // separated by environment), never a fabricated market.
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.lighter_enabled = true;
            s.binance_enabled = true;
        }

        // Testnet: resolution returns the pinned tn_* adapter itself.
        let tn = state.venues_for_env(TradingEnvironment::Testnet);
        let (vid, _name, adapter) = tn.iter().find(|(v, _, _)| *v == "lighter").unwrap();
        assert_eq!(*vid, "lighter");
        assert!(
            Arc::ptr_eq(
                adapter,
                &(state.tn_lighter.clone() as Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>)
            ) || adapter
                .rest_base_url()
                .map(|u| u.contains("testnet.zklighter"))
                .unwrap_or(false),
            "testnet lighter must resolve to the pinned testnet adapter, got {:?}",
            adapter.rest_base_url()
        );
        assert!(
            adapter
                .rest_base_url()
                .map(|u| u.contains("testnet.zklighter"))
                .unwrap_or(false),
            "testnet adapter must use the venue demo endpoint"
        );

        // Live: the same venue resolves to a DIFFERENT pinned adapter on
        // the mainnet endpoint — environments never cross.
        let live = state.venues_for_env(TradingEnvironment::Live);
        let (_, _, live_adapter) = live.iter().find(|(v, _, _)| *v == "lighter").unwrap();
        assert!(
            live_adapter
                .rest_base_url()
                .map(|u| u.contains("mainnet.zklighter"))
                .unwrap_or(false),
            "live adapter must use the mainnet endpoint: {:?}",
            live_adapter.rest_base_url()
        );
        assert!(!std::ptr::eq(
            Arc::as_ptr(adapter),
            Arc::as_ptr(live_adapter)
        ));

        // A fresh state with nothing enabled admits and resolves nothing —
        // there is no silent simulated fallback anywhere in resolution.
        let empty = AppEngineState::new();
        assert!(empty.get_venues().is_empty());
        for v in [
            "lighter",
            "binance",
            "hyperliquid",
            "vanta",
            "okx",
            "kraken",
        ] {
            assert!(empty.get_adapter(v).is_none(), "{v} resolved unadmitted");
        }
        // Near-matches and junk never canonicalize to a venue either.
        for junk in ["lighte", "lighter2", "", " hyper liquid ", "lighters"] {
            assert!(
                empty.get_adapter(junk).is_none(),
                "{junk:?} must not resolve"
            );
        }
    }

    #[test]
    fn kraken_is_live_only_and_ineligible_in_testnet() {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.kraken_enabled = true;
            s.kraken_futures_enabled = true;
        }
        // Testnet: Kraken must not be admitted because it has no public testnet.
        let tn_venues = state.venues_for_env(TradingEnvironment::Testnet);
        assert!(
            !tn_venues.iter().any(|(v, _, _)| *v == "kraken"),
            "Kraken must never be admitted in Testnet mode"
        );

        // Live: Kraken must be admitted when enabled.
        let live_venues = state.venues_for_env(TradingEnvironment::Live);
        assert!(
            live_venues.iter().any(|(v, _, _)| *v == "kraken"),
            "Kraken must be admitted in Live mode when enabled"
        );
    }
}
