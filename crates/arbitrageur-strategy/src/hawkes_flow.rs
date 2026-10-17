//! Strategy-Level Multi-Asset Hawkes Order Flow Orchestrator.
//!
//! Coordinates cross-asset jump contagion monitoring, cross-venue lead-lag flow
//! tracking, and directional toxic order flow arbitration across monitored symbols and venues.

use arbitrageur_core::types::VenueId;
use arbitrageur_math::cross_hawkes::{
    CrossHawkesConfig, CrossHawkesRegime, MultiAssetCrossHawkesEngine,
};
use serde::{Deserialize, Serialize};

/// Verdict from evaluating market flow conditions for a candidate trade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HawkesFlowVerdict {
    pub regime: CrossHawkesRegime,
    pub is_supercritical: bool,
    pub toxic_drift_bps: f64,
    pub is_toxic_burst: bool,
    pub cojump_withdrawal_recommended: bool,
    pub penalty_hurdle_bps: f64,
    pub is_entry_allowed: bool,
}

/// Orchestrator for multi-asset mutual Hawkes excitation and toxic flow detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HawkesFlowOrchestrator {
    pub engine: MultiAssetCrossHawkesEngine,
}

impl HawkesFlowOrchestrator {
    /// Creates a new HawkesFlowOrchestrator with default crypto market configurations.
    pub fn new(
        asset_symbols: Vec<String>,
        venue_names: Vec<String>,
        adv_usd: Vec<f64>,
        correlations: Vec<Vec<f64>>,
        self_impact: Vec<f64>,
    ) -> Self {
        let config = CrossHawkesConfig {
            num_assets: asset_symbols.len(),
            num_venues: venue_names.len(),
            baseline_intensity: 0.10,
            self_excitation_alpha: 0.20,
            cross_venue_excitation_alpha: 0.10,
            cross_asset_excitation_alpha: 0.05,
            decay_beta: 1.50,
            beta_impact_decay: 2.00,
            toxic_cojump_threshold: 4.80,
        };

        let engine = MultiAssetCrossHawkesEngine::new(
            asset_symbols,
            venue_names,
            adv_usd,
            correlations,
            self_impact,
            config,
        );

        Self { engine }
    }

    /// Records an observed trade event in the multi-asset universe.
    pub fn record_trade(
        &mut self,
        asset_symbol: &str,
        venue: &VenueId,
        is_buy: bool,
        timestamp_sec: f64,
    ) {
        let asset_idx = match self
            .engine
            .asset_symbols
            .iter()
            .position(|s| s == asset_symbol)
        {
            Some(idx) => idx,
            None => return,
        };
        let venue_idx = match self.engine.venue_names.iter().position(|v| v == &venue.0) {
            Some(idx) => idx,
            None => return,
        };

        let stream_idx = MultiAssetCrossHawkesEngine::encode_stream(
            asset_idx,
            venue_idx,
            is_buy,
            self.engine.venue_names.len(),
        );

        self.engine.record_event(stream_idx, timestamp_sec);
    }

    /// Ingests a public trade event tick directly from the MarketDataHub.
    pub fn ingest_trade_event(&mut self, trade: &arbitrageur_core::hub::PublicTradeEvent) {
        let ts_sec = trade.timestamp.timestamp_millis() as f64 / 1000.0;
        self.record_trade(&trade.symbol, &trade.venue, trade.side.is_buy(), ts_sec);
    }

    /// Evaluates current flow conditions for an intended trade on an asset.
    pub fn evaluate_flow_conditions(
        &self,
        asset_symbol: &str,
        venue: &VenueId,
        is_buy_direction: bool,
        current_time_sec: f64,
    ) -> HawkesFlowVerdict {
        let regime = self.engine.classify_regime();
        let is_supercritical = regime == CrossHawkesRegime::Supercritical;

        let asset_idx = self
            .engine
            .asset_symbols
            .iter()
            .position(|s| s == asset_symbol)
            .unwrap_or(0);
        let venue_idx = self
            .engine
            .venue_names
            .iter()
            .position(|v| v == &venue.0)
            .unwrap_or(0);

        let stream_idx = MultiAssetCrossHawkesEngine::encode_stream(
            asset_idx,
            venue_idx,
            is_buy_direction,
            self.engine.venue_names.len(),
        );

        // Check single stream toxic burst
        let intensities = self.engine.current_intensities(current_time_sec);
        let stream_intensity = intensities[stream_idx];
        let baseline = self.engine.baseline_mu[stream_idx].max(1e-6);
        let is_toxic_burst = stream_intensity >= (15.0 * baseline);

        // Compute cross-asset directional toxic drift
        let raw_drift = self
            .engine
            .toxic_reservation_drift_bps(asset_idx, current_time_sec);
        // If we are buying, a negative drift (selling pressure) means adverse selection risk
        let directional_toxic_drift = if is_buy_direction {
            -raw_drift
        } else {
            raw_drift
        };

        // Co-jump withdrawal check across correlated assets
        let mut cojump_withdrawal_recommended = false;
        for other_asset_idx in 0..self.engine.asset_symbols.len() {
            if other_asset_idx != asset_idx {
                let (withdraw, _) = self.engine.check_cojump_quote_withdrawal(
                    asset_idx,
                    other_asset_idx,
                    current_time_sec,
                );
                if withdraw {
                    cojump_withdrawal_recommended = true;
                    break;
                }
            }
        }

        // Penalty hurdle in bps
        let mut penalty_hurdle_bps = match regime {
            CrossHawkesRegime::Supercritical => 30.0,
            CrossHawkesRegime::Critical => 6.0,
            CrossHawkesRegime::Subcritical => 0.0,
        };

        if directional_toxic_drift > 0.0 {
            penalty_hurdle_bps += directional_toxic_drift;
        }

        let is_entry_allowed =
            !is_supercritical && !is_toxic_burst && !cojump_withdrawal_recommended;

        HawkesFlowVerdict {
            regime,
            is_supercritical,
            toxic_drift_bps: directional_toxic_drift,
            is_toxic_burst,
            cojump_withdrawal_recommended,
            penalty_hurdle_bps,
            is_entry_allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_orchestrator() -> HawkesFlowOrchestrator {
        let assets = vec!["BTC".to_string(), "ETH".to_string()];
        let venues = vec!["binance".to_string(), "hyperliquid".to_string()];
        let adv = vec![30_000_000_000.0, 15_000_000_000.0];
        let corr = vec![vec![1.0, 0.85], vec![0.85, 1.0]];
        let self_impact = vec![0.0001, 0.0002];

        HawkesFlowOrchestrator::new(assets, venues, adv, corr, self_impact)
    }

    #[test]
    fn test_hawkes_orchestrator_normal_flow() {
        let orch = make_test_orchestrator();
        let verdict = orch.evaluate_flow_conditions("BTC", &VenueId("binance".into()), true, 1.0);

        assert_eq!(verdict.regime, CrossHawkesRegime::Subcritical);
        assert!(!verdict.is_supercritical);
        assert!(!verdict.is_toxic_burst);
        assert!(verdict.is_entry_allowed);
        assert_eq!(verdict.penalty_hurdle_bps, 0.0);
    }

    #[test]
    fn test_hawkes_orchestrator_toxic_cojump_rejection() {
        let mut orch = make_test_orchestrator();
        // Record burst of 30 market sells on BTC Binance
        for i in 0..30 {
            orch.record_trade(
                "BTC",
                &VenueId("binance".into()),
                false,
                1.0 + i as f64 * 0.005,
            );
        }

        // Now evaluate buying ETH on Hyperliquid
        let verdict =
            orch.evaluate_flow_conditions("ETH", &VenueId("hyperliquid".into()), true, 1.20);
        assert!(
            !verdict.is_entry_allowed,
            "Buying ETH should be disallowed or heavily penalized after massive correlated BTC sell dump"
        );
        assert!(verdict.penalty_hurdle_bps > 10.0);
    }
}
