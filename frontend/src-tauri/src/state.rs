use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use arbitrager_core::types::{dec, Decimal};
use arbitrager_exchanges::hyperliquid::HyperliquidAdapter;
use arbitrager_exchanges::lighter::LighterAdapter;
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
            user_target_leverage: 10,
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
    pub live_hl: Arc<HyperliquidAdapter>,
    pub live_lighter: Arc<LighterAdapter>,
    pub live_vanta: Arc<VantaAdapter>,
    pub live_robinhood: Arc<RobinhoodAdapter>,
    pub live_polymarket: Arc<PolymarketAdapter>,
    pub live_kalshi: Arc<KalshiAdapter>,
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
}

impl AppEngineState {
    pub fn new() -> Self {
        let ex_hl = Arc::new(SimulatedExchange::new("hyperliquid", "Hyperliquid (L1)", 2, dec!(2.0), dec!(100000.0)));
        let ex_lighter = Arc::new(SimulatedExchange::new("lighter", "Lighter (zkDEX)", 5, dec!(1.5), dec!(100000.0)));
        let ex_vanta = Arc::new(SimulatedExchange::new("vanta", "Vanta.exchange", 4, dec!(3.0), dec!(100000.0)));
        let ex_robinhood = Arc::new(SimulatedExchange::new("robinhood", "Robinhood EVM", 8, dec!(2.5), dec!(100000.0)));

        // Seed initial markets
        ex_hl.seed_market("BTC-USDC", dec!(64200.0), dec!(64208.0), dec!(8.5), dec!(0.0003));
        ex_lighter.seed_market("BTC-USDC", dec!(64225.0), dec!(64235.0), dec!(6.2), dec!(0.0001));
        ex_vanta.seed_market("BTC-USDC", dec!(64210.0), dec!(64218.0), dec!(9.0), dec!(0.00015));
        ex_robinhood.seed_market("BTC-USDC", dec!(64205.0), dec!(64215.0), dec!(5.0), dec!(0.0002));

        ex_hl.seed_market("ETH-USDC", dec!(3450.0), dec!(3451.2), dec!(45.0), dec!(0.00025));
        ex_lighter.seed_market("ETH-USDC", dec!(3453.5), dec!(3454.8), dec!(38.0), dec!(0.00008));
        ex_vanta.seed_market("ETH-USDC", dec!(3451.0), dec!(3452.1), dec!(50.0), dec!(0.00012));
        ex_robinhood.seed_market("ETH-USDC", dec!(3450.5), dec!(3451.8), dec!(30.0), dec!(0.00018));

        ex_hl.seed_market("SOL-USDC", dec!(152.2), dec!(152.4), dec!(350.0), dec!(0.0004));
        ex_lighter.seed_market("SOL-USDC", dec!(152.8), dec!(153.0), dec!(280.0), dec!(0.0001));
        ex_vanta.seed_market("SOL-USDC", dec!(152.4), dec!(152.6), dec!(400.0), dec!(0.0002));
        ex_robinhood.seed_market("SOL-USDC", dec!(152.3), dec!(152.5), dec!(200.0), dec!(0.00025));

        // Seed comprehensive union of 52 perp DEX tickers (each on >= 2 venues)
        let other_tickers = [
            "AVAX-USDC", "BNB-USDC", "ARB-USDC", "OP-USDC", "SUI-USDC", "NEAR-USDC",
            "APT-USDC", "LINK-USDC", "DOGE-USDC", "PEPE-USDC", "WIF-USDC", "POL-USDC",
            "TIA-USDC", "SEI-USDC", "INJ-USDC", "RENDER-USDC", "FTM-USDC", "ATOM-USDC",
            "ADA-USDC", "XRP-USDC", "DOT-USDC", "LTC-USDC", "BCH-USDC", "UNI-USDC",
            "AAVE-USDC", "CRV-USDC", "MKR-USDC", "LDO-USDC", "JUP-USDC", "PYTH-USDC",
            "ONDO-USDC", "KAS-USDC", "TON-USDC", "NOT-USDC", "PENDLE-USDC", "FET-USDC",
            "TAO-USDC", "WLD-USDC", "BLUR-USDC", "STX-USDC", "RUNE-USDC", "DYDX-USDC",
            "ENA-USDC", "STRK-USDC", "JTO-USDC", "BONK-USDC", "FLOKI-USDC", "SHIB-USDC",
            "AR-USDC",
        ];

        for &sym in &other_tickers {
            let base = crate::commands::symbol_base_price(sym);
            let depth = (dec!(500_000.0) / base).max(dec!(50.0));
            let half_spread = base * dec!(0.00015);
            let lgt_disloc = base * dec!(0.00028);
            let vnt_disloc = base * dec!(0.00010);
            let rh_disloc = base * dec!(0.00005);

            ex_hl.seed_market(sym, base - half_spread, base + half_spread, depth, dec!(0.00025));
            ex_lighter.seed_market(sym, base + lgt_disloc - half_spread, base + lgt_disloc + half_spread, depth * dec!(0.8), dec!(0.00008));
            ex_vanta.seed_market(sym, base + vnt_disloc - half_spread, base + vnt_disloc + half_spread, depth * dec!(1.2), dec!(0.00015));
            ex_robinhood.seed_market(sym, base + rh_disloc - half_spread, base + rh_disloc + half_spread, depth * dec!(0.6), dec!(0.00018));
        }

        let live_hl = Arc::new(HyperliquidAdapter::new(None, None, None));
        let live_lighter = Arc::new(LighterAdapter::new(None, None, None));
        let live_vanta = Arc::new(VantaAdapter::new(None, None, None, None));
        let live_robinhood = Arc::new(RobinhoodAdapter::new(None, None, None, None));
        let live_polymarket = Arc::new(PolymarketAdapter::new(None, None));
        let live_kalshi = Arc::new(KalshiAdapter::new(None, None));

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

        let initial_logs = vec![
            ExecutionLogEntry {
                id: "exec-init-1".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 48.5,
                fees: 2.1,
                net_profit: 46.4,
                latency_us: 114,
                note: "Executed atomically at spread 18.2 BPS (Dislocation +6.8 BPS over baseline)".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(18.2),
                baseline_mean_spread_bps: Some(11.4),
                position_step: Some(1),
                max_units: Some(3),
                nominal_hurdle_bps: Some(14.8),
            },
            ExecutionLogEntry {
                id: "exec-init-2".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Funding".into(),
                symbol: "ETH-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Vanta.exchange".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 32.0,
                fees: 1.8,
                net_profit: 30.2,
                latency_us: 98,
                note: "Carry opened: Short HL (0.03%/1h) + Long Vanta (0.015%/8h)".into(),
                direction: Some("Carry Open".into()),
                trigger_spread_bps: Some(15.0),
                baseline_mean_spread_bps: Some(9.5),
                position_step: Some(1),
                max_units: Some(3),
                nominal_hurdle_bps: Some(12.0),
            },
            ExecutionLogEntry {
                id: "exec-init-3".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Robinhood EVM".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 41.0,
                fees: 1.9,
                net_profit: 39.1,
                latency_us: 120,
                note: "Step 2 scale-in on widening divergence".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(22.4),
                baseline_mean_spread_bps: Some(13.1),
                position_step: Some(2),
                max_units: Some(3),
                nominal_hurdle_bps: Some(15.2),
            },
            ExecutionLogEntry {
                id: "exec-init-3b".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 26.0,
                fees: 1.5,
                net_profit: 24.5,
                latency_us: 108,
                note: "Partial unwind as spread converged toward mean".into(),
                direction: Some("Convergence Unwind".into()),
                trigger_spread_bps: Some(4.2),
                baseline_mean_spread_bps: Some(12.0),
                position_step: Some(2),
                max_units: Some(3),
                nominal_hurdle_bps: Some(6.0),
            },
            ExecutionLogEntry {
                id: "exec-init-4".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "SOL-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 20.0,
                fees: 1.8,
                net_profit: 18.2,
                latency_us: 105,
                note: "Executed atomically at spread 16.4 BPS".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(16.4),
                baseline_mean_spread_bps: Some(10.2),
                position_step: Some(1),
                max_units: Some(4),
                nominal_hurdle_bps: Some(13.5),
            },
            ExecutionLogEntry {
                id: "exec-init-5".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "SUI-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 12.0,
                fees: 1.7,
                net_profit: 10.3,
                latency_us: 92,
                note: "Sui perp dislocation captured".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(24.5),
                baseline_mean_spread_bps: Some(14.0),
                position_step: Some(1),
                max_units: Some(3),
                nominal_hurdle_bps: Some(17.2),
            },
            ExecutionLogEntry {
                id: "exec-init-6".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "AVAX-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Vanta.exchange".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "FILLED".into(),
                success: true,
                gross_profit: 7.5,
                fees: 1.1,
                net_profit: 6.4,
                latency_us: 110,
                note: "AVAX perp basis captured on convergence".into(),
                direction: Some("Convergence Unwind".into()),
                trigger_spread_bps: Some(3.2),
                baseline_mean_spread_bps: Some(8.5),
                position_step: Some(2),
                max_units: Some(3),
                nominal_hurdle_bps: Some(5.0),
            },
            ExecutionLogEntry {
                id: "exec-init-7".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "ARB-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "REJECTED".into(),
                success: false,
                gross_profit: 0.0,
                fees: 0.8,
                net_profit: -0.8,
                latency_us: 130,
                note: "DeltaGuard safety unwind on Leg B rejection".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(19.0),
                baseline_mean_spread_bps: Some(12.0),
                position_step: Some(1),
                max_units: Some(3),
                nominal_hurdle_bps: Some(15.0),
            },
            ExecutionLogEntry {
                id: "exec-init-8".into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                symbol: "DOGE-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                leg_a_status: "FILLED".into(),
                leg_b_status: "REJECTED".into(),
                success: false,
                gross_profit: 0.0,
                fees: 1.5,
                net_profit: -1.5,
                latency_us: 145,
                note: "Emergency unwind on venue latency spike".into(),
                direction: Some("Divergence Entry".into()),
                trigger_spread_bps: Some(21.5),
                baseline_mean_spread_bps: Some(12.8),
                position_step: Some(1),
                max_units: Some(3),
                nominal_hurdle_bps: Some(16.0),
            },
        ];

        let vault = RwLock::new(crate::vault::SecureVault::new_with_path(crate::vault::SecureVault::default_path()));
        let mut virtual_ledger_raw = arbitrager_execution::virtual_ledger::VirtualStrategyLedger::new();
        for log in &initial_logs {
            let strategy_id = match log.trade_type.as_str() {
                "Spatial" => "spatial_spread",
                "Funding" => "funding_rate",
                "Prediction" => "prediction_parity",
                _ => "spatial_spread",
            };
            virtual_ledger_raw.open_cycle(
                log.id.clone(),
                strategy_id.to_string(),
                log.symbol.clone(),
                log.venue_a.clone(),
                log.venue_b.clone(),
                log.trigger_spread_bps.unwrap_or(15.0),
                0.1,
                64000.0,
                0.1,
                64100.0,
                log.fees,
            );
            let status = if log.success {
                arbitrager_execution::virtual_ledger::CycleStatus::Closed
            } else {
                arbitrager_execution::virtual_ledger::CycleStatus::StoppedOut
            };
            let pnl_bps = log.trigger_spread_bps.unwrap_or(12.0) * if log.success { 0.8 } else { -0.5 };
            virtual_ledger_raw.close_cycle_with_pnl(&log.id, log.net_profit, pnl_bps, status);
        }
        let virtual_ledger = RwLock::new(virtual_ledger_raw);

        Self {
            ex_hl,
            ex_lighter,
            ex_vanta,
            ex_robinhood,
            live_hl,
            live_lighter,
            live_vanta,
            live_robinhood,
            live_polymarket,
            live_kalshi,
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
        }
    }

    /// Ensure market is seeded dynamically if queried
    pub fn ensure_market(&self, symbol: &str) {
        if self.ex_hl.has_market(symbol) {
            return;
        }

        let base = crate::commands::symbol_base_price(symbol);
        let depth = (dec!(500_000.0) / base).max(dec!(50.0));
        let half_spread = base * dec!(0.00015);
        let lgt_disloc = base * dec!(0.00028);
        let vnt_disloc = base * dec!(0.00010);
        let rh_disloc = base * dec!(0.00005);

        self.ex_hl.seed_market(symbol, base - half_spread, base + half_spread, depth, dec!(0.00025));
        self.ex_lighter.seed_market(symbol, base + lgt_disloc - half_spread, base + lgt_disloc + half_spread, depth * dec!(0.8), dec!(0.00008));
        self.ex_vanta.seed_market(symbol, base + vnt_disloc - half_spread, base + vnt_disloc + half_spread, depth * dec!(1.2), dec!(0.00015));
        self.ex_robinhood.seed_market(symbol, base + rh_disloc - half_spread, base + rh_disloc + half_spread, depth * dec!(0.6), dec!(0.00018));
    }

    /// Returns active venue list dynamically based on simulated vs live mode
    pub fn get_venues(&self) -> Vec<(&'static str, &'static str, Arc<dyn arbitrager_core::exchange::ExchangeAdapter>)> {
        let is_live = *self.is_live_mode.read();
        if is_live {
            vec![
                ("hyperliquid", "Hyperliquid (L1 DEX)", self.live_hl.clone()),
                ("lighter", "Lighter (zkDEX)", self.live_lighter.clone()),
                ("vanta", "Vanta.exchange", self.live_vanta.clone()),
                ("robinhood", "Robinhood EVM", self.live_robinhood.clone()),
            ]
        } else {
            vec![
                ("hyperliquid", "Hyperliquid (L1)", self.ex_hl.clone()),
                ("lighter", "Lighter (zkDEX)", self.ex_lighter.clone()),
                ("vanta", "Vanta.exchange", self.ex_vanta.clone()),
                ("robinhood", "Robinhood EVM", self.ex_robinhood.clone()),
            ]
        }
    }

    /// Resolves an exchange adapter by venue identifier or name
    pub fn get_adapter(&self, venue_id: &str) -> Option<Arc<dyn arbitrager_core::exchange::ExchangeAdapter>> {
        let is_live = *self.is_live_mode.read();
        let id_clean = venue_id.to_lowercase();
        if id_clean.contains("hyperliquid") || id_clean == "hl" {
            if is_live { Some(self.live_hl.clone()) } else { Some(self.ex_hl.clone()) }
        } else if id_clean.contains("lighter") {
            if is_live { Some(self.live_lighter.clone()) } else { Some(self.ex_lighter.clone()) }
        } else if id_clean.contains("vanta") {
            if is_live { Some(self.live_vanta.clone()) } else { Some(self.ex_vanta.clone()) }
        } else if id_clean.contains("robinhood") {
            if is_live { Some(self.live_robinhood.clone()) } else { Some(self.ex_robinhood.clone()) }
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
    use arbitrager_core::exchange::ExchangeAdapter;

    #[tokio::test]
    async fn test_app_state_initialization() {
        let state = AppEngineState::new();
        assert!(!*state.is_live_mode.read());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
        assert!(state.circuit_breaker.is_trading_allowed().is_ok());

        // Verify seeded orderbook on Hyperliquid
        let book_hl = state.ex_hl.fetch_orderbook("BTC-USDC").await.expect("orderbook should exist");
        assert_eq!(book_hl.best_bid_price(), Some(dec!(64200.0)));
        assert_eq!(book_hl.best_ask_price(), Some(dec!(64208.0)));
        assert_eq!(book_hl.spread(), Some(dec!(8.0)));

        // Verify seeded orderbook on Lighter
        let book_lighter = state.ex_lighter.fetch_orderbook("BTC-USDC").await.expect("orderbook should exist");
        assert_eq!(book_lighter.best_bid_price(), Some(dec!(64225.0)));
        assert_eq!(book_lighter.best_ask_price(), Some(dec!(64235.0)));

        // Verify spatial dislocation: Lighter Bid (64225) > HL Ask (64208)
        assert!(book_lighter.best_bid_price().unwrap() > book_hl.best_ask_price().unwrap());
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

        // Check simulated venues by default
        let venues_sim = state.get_venues();
        assert_eq!(venues_sim.len(), 4);
        assert_eq!(venues_sim[0].0, "hyperliquid");
        assert_eq!(venues_sim[0].1, "Hyperliquid (L1)");

        // Check adapter lookup
        assert!(state.get_adapter("hyperliquid").is_some());
        assert!(state.get_adapter("lighter").is_some());
        assert!(state.get_adapter("vanta").is_some());
        assert!(state.get_adapter("robinhood").is_some());
        assert!(state.get_adapter("polymarket").is_some());
        assert!(state.get_adapter("kalshi").is_some());
        assert!(state.get_adapter("nonexistent").is_none());

        // Toggle to live mode
        *state.is_live_mode.write() = true;
        let venues_live = state.get_venues();
        assert_eq!(venues_live.len(), 4);
        assert_eq!(venues_live[0].1, "Hyperliquid (L1 DEX)");
        assert!(state.get_adapter("hyperliquid").is_some());
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

