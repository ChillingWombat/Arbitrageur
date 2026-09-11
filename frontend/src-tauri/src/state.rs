use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use arbitrager_core::hub::MarketDataHub;
use arbitrager_core::types::{dec, Decimal};
use arbitrager_exchanges::arcus::ArcusAdapter;
use arbitrager_exchanges::hyperliquid::HyperliquidAdapter;
use arbitrager_exchanges::lighter::LighterAdapter;
use arbitrager_exchanges::popdex::PopDexAdapter;
use arbitrager_exchanges::prediction::{KalshiAdapter, PolymarketAdapter};
use arbitrager_exchanges::robinhood::RobinhoodAdapter;
use arbitrager_exchanges::simulated::SimulatedExchange;
use arbitrager_exchanges::vanta::VantaAdapter;
use arbitrager_execution::coordinator::AtomicArbitrageCoordinator;
use arbitrager_execution::smart_routing::SmartOrderRouter;
use arbitrager_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrager_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use arbitrager_risk::delta_guard::DeltaGuard;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ExecutionLogEntry {
    pub id: String,
    pub timestamp: String,
    pub trade_type: String, // "Spatial" or "Funding" or "Prediction"
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
}

impl Default for TickerStrategyConfigDto {
    fn default() -> Self {
        Self {
            symbol: "BTC-USDC".into(),
            target_profit_bps: 5.0,
            window_percentile: 50,
            max_accumulated_units: 3,
            current_accumulated_units: 0,
            execution_mode: "SINGLE_WAY".into(),
            unit_size: 0.005,
            user_target_leverage: 5,
        }
    }
}

fn default_hsm_url() -> String { "http://127.0.0.1:8787/sign".into() }
fn default_hmac_token() -> String { "••••••••••••••••••••••••".into() }
fn default_max_slippage() -> f64 { 15.0 }
fn default_min_depth_ratio() -> f64 { 3.0 }
fn default_hyperliquid_url() -> String { "https://api.hyperliquid.xyz".into() }
fn default_lighter_account_index() -> String { "0".into() }
fn default_lighter_url() -> String { "https://mainnet.zklighter.elliot.ai".into() }
fn default_vanta_url() -> String { "https://api-evm.orderly.org".into() }
fn default_robinhood_rpc_url() -> String { "https://rpc.robinhood.network".into() }
fn default_robinhood_router() -> String { "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba".into() }
fn default_polymarket_url() -> String { "https://clob.polymarket.com".into() }
fn default_kalshi_url() -> String { "https://trading-api.kalshi.com".into() }
fn default_engine_rpc_port() -> u16 { 8787 }
fn default_engine_ws_port() -> u16 { 8789 }
fn default_bind_address() -> String { "127.0.0.1".into() }
fn default_trading_mode() -> String { "paper".into() }
fn default_fast_warmup_enabled() -> bool { true }
fn default_venue_rebalance_threshold_usd() -> f64 { 500.0 }
fn default_referral_lighter() -> String { "https://app.lighter.xyz/?referral=SOLO&source=none".into() }
fn default_referral_robinhood() -> String { "https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none".into() }
fn default_sponsor_link() -> String { "https://buymeacoffee.com/dav1d1010".into() }
fn default_app_version() -> String { "v1.2.0".into() }
fn default_git_repo() -> String { "https://github.com/arbitrager/arbitrager".into() }
fn default_true() -> bool { true }
fn default_kraken_spot_url() -> String { "https://api.kraken.com".into() }
fn default_kraken_futures_url() -> String { "https://futures.kraken.com".into() }
fn default_arcus_url() -> String { "https://api.arcus.exchange".into() }
fn default_popdex_url() -> String { "https://api.popdex.trade".into() }
fn default_popdex_account_index() -> String { "0".into() }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct VenueBalanceInfo {
    pub venue: String,
    pub balance_usd: f64,
    pub min_required_usd: f64,
    pub needs_rebalance: bool,
    pub status: String, // "HEALTHY", "LOW_MARGIN", "DEPLETED"
    pub recommended_action: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct EngineSettingsDto {
    #[serde(default = "default_hsm_url")]
    pub hsm_url: String,
    #[serde(default = "default_hmac_token")]
    pub hmac_token: String,
    #[serde(default = "default_max_slippage")]
    pub max_slippage_bps: f64,
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
    #[serde(default = "default_fast_warmup_enabled")]
    pub fast_warmup_enabled: bool,
    #[serde(default = "default_venue_rebalance_threshold_usd")]
    pub venue_rebalance_threshold_usd: f64,

    // Exchange Enablement Toggles
    #[serde(default = "default_true")]
    pub hyperliquid_enabled: bool,
    #[serde(default = "default_true")]
    pub hyperliquid_spot_enabled: bool,
    #[serde(default = "default_true")]
    pub lighter_enabled: bool,
    #[serde(default = "default_true")]
    pub lighter_spot_enabled: bool,
    #[serde(default = "default_true")]
    pub vanta_enabled: bool,
    #[serde(default = "default_true")]
    pub robinhood_enabled: bool,
    #[serde(default = "default_true")]
    pub arcus_enabled: bool,
    #[serde(default = "default_true")]
    pub popdex_enabled: bool,
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
    pub hyperliquid_account_index: Option<u32>,
    #[serde(default)]
    pub hyperliquid_subaccount_index: Option<u32>,

    // Lighter zkDEX
    #[serde(default = "default_lighter_account_index")]
    pub lighter_account_index: String,
    #[serde(default)]
    pub lighter_api_key: String,
    #[serde(default = "default_lighter_url")]
    pub lighter_url: String,
    #[serde(default)]
    pub lighter_subaccount_index: Option<u32>,

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
    pub vanta_subaccount_index: Option<u32>,

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
    pub robinhood_subaccount_index: Option<u32>,

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

    // PopDEX Protocol
    #[serde(default = "default_popdex_account_index")]
    pub popdex_account_index: String,
    #[serde(default)]
    pub popdex_api_key: String,
    #[serde(default = "default_popdex_url")]
    pub popdex_url: String,

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
            max_slippage_bps: default_max_slippage(),
            min_depth_ratio: default_min_depth_ratio(),
            engine_rpc_port: default_engine_rpc_port(),
            engine_ws_port: default_engine_ws_port(),
            bind_address: default_bind_address(),
            trading_mode: default_trading_mode(),
            fast_warmup_enabled: default_fast_warmup_enabled(),
            venue_rebalance_threshold_usd: default_venue_rebalance_threshold_usd(),
            hyperliquid_enabled: false,
            hyperliquid_spot_enabled: false,
            lighter_enabled: false,
            lighter_spot_enabled: false,
            vanta_enabled: false,
            robinhood_enabled: false,
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
            popdex_enabled: false,
            popdex_account_index: default_popdex_account_index(),
            popdex_api_key: "".into(),
            popdex_url: default_popdex_url(),
            referral_lighter: default_referral_lighter(),
            referral_robinhood: default_referral_robinhood(),
            sponsor_link: default_sponsor_link(),
            version: default_app_version(),
            git_repo: default_git_repo(),
        }
    }
}

#[allow(dead_code)]
pub struct AppEngineState {
    pub ex_hl: Arc<SimulatedExchange>,
    pub ex_lighter: Arc<SimulatedExchange>,
    pub ex_vanta: Arc<SimulatedExchange>,
    pub ex_robinhood: Arc<SimulatedExchange>,
    pub ex_arcus: Arc<SimulatedExchange>,
    pub ex_popdex: Arc<SimulatedExchange>,
    pub live_hl: Arc<HyperliquidAdapter>,
    pub live_lighter: Arc<LighterAdapter>,
    pub live_vanta: Arc<VantaAdapter>,
    pub live_robinhood: Arc<RobinhoodAdapter>,
    pub live_arcus: Arc<ArcusAdapter>,
    pub live_popdex: Arc<PopDexAdapter>,
    pub live_polymarket: Arc<PolymarketAdapter>,
    pub live_kalshi: Arc<KalshiAdapter>,
    pub market_data_hub: Arc<MarketDataHub>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub delta_guard: RwLock<DeltaGuard>,
    pub coordinator: Arc<AtomicArbitrageCoordinator>,
    pub kalman_filter: RwLock<HedgeRatioKalmanFilter>,
    pub is_live_mode: RwLock<bool>,
    pub total_realized_profit: RwLock<Decimal>,
    pub execution_logs: RwLock<Vec<ExecutionLogEntry>>,
    pub sim_step_counter: RwLock<usize>,
    pub settings: RwLock<EngineSettingsDto>,
    pub strategy_configs: RwLock<std::collections::HashMap<String, TickerStrategyConfigDto>>,
    pub vault: RwLock<crate::vault::SecureVault>,
    pub virtual_ledger: RwLock<arbitrager_execution::virtual_ledger::VirtualStrategyLedger>,
    pub is_paused: RwLock<bool>,
}

impl AppEngineState {
    pub fn new() -> Self {
        let ex_hl = Arc::new(SimulatedExchange::new("hyperliquid", "Hyperliquid (L1)", 2, dec!(2.0), dec!(100000.0)));
        let ex_lighter = Arc::new(SimulatedExchange::new("lighter", "Lighter (zkDEX)", 5, dec!(1.5), dec!(100000.0)));
        let ex_vanta = Arc::new(SimulatedExchange::new("vanta", "Vanta.exchange", 4, dec!(3.0), dec!(100000.0)));
        let ex_robinhood = Arc::new(SimulatedExchange::new("robinhood", "Robinhood EVM", 8, dec!(2.5), dec!(100000.0)));
        let ex_arcus = Arc::new(SimulatedExchange::new("arcus", "Arcus", 3, dec!(1.5), dec!(100000.0)));
        let ex_popdex = Arc::new(SimulatedExchange::new("popdex", "PopDEX", 4, dec!(2.0), dec!(100000.0)));

        // No synthetic market seeding. All market quotes and orderbooks are fetched dynamically
        // from live venue APIs once API credentials are configured by the user.

        let live_hl = Arc::new(HyperliquidAdapter::new(None, None, None));
        let live_lighter = Arc::new(LighterAdapter::new(None, None, None));
        let live_vanta = Arc::new(VantaAdapter::new(None, None, None, None));
        let live_robinhood = Arc::new(RobinhoodAdapter::new(None, None, None, None));
        let live_arcus = Arc::new(ArcusAdapter::new(None, None, None));
        let live_popdex = Arc::new(PopDexAdapter::new(None, None, None));
        let live_polymarket = Arc::new(PolymarketAdapter::new(None, None));
        let live_kalshi = Arc::new(KalshiAdapter::new(None, None));
        let market_data_hub = MarketDataHub::new(1024, 400);

        let cb_config = CircuitBreakerConfig {
            max_daily_drawdown_usd: dec!(25000.0),
            max_daily_drawdown_pct: dec!(5.0),
            max_consecutive_errors: 5,
            max_spread_divergence_bps: dec!(250.0),
        };
        let cb = Arc::new(CircuitBreaker::new(cb_config, dec!(400000.0)));
        let delta_guard = RwLock::new(DeltaGuard::new(dec!(2500.0)));
        let router = SmartOrderRouter::new(dec!(15.0)); // 15 bps slippage tolerance
        let coordinator = Arc::new(AtomicArbitrageCoordinator::new(router, cb.clone()));
        let kalman_filter = RwLock::new(HedgeRatioKalmanFilter::new(1e-4, 1e-2));

        let settings = RwLock::new(EngineSettingsDto::default());

        let initial_logs: Vec<ExecutionLogEntry> = Vec::new();
        let vault = RwLock::new(crate::vault::SecureVault::new_with_path(crate::vault::SecureVault::default_path()));
        let virtual_ledger = RwLock::new(arbitrager_execution::virtual_ledger::VirtualStrategyLedger::new());

        Self {
            ex_hl,
            ex_lighter,
            ex_vanta,
            ex_robinhood,
            ex_arcus,
            ex_popdex,
            live_hl,
            live_lighter,
            live_vanta,
            live_robinhood,
            live_arcus,
            live_popdex,
            live_polymarket,
            live_kalshi,
            market_data_hub,
            circuit_breaker: cb,
            delta_guard,
            coordinator,
            kalman_filter,
            is_live_mode: RwLock::new(false),
            total_realized_profit: RwLock::new(dec!(0)),
            execution_logs: RwLock::new(initial_logs),
            sim_step_counter: RwLock::new(8),
            settings,
            strategy_configs: RwLock::new(std::collections::HashMap::new()),
            vault,
            virtual_ledger,
            is_paused: RwLock::new(false),
        }
    }

    /// Ensure market is loaded - live real market data is queried from active exchange APIs
    pub fn ensure_market(&self, _symbol: &str) {
        // No-op: Real market data is pulled directly from configured live exchange APIs only.
        // No synthetic markets are ever seeded or fabricated.
    }

    /// Returns active venue list using live market adapters, strictly filtered by enabled & connected credentials
    pub fn get_venues(&self) -> Vec<(&'static str, &'static str, Arc<dyn arbitrager_core::exchange::ExchangeAdapter>)> {
        let is_live = *self.is_live_mode.read();
        let s = self.settings.read();
        let mut list: Vec<(&'static str, &'static str, Arc<dyn arbitrager_core::exchange::ExchangeAdapter>)> = Vec::new();

        if s.hyperliquid_enabled && (!s.hyperliquid_wallet.trim().is_empty() || !s.hyperliquid_private_key.trim().is_empty()) {
            list.push(("hyperliquid", if is_live { "Hyperliquid (L1 DEX)" } else { "Hyperliquid (L1)" }, self.live_hl.clone()));
        }
        if s.lighter_enabled && (!s.lighter_api_key.trim().is_empty() || (!s.lighter_account_index.trim().is_empty() && s.lighter_account_index.trim() != "0")) {
            list.push(("lighter", if is_live { "Lighter (zkDEX)" } else { "Lighter (zkDEX)" }, self.live_lighter.clone()));
        }
        if s.vanta_enabled && (!s.vanta_orderly_key.trim().is_empty() || !s.vanta_account_id.trim().is_empty()) {
            list.push(("vanta", if is_live { "Vanta.exchange" } else { "Vanta.exchange" }, self.live_vanta.clone()));
        }
        if s.robinhood_enabled && (!s.robinhood_wallet.trim().is_empty() || !s.robinhood_private_key.trim().is_empty()) {
            list.push(("robinhood", if is_live { "Robinhood EVM" } else { "Robinhood EVM" }, self.live_robinhood.clone()));
        }
        if s.arcus_enabled && !s.arcus_api_key.trim().is_empty() {
            list.push(("arcus", if is_live { "Arcus (CLOB DEX)" } else { "Arcus" }, self.live_arcus.clone()));
        }
        if s.popdex_enabled && !s.popdex_api_key.trim().is_empty() {
            list.push(("popdex", if is_live { "PopDEX (Arbitrage Protocol)" } else { "PopDEX" }, self.live_popdex.clone()));
        }

        list
    }

    /// Resolves an exchange adapter by venue identifier or name using live market adapters
    pub fn get_adapter(&self, venue_id: &str) -> Option<Arc<dyn arbitrager_core::exchange::ExchangeAdapter>> {
        let id_clean = venue_id.to_lowercase();
        if id_clean.contains("hyperliquid") || id_clean == "hl" {
            Some(self.live_hl.clone())
        } else if id_clean.contains("lighter") || id_clean == "lgt" {
            Some(self.live_lighter.clone())
        } else if id_clean.contains("vanta") || id_clean == "vnt" {
            Some(self.live_vanta.clone())
        } else if id_clean.contains("robinhood") || id_clean == "rh" {
            Some(self.live_robinhood.clone())
        } else if id_clean.contains("arcus") || id_clean == "arc" {
            Some(self.live_arcus.clone())
        } else if id_clean.contains("popdex") || id_clean == "pop" {
            Some(self.live_popdex.clone())
        } else if id_clean.contains("polymarket") {
            Some(self.live_polymarket.clone())
        } else if id_clean.contains("kalshi") {
            Some(self.live_kalshi.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Check unconfigured state: 0 active venues without credentials
        let venues_unconfigured = state.get_venues();
        assert_eq!(venues_unconfigured.len(), 0);

        // Adapter lookup by name is always available
        assert!(state.get_adapter("hyperliquid").is_some());
        assert!(state.get_adapter("lighter").is_some());
        assert!(state.get_adapter("vanta").is_some());
        assert!(state.get_adapter("robinhood").is_some());
        assert!(state.get_adapter("arcus").is_some());
        assert!(state.get_adapter("popdex").is_some());
        assert!(state.get_adapter("polymarket").is_some());
        assert!(state.get_adapter("kalshi").is_some());
        assert!(state.get_adapter("nonexistent").is_none());

        // Configure credentials: now venues become active in both paper and live modes
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
            s.vanta_enabled = true;
            s.robinhood_enabled = true;
            s.arcus_enabled = true;
            s.popdex_enabled = true;
            s.hyperliquid_wallet = "0x1111".into();
            s.lighter_api_key = "lgt_key".into();
            s.vanta_orderly_key = "vnt_key".into();
            s.robinhood_wallet = "0x2222".into();
            s.arcus_api_key = "arc_key".into();
            s.popdex_api_key = "pop_key".into();
        }
        let venues_paper = state.get_venues();
        assert_eq!(venues_paper.len(), 6);
        assert_eq!(venues_paper[0].0, "hyperliquid");

        *state.is_live_mode.write() = true;
        let venues_live = state.get_venues();
        assert_eq!(venues_live.len(), 6);
        assert_eq!(venues_live[0].1, "Hyperliquid (L1 DEX)");
    }

    #[tokio::test]
    async fn test_engine_settings_defaults() {
        let state = AppEngineState::new();
        let s = state.settings.read().clone();
        assert_eq!(s.max_slippage_bps, 15.0);
        assert_eq!(s.min_depth_ratio, 3.0);
        assert!(!s.hsm_url.is_empty());
        assert!(!s.robinhood_router.is_empty());
        assert_eq!(s.hyperliquid_url, "https://api.hyperliquid.xyz");
        assert_eq!(s.lighter_url, "https://mainnet.zklighter.elliot.ai");
        assert_eq!(s.vanta_url, "https://api-evm.orderly.org");
        assert_eq!(s.polymarket_url, "https://clob.polymarket.com");
        assert_eq!(s.kalshi_url, "https://trading-api.kalshi.com");
    }
}

