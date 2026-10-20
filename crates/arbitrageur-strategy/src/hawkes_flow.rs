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
        let Some(asset_idx) = self.resolve_asset_idx(asset_symbol) else {
            return;
        };
        let Some(venue_idx) = self.resolve_venue_idx(&venue.0) else {
            return;
        };

        let stream_idx = MultiAssetCrossHawkesEngine::encode_stream(
            asset_idx,
            venue_idx,
            is_buy,
            self.engine.venue_names.len(),
        );

        self.engine.record_event(stream_idx, timestamp_sec);
    }

    /// Resolve a configured asset without ever aliasing an unknown symbol to
    /// matrix index zero. The engine has fixed capacity from construction;
    /// unseen identifiers fail closed.
    fn resolve_asset_idx(&self, asset_symbol: &str) -> Option<usize> {
        self.engine
            .asset_symbols
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(asset_symbol))
    }

    /// Resolve a configured venue, never falling back to another venue.
    fn resolve_venue_idx(&self, venue: &str) -> Option<usize> {
        self.engine
            .venue_names
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(venue))
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

        let Some(asset_idx) = self.resolve_asset_idx(asset_symbol) else {
            return HawkesFlowVerdict {
                regime,
                is_supercritical: false,
                toxic_drift_bps: 0.0,
                is_toxic_burst: false,
                cojump_withdrawal_recommended: false,
                penalty_hurdle_bps: 0.0,
                is_entry_allowed: false,
            };
        };
        let Some(venue_idx) = self.resolve_venue_idx(&venue.0) else {
            return HawkesFlowVerdict {
                regime,
                is_supercritical: false,
                toxic_drift_bps: 0.0,
                is_toxic_burst: false,
                cojump_withdrawal_recommended: false,
                penalty_hurdle_bps: 0.0,
                is_entry_allowed: false,
            };
        };

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

    /// Evaluates both legs of a buy-A/sell-B route. Either leg may veto the
    /// route; the hurdle expands by the larger directional flow penalty so a
    /// single adverse venue cannot be diluted by a quiet sibling.
    pub fn evaluate_pair_gate(
        &self,
        asset: &str,
        venue_a: &str,
        venue_b: &str,
        current_time_sec: f64,
        base_hurdle_bps: f64,
    ) -> (bool, Option<String>, f64) {
        if self.resolve_asset_idx(asset).is_none() {
            return (
                false,
                Some(format!(
                    "asset '{asset}' is not configured in the Hawkes matrix"
                )),
                base_hurdle_bps.max(0.0),
            );
        }
        if self.resolve_venue_idx(venue_a).is_none() {
            return (
                false,
                Some(format!(
                    "venue '{venue_a}' is not configured in the Hawkes matrix"
                )),
                base_hurdle_bps.max(0.0),
            );
        }
        if self.resolve_venue_idx(venue_b).is_none() {
            return (
                false,
                Some(format!(
                    "venue '{venue_b}' is not configured in the Hawkes matrix"
                )),
                base_hurdle_bps.max(0.0),
            );
        }

        let buy_a = self.evaluate_flow_conditions(
            asset,
            &VenueId(venue_a.to_string()),
            true,
            current_time_sec,
        );
        let sell_b = self.evaluate_flow_conditions(
            asset,
            &VenueId(venue_b.to_string()),
            false,
            current_time_sec,
        );
        let flow_ok = buy_a.is_entry_allowed && sell_b.is_entry_allowed;
        let reason = if !flow_ok {
            let mut blockers = Vec::new();
            if !buy_a.is_entry_allowed {
                blockers.push(format!("{venue_a} buy leg"));
            }
            if !sell_b.is_entry_allowed {
                blockers.push(format!("{venue_b} sell leg"));
            }
            Some(format!(
                "Hawkes pair gate blocked {}",
                blockers.join(" and ")
            ))
        } else {
            None
        };
        let penalty = buy_a
            .penalty_hurdle_bps
            .max(sell_b.penalty_hurdle_bps)
            .max(0.0);
        (flow_ok, reason, base_hurdle_bps.max(0.0) + penalty)
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
    fn unknown_venue_or_asset_never_aliases_index_zero() {
        let orch = make_test_orchestrator();
        let unknown_venue =
            orch.evaluate_flow_conditions("BTC", &VenueId("unknown".into()), true, 1.0);
        assert!(!unknown_venue.is_entry_allowed);

        let unknown_asset =
            orch.evaluate_flow_conditions("SOL", &VenueId("binance".into()), true, 1.0);
        assert!(!unknown_asset.is_entry_allowed);

        let (allowed, reason, hurdle) =
            orch.evaluate_pair_gate("BTC", "binance", "unknown", 1.0, 5.0);
        assert!(!allowed);
        assert_eq!(hurdle, 5.0);
        assert!(reason.expect("reason").contains("unknown"));
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
