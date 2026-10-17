//! Cross-Exchange Cascading Collateral Liquidation Barrier Protection & Dynamic Extreme Stress Testing.
//!
//! Models segregated cross-margin accounts across disparate venues (e.g. Binance, OKX, Hyperliquid, Lighter, Deribit).
//! Solves:
//! 1. Continuous liquidation barrier:
//!    $$B_{\text{liq}, k}(t) = E_k(t) - \text{MMR}_k(t)$$
//! 2. Dynamic correlated stress matrix on signed dollar delta:
//!    $$E_k(\delta) = E_k(t) + \delta \cdot \text{net\_dollar\_delta}_k$$
//!    $$\text{MMR}_k(\delta) = \text{notional}_k \cdot (1 + |\delta|) \cdot \text{MMR\_rate}_k$$
//!    $$B_{\text{liq}, k}(\delta) = E_k(\delta) - \text{MMR}_k(\delta)$$
//! 3. Proactive water-filling rebalance transfer vector $\Delta C_{i \to j}^*$ from surplus venues to deficit venues
//!    accounting for rail transfer latencies and fee friction before margin calls occur.

use serde::{Deserialize, Serialize};

/// Segregated margin state for one venue account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VenueAccountMarginState {
    pub venue_id: String,
    pub current_equity_usd: f64,
    pub notional_usd: f64,
    pub maintenance_margin_rate: f64,
    pub equity_volatility_usd: f64,
    /// Signed dollar delta: positive for net long, negative for net short.
    pub net_dollar_delta: f64,
}

impl VenueAccountMarginState {
    pub fn new(
        venue_id: impl Into<String>,
        current_equity_usd: f64,
        notional_usd: f64,
        maintenance_margin_rate: f64,
        equity_volatility_usd: f64,
        net_dollar_delta: f64,
    ) -> Self {
        Self {
            venue_id: venue_id.into(),
            current_equity_usd,
            notional_usd: notional_usd.max(0.0),
            maintenance_margin_rate: maintenance_margin_rate.clamp(0.005, 0.50),
            equity_volatility_usd: equity_volatility_usd.max(0.0),
            net_dollar_delta,
        }
    }

    /// Maintenance margin requirement in USD
    pub fn maintenance_margin_usd(&self) -> f64 {
        self.notional_usd * self.maintenance_margin_rate
    }

    /// Continuous liquidation barrier: B_liq = Equity - MMR
    pub fn liquidation_barrier_usd(&self) -> f64 {
        self.current_equity_usd - self.maintenance_margin_usd()
    }

    /// Stressed liquidation barrier under directional market shock delta (e.g. -0.25 for -25% crash)
    pub fn stressed_liquidation_barrier_usd(&self, shock_pct: f64) -> f64 {
        let stressed_equity = self.current_equity_usd + shock_pct * self.net_dollar_delta;
        let stressed_notional = self.notional_usd * (1.0 + shock_pct.abs());
        let stressed_mmr = stressed_notional * self.maintenance_margin_rate;
        stressed_equity - stressed_mmr
    }
}

/// A planned proactive collateral rebalancing transfer between two segregated venues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollateralRebalanceTransfer {
    pub from_venue: String,
    pub to_venue: String,
    pub amount_usd: f64,
    pub estimated_fee_usd: f64,
    pub transfer_latency_sec: f64,
}

/// Proactive Collateral Barrier Rebalancing Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossVenueCollateralBarrierRebalancer {
    /// Target safety margin buffer ratio (e.g. 0.25 = 25% of notional buffer above MMR)
    pub target_safety_margin_ratio: f64,
    /// Critical time-to-liquidation cutoff in seconds (e.g. 60s)
    pub critical_time_to_liq_sec: f64,
    /// Standard stress shocks to evaluate (e.g. -35%, -25%, -15%, +15%, +25%, +35%)
    pub default_stress_shocks: Vec<f64>,
}

impl Default for CrossVenueCollateralBarrierRebalancer {
    fn default() -> Self {
        Self {
            target_safety_margin_ratio: 0.25,
            critical_time_to_liq_sec: 60.0,
            default_stress_shocks: vec![-0.35, -0.25, -0.15, 0.15, 0.25, 0.35],
        }
    }
}

impl CrossVenueCollateralBarrierRebalancer {
    pub fn new(target_buffer: f64, critical_time_sec: f64) -> Self {
        Self {
            target_safety_margin_ratio: target_buffer.clamp(0.05, 0.50),
            critical_time_to_liq_sec: critical_time_sec.max(10.0),
            default_stress_shocks: vec![-0.35, -0.25, -0.15, 0.15, 0.25, 0.35],
        }
    }

    /// Evaluates stress matrix across accounts and returns (shock_pct, Vec<(venue_id, stressed_barrier)>)
    pub fn evaluate_stress_matrix(
        &self,
        accounts: &[VenueAccountMarginState],
        custom_shocks: Option<&[f64]>,
    ) -> Vec<(f64, Vec<(String, f64)>)> {
        let shocks = custom_shocks.unwrap_or(&self.default_stress_shocks);
        let mut results = Vec::with_capacity(shocks.len());

        for &shock in shocks {
            let mut venue_barriers = Vec::with_capacity(accounts.len());
            for acc in accounts {
                let barrier = acc.stressed_liquidation_barrier_usd(shock);
                venue_barriers.push((acc.venue_id.clone(), barrier));
            }
            results.push((shock, venue_barriers));
        }

        results
    }

    /// Returns true if any account breaches liquidation barrier (B_liq <= 0) under any stress scenario.
    pub fn has_stress_liquidation_breach(
        &self,
        accounts: &[VenueAccountMarginState],
        custom_shocks: Option<&[f64]>,
    ) -> bool {
        let matrix = self.evaluate_stress_matrix(accounts, custom_shocks);
        matrix
            .iter()
            .any(|(_, venues)| venues.iter().any(|(_, b)| *b <= 0.0))
    }

    /// Evaluates multi-venue liquidation barrier B_liq,k and solves proactive transfer vectors
    pub fn plan_proactive_rebalancing(
        &self,
        accounts: &[VenueAccountMarginState],
        rail_latencies: &[(&str, &str, f64, f64)], // (from, to, latency_sec, fee_pct)
    ) -> Vec<CollateralRebalanceTransfer> {
        let mut surpluses = Vec::new();
        let mut deficits = Vec::new();

        for acc in accounts {
            let mmr_usd = acc.maintenance_margin_usd();
            let b_liq = acc.current_equity_usd - mmr_usd;
            let target_b = acc.notional_usd * self.target_safety_margin_ratio;

            if b_liq < target_b {
                deficits.push((
                    acc.venue_id.clone(),
                    target_b - b_liq,
                    acc.equity_volatility_usd,
                    b_liq,
                ));
            } else if b_liq > target_b {
                surpluses.push((acc.venue_id.clone(), b_liq - target_b));
            }
        }

        let mut transfers = Vec::new();

        // Water-filling matching
        for (def_venue, mut needed_amount, vol_usd, b_liq) in deficits {
            // Critical time-to-liquidation estimate: (B_liq / sigma_E)^2
            let time_to_liq_sec = if vol_usd > 1e-4 && b_liq > 0.0 {
                (b_liq / vol_usd).powi(2)
            } else if b_liq <= 0.0 {
                0.0
            } else {
                3600.0
            };

            for (surp_venue, avail_amount) in surpluses.iter_mut() {
                if needed_amount <= 1.0 {
                    break;
                }
                if *avail_amount <= 1.0 {
                    continue;
                }

                // Check rail compatibility
                if let Some(&(_, _, latency, fee_pct)) = rail_latencies
                    .iter()
                    .find(|(f, t, _, _)| f == surp_venue && t == &def_venue)
                {
                    if (time_to_liq_sec <= 0.0 || latency < time_to_liq_sec)
                        && latency <= self.critical_time_to_liq_sec
                    {
                        let transfer_amt = needed_amount.min(*avail_amount);
                        needed_amount -= transfer_amt;
                        *avail_amount -= transfer_amt;

                        transfers.push(CollateralRebalanceTransfer {
                            from_venue: surp_venue.clone(),
                            to_venue: def_venue.clone(),
                            amount_usd: transfer_amt,
                            estimated_fee_usd: transfer_amt * fee_pct,
                            transfer_latency_sec: latency,
                        });
                    }
                }
            }
        }

        transfers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_venue_account_margin_state_and_barriers() {
        // Venue A: Long 5.0 BTC at $65,000 = $325,000 notional. Equity = $35,000. MMR = 5% ($16,250).
        let venue_a = VenueAccountMarginState::new(
            "hyperliquid",
            35_000.0,
            325_000.0,
            0.05,
            500.0,
            325_000.0,
        );
        assert_eq!(venue_a.maintenance_margin_usd(), 16_250.0);
        assert_eq!(venue_a.liquidation_barrier_usd(), 18_750.0);

        // Under 25% crash (-0.25):
        // Stressed equity = 35,000 - 0.25 * 325,000 = -46,250.
        // Stressed MMR = 325,000 * 1.25 * 0.05 = 20,312.50.
        // Stressed barrier = -46,250 - 20,312.50 = -66,562.50 (< 0, breached!).
        let stressed_barrier = venue_a.stressed_liquidation_barrier_usd(-0.25);
        assert!(
            stressed_barrier < 0.0,
            "Long leg must breach barrier under 25% crash"
        );

        // Venue B: Short 5.0 BTC at $65,000 = $325,000 notional. Equity = $35,000. MMR = 5%.
        let venue_b =
            VenueAccountMarginState::new("lighter", 35_000.0, 325_000.0, 0.05, 500.0, -325_000.0);
        // Under 25% crash (-0.25):
        // Stressed equity = 35,000 + 0.25 * 325,000 = 116,250.
        // Stressed barrier = 116,250 - 20,312.50 = +95,937.50 (> 0, massive surplus).
        let stressed_barrier_b = venue_b.stressed_liquidation_barrier_usd(-0.25);
        assert!(
            stressed_barrier_b > 50_000.0,
            "Short leg must gain massive surplus under 25% crash"
        );
    }

    #[test]
    fn test_stress_matrix_evaluation() {
        let rebalancer = CrossVenueCollateralBarrierRebalancer::default();
        let venue_a = VenueAccountMarginState::new(
            "hyperliquid",
            35_000.0,
            325_000.0,
            0.05,
            500.0,
            325_000.0,
        );
        let venue_b =
            VenueAccountMarginState::new("lighter", 35_000.0, 325_000.0, 0.05, 500.0, -325_000.0);

        let matrix = rebalancer.evaluate_stress_matrix(&[venue_a.clone(), venue_b.clone()], None);
        assert!(!matrix.is_empty());

        // Check breach detection
        assert!(rebalancer.has_stress_liquidation_breach(&[venue_a, venue_b], None));
    }

    #[test]
    fn test_proactive_rebalancing_transfers() {
        let rebalancer = CrossVenueCollateralBarrierRebalancer::new(0.20, 60.0);

        // Account A needs buffer: notional $100k, equity $15k, MMR 5% ($5k). B_liq = $10k. Target = 20% * $100k = $20k. Deficit = $10k.
        let acc_a =
            VenueAccountMarginState::new("venue_a", 15_000.0, 100_000.0, 0.05, 200.0, 100_000.0);
        // Account B has surplus: notional $100k, equity $50k, MMR 5% ($5k). B_liq = $45k. Target = $20k. Surplus = $25k.
        let acc_b =
            VenueAccountMarginState::new("venue_b", 50_000.0, 100_000.0, 0.05, 200.0, -100_000.0);

        let rails = vec![("venue_b", "venue_a", 15.0, 0.001)]; // 15s latency, 10 bps fee

        let transfers = rebalancer.plan_proactive_rebalancing(&[acc_a, acc_b], &rails);
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].from_venue, "venue_b");
        assert_eq!(transfers[0].to_venue, "venue_a");
        assert_eq!(transfers[0].amount_usd, 10_000.0);
        assert_eq!(transfers[0].estimated_fee_usd, 10.0);
    }
}
