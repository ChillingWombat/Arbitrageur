use crate::funding_rate_arb::FundingRateArbStrategy;
use crate::spatial_price_arb::SpatialPriceArbStrategy;
use arbitrageur_core::types::dec;
use arbitrageur_execution::coordinator::AtomicArbitrageCoordinator;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct ArbitrageEngine {
    is_running: Arc<AtomicBool>,
    coordinator: Arc<AtomicArbitrageCoordinator>,
    poll_interval: Duration,
}

impl ArbitrageEngine {
    pub fn new(coordinator: Arc<AtomicArbitrageCoordinator>, poll_interval_ms: u64) -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            coordinator,
            poll_interval: Duration::from_millis(poll_interval_ms),
        }
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Runs a spatial price spread arbitrage strategy loop.
    pub async fn run_spatial_loop(&self, mut strategy: SpatialPriceArbStrategy) {
        self.is_running.store(true, Ordering::SeqCst);
        info!("Starting Spatial Arbitrage loop for {}", strategy.symbol);

        while self.is_running.load(Ordering::Relaxed) {
            match strategy.evaluate_opportunity().await {
                Ok(Some(signal)) => {
                    info!(
                        "⚡ SPATIAL ARBITRAGE SIGNAL: {} {:?} Spread={:.4} ({:.2} BPS), Est Profit=${:.2}",
                        signal.symbol,
                        signal.direction,
                        signal.spread,
                        signal.spread_bps,
                        signal.expected_profit_usd
                    );

                    let result = strategy.execute_signal(&signal, &self.coordinator).await;
                    info!("Execution result: {:?}", result);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Error evaluating spatial opportunity: {}", e);
                }
            }

            sleep(self.poll_interval).await;
        }

        info!("Spatial Arbitrage loop stopped for {}", strategy.symbol);
    }

    /// Runs a funding rate carry arbitrage monitor loop.
    pub async fn run_funding_loop(&self, strategy: FundingRateArbStrategy) {
        self.is_running.store(true, Ordering::SeqCst);
        info!("Starting Funding Carry loop for {}", strategy.symbol);

        while self.is_running.load(Ordering::Relaxed) {
            match strategy.scan_opportunity().await {
                Ok(Some(opp)) => {
                    info!(
                        "💸 FUNDING CARRY OPPORTUNITY: {} Long: {}, Short: {}, 8h Rate: {:.4}%, APR: {:.2}%, Breakeven: {:.1}h",
                        opp.symbol,
                        opp.long_venue,
                        opp.short_venue,
                        opp.net_rate_8h * dec!(100),
                        opp.net_apr * dec!(100),
                        opp.break_even_hours
                    );

                    match strategy.execute_open_carry(&opp, &self.coordinator).await {
                        Ok(res) => info!("Carry execution result: {:?}", res),
                        Err(e) => error!("Failed to execute carry trade: {}", e),
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Error scanning funding carry: {}", e);
                }
            }

            sleep(self.poll_interval * 5).await; // Funding rate checks can be spaced out
        }

        info!("Funding Carry loop stopped for {}", strategy.symbol);
    }
}
