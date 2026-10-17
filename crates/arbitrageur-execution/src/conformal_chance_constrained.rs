//! Conformal Distributionally Robust Chance-Constrained Optimization (CC-DRO) for Multi-Venue Margin Safeguards.
//!
//! Provides non-parametric chance-constrained optimization and Wasserstein ambiguity ball margin guards
//! across segregated exchanges (Deribit, Hyperliquid, Binance, OKX), triggering proactive de-leveraging
//! or emergency rebalancing before liquidation waterfalls occur.

use serde::{Deserialize, Serialize};

/// Real-time margin and equity state for a venue account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenueMarginState {
    pub venue: String,
    pub equity_usd: f64,
    pub maintenance_margin_usd: f64,
    pub notional_usd: f64,
}

/// Configuration parameters for the Conformal Chance-Constrained Safeguard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalChanceConstrainedConfig {
    /// Target miscoverage probability $\epsilon \in (0, 0.2)$ (e.g. 0.01 for 99% solvency).
    pub target_miscoverage_epsilon: f64,
    /// Conformal Wasserstein ambiguity radius multiplier.
    pub wasserstein_radius_multiplier: f64,
    /// Minimum historical calibration observations.
    pub min_sample_size: usize,
}

impl Default for ConformalChanceConstrainedConfig {
    fn default() -> Self {
        Self {
            target_miscoverage_epsilon: 0.01,
            wasserstein_radius_multiplier: 1.5,
            min_sample_size: 30,
        }
    }
}

/// Operational verdict produced by the Conformal CC-DRO Margin Safeguard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarginSafeguardVerdict {
    /// Accounts are well-capitalized; normal sizing permitted.
    SolventAllocation {
        margin_buffer_ratio: f64,
        worst_case_deficit_usd: f64,
    },
    /// Proactive de-leveraging advised; reduce positions to restore buffer.
    PrecautionaryDeleveraging {
        scale_factor: f64,
        required_reduction_usd: f64,
        reason: String,
    },
    /// Immediate liquidation barrier imminent; halt or emergency rebalance.
    EmergencyRebalanceRequired {
        worst_case_deficit_usd: f64,
        reason: String,
    },
}

/// Conformal Chance-Constrained Margin Safeguard Engine.
pub struct ConformalChanceConstrainedEngine {
    pub config: ConformalChanceConstrainedConfig,
    pub historical_shocks_usd: Vec<f64>,
}

impl ConformalChanceConstrainedEngine {
    pub fn new(config: ConformalChanceConstrainedConfig) -> Self {
        Self {
            config,
            historical_shocks_usd: Vec::with_capacity(128),
        }
    }

    /// Records an observed adverse margin shock or portfolio valuation swing.
    pub fn record_margin_shock(&mut self, shock_usd: f64) {
        if self.historical_shocks_usd.len() >= 200 {
            self.historical_shocks_usd.remove(0);
        }
        self.historical_shocks_usd.push(shock_usd.abs());
    }

    /// Evaluates the conformal Wasserstein radius $\rho_N(\epsilon)$.
    pub fn compute_conformal_wasserstein_radius(&self) -> f64 {
        let n = self.historical_shocks_usd.len();
        if n < self.config.min_sample_size {
            return 500.0; // Conservative fallback
        }

        let mut sorted = self.historical_shocks_usd.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let rank = (((n + 1) as f64 * (1.0 - self.config.target_miscoverage_epsilon)).ceil()
            as usize)
            .min(n);
        let idx = if rank == 0 { 0 } else { rank - 1 };

        sorted[idx] * self.config.wasserstein_radius_multiplier
    }

    /// Evaluates multi-venue margin solvency and determines proactive shielding actions.
    pub fn evaluate_margin_safeguard(&self, venues: &[VenueMarginState]) -> MarginSafeguardVerdict {
        if venues.is_empty() {
            return MarginSafeguardVerdict::SolventAllocation {
                margin_buffer_ratio: 1.0,
                worst_case_deficit_usd: 0.0,
            };
        }

        let radius = self.compute_conformal_wasserstein_radius();
        let mut total_equity = 0.0;
        let mut total_margin = 0.0;
        let mut max_single_venue_deficit = 0.0;

        for v in venues {
            total_equity += v.equity_usd;
            total_margin += v.maintenance_margin_usd;

            // Worst-case deficit for this venue under conformal shock radius
            let venue_deficit = (v.maintenance_margin_usd + radius) - v.equity_usd;
            if venue_deficit > max_single_venue_deficit {
                max_single_venue_deficit = venue_deficit;
            }
        }

        let buffer_ratio = if total_margin > 1e-6 {
            (total_equity - total_margin) / total_margin
        } else {
            10.0
        };

        if max_single_venue_deficit <= 0.0 && buffer_ratio >= 0.35 {
            MarginSafeguardVerdict::SolventAllocation {
                margin_buffer_ratio: buffer_ratio,
                worst_case_deficit_usd: 0.0,
            }
        } else if max_single_venue_deficit <= total_equity * 0.25 {
            let scale = (total_equity / (total_equity + max_single_venue_deficit))
                .powi(2)
                .clamp(0.15, 0.90);
            MarginSafeguardVerdict::PrecautionaryDeleveraging {
                scale_factor: scale,
                required_reduction_usd: max_single_venue_deficit,
                reason: format!(
                    "Conformal CC-DRO: single venue worst-case deficit ${:.2} under radius ${:.2}; scale throttled to {:.1}%",
                    max_single_venue_deficit,
                    radius,
                    scale * 100.0
                ),
            }
        } else {
            MarginSafeguardVerdict::EmergencyRebalanceRequired {
                worst_case_deficit_usd: max_single_venue_deficit,
                reason: format!(
                    "CRITICAL: worst-case deficit ${:.2} exceeds 25% of total equity ${:.2} under conformal shock radius ${:.2}",
                    max_single_venue_deficit,
                    total_equity,
                    radius
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solvent_allocation_when_margin_healthy() {
        let config = ConformalChanceConstrainedConfig {
            target_miscoverage_epsilon: 0.01,
            wasserstein_radius_multiplier: 1.2,
            min_sample_size: 10,
        };
        let mut engine = ConformalChanceConstrainedEngine::new(config);
        for _ in 0..15 {
            engine.record_margin_shock(50.0);
        }

        let venues = vec![
            VenueMarginState {
                venue: "deribit".into(),
                equity_usd: 10_000.0,
                maintenance_margin_usd: 2_000.0,
                notional_usd: 20_000.0,
            },
            VenueMarginState {
                venue: "hyperliquid".into(),
                equity_usd: 15_000.0,
                maintenance_margin_usd: 3_000.0,
                notional_usd: 30_000.0,
            },
        ];

        let verdict = engine.evaluate_margin_safeguard(&venues);
        match verdict {
            MarginSafeguardVerdict::SolventAllocation {
                margin_buffer_ratio,
                ..
            } => {
                assert!(margin_buffer_ratio > 1.0);
            }
            _ => panic!("Expected SolventAllocation for healthy margin states"),
        }
    }

    #[test]
    fn test_precautionary_deleveraging_and_emergency_rebalance() {
        let config = ConformalChanceConstrainedConfig {
            target_miscoverage_epsilon: 0.01,
            wasserstein_radius_multiplier: 1.5,
            min_sample_size: 10,
        };
        let mut engine = ConformalChanceConstrainedEngine::new(config);
        for _ in 0..15 {
            engine.record_margin_shock(200.0); // radius ~ 300.0
        }

        // Moderate stress: equity = 1,000, margin = 850, radius = 300 -> deficit = 1150 - 1000 = 150 (<= 250 = 25% of 1000)
        let venues_moderate = vec![VenueMarginState {
            venue: "binance".into(),
            equity_usd: 1000.0,
            maintenance_margin_usd: 850.0,
            notional_usd: 8_000.0,
        }];
        match engine.evaluate_margin_safeguard(&venues_moderate) {
            MarginSafeguardVerdict::PrecautionaryDeleveraging { scale_factor, .. } => {
                assert!((0.15..1.0).contains(&scale_factor));
            }
            _ => panic!("Expected PrecautionaryDeleveraging for moderate margin deficit"),
        }

        // Severe stress: equity = 1,000, margin = 950, radius = 400 -> deficit = 1350 - 1000 = 350 (> 25% of 1000)
        for _ in 0..10 {
            engine.record_margin_shock(400.0);
        }
        let venues_severe = vec![VenueMarginState {
            venue: "okx".into(),
            equity_usd: 1000.0,
            maintenance_margin_usd: 950.0,
            notional_usd: 9_000.0,
        }];
        match engine.evaluate_margin_safeguard(&venues_severe) {
            MarginSafeguardVerdict::EmergencyRebalanceRequired {
                worst_case_deficit_usd,
                ..
            } => {
                assert!(worst_case_deficit_usd > 250.0);
            }
            _ => panic!("Expected EmergencyRebalanceRequired for severe margin deficit"),
        }
    }
}
