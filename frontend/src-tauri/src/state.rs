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
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct EngineSettingsDto {
    pub hsm_url: String,
    pub hmac_token: String,
    pub max_slippage_bps: f64,
    pub min_depth_ratio: f64,
    pub robinhood_router: String,
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

        let settings = RwLock::new(EngineSettingsDto {
            hsm_url: "http://127.0.0.1:8787/sign".into(),
            hmac_token: "••••••••••••••••••••••••".into(),
            max_slippage_bps: 15.0,
            min_depth_ratio: 3.0,
            robinhood_router: "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba".into(),
        });

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
            execution_logs: RwLock::new(Vec::new()),
            sim_step_counter: RwLock::new(0),
            settings,
        }
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
    }
}

