//! Volume-Synchronized Probability of Toxicity (VPIN)
//! and Almgren-Chriss Multi-Venue Quadratic Smart Order Router.
//!
//! Provides:
//! - Volume-Synchronized Probability of Toxicity (VPIN) engine for high-frequency crypto feeds:
//!   $$\text{VPIN}_t = \frac{\sum_{\tau=t-N+1}^t |V_\tau^B - V_\tau^S|}{N \cdot V}$$
//!   detecting toxic informed directional sweeps before orderbook exhaustion.
//! - Almgren-Chriss Multi-Venue Quadratic SOR Solver:
//!   $$\min_{\{q_1, \dots, q_M\}} \sum_{i=1}^M \left( c_i q_i + \frac{1}{2} \lambda_i q_i^2 \right) \quad \text{s.t.} \quad \sum_{i=1}^M q_i = Q, \quad 0 \le q_i \le Q_i^{\max}$$
//!   solved in closed form via exact KKT Lagrange multiplier search.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Volume-Synchronized Probability of Toxicity (VPIN) Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpinEngine {
    /// Volume capacity per bucket V = ADV / M_buckets
    pub bucket_volume: f64,
    /// Number of volume buckets in rolling window (N, typically 50)
    pub num_buckets: usize,
    /// Critical toxicity threshold (typically 0.70 - 0.75)
    pub toxicity_threshold: f64,
    /// Accumulated buyer-initiated volume in current incomplete bucket
    pub current_buy_vol: f64,
    /// Accumulated seller-initiated volume in current incomplete bucket
    pub current_sell_vol: f64,
    /// Rolling history of bucket imbalances |V_B - V_S|
    pub bucket_imbalances: VecDeque<f64>,
    /// Current calculated VPIN metric in [0.0, 1.0]
    pub current_vpin: f64,
}

impl VpinEngine {
    pub fn new(bucket_volume: f64, num_buckets: usize, toxicity_threshold: f64) -> Self {
        Self {
            bucket_volume: bucket_volume.max(1.0),
            num_buckets: num_buckets.max(5),
            toxicity_threshold: toxicity_threshold.clamp(0.40, 0.95),
            current_buy_vol: 0.0,
            current_sell_vol: 0.0,
            bucket_imbalances: VecDeque::with_capacity(num_buckets),
            current_vpin: 0.25, // Uninformed baseline prior
        }
    }

    /// Ingests an incoming trade tick.
    /// `size`: traded quantity, `is_buyer_taker`: true if buyer aggressed.
    /// Returns (current_vpin, is_toxic_circuit_triggered).
    pub fn ingest_trade(&mut self, size: f64, is_buyer_taker: bool) -> (f64, bool) {
        if size <= 0.0 || !size.is_finite() {
            return (self.current_vpin, self.is_toxic());
        }

        let mut rem_size = size;
        while rem_size > 0.0 {
            let filled_in_bucket = self.current_buy_vol + self.current_sell_vol;
            let bucket_space = self.bucket_volume - filled_in_bucket;

            let chunk = rem_size.min(bucket_space);
            if is_buyer_taker {
                self.current_buy_vol += chunk;
            } else {
                self.current_sell_vol += chunk;
            }
            rem_size -= chunk;

            // Check if bucket is full
            if (self.current_buy_vol + self.current_sell_vol) >= self.bucket_volume - 1e-9 {
                let imbalance = (self.current_buy_vol - self.current_sell_vol).abs();
                if self.bucket_imbalances.len() >= self.num_buckets {
                    self.bucket_imbalances.pop_front();
                }
                self.bucket_imbalances.push_back(imbalance);

                // Recompute VPIN: sum(|V_B - V_S|) / (N * V)
                let sum_imbalance: f64 = self.bucket_imbalances.iter().sum();
                let denom = self.bucket_imbalances.len() as f64 * self.bucket_volume;
                if denom > 0.0 {
                    self.current_vpin = (sum_imbalance / denom).clamp(0.0, 1.0);
                }

                // Reset for next volume bucket
                self.current_buy_vol = 0.0;
                self.current_sell_vol = 0.0;
            }
        }

        (self.current_vpin, self.is_toxic())
    }

    /// Returns true if current VPIN exceeds the critical toxicity threshold.
    pub fn is_toxic(&self) -> bool {
        self.current_vpin >= self.toxicity_threshold
    }
}

/// Venue candidate specification for Multi-Venue SOR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenueExecutionSpec {
    pub venue_id: String,
    /// Half-spread plus taker fee c_i = 0.5 * S_i + fee_i
    pub half_spread_plus_fee: f64,
    /// Temporary quadratic price impact parameter lambda_i
    pub impact_parameter_lambda: f64,
    /// Maximum execution capacity Q_i^max before severe exhaustion
    pub max_capacity: f64,
}

/// Almgren-Chriss Multi-Venue Quadratic SOR Solver.
pub struct MultiVenueQuadraticSor;

impl MultiVenueQuadraticSor {
    /// Solves the optimal order splitting:
    /// $$\min \sum_{i=1}^M \left( c_i q_i + \frac{1}{2} \lambda_i q_i^2 \right) \quad \text{s.t.} \quad \sum q_i = Q, \quad 0 \le q_i \le Q_i^{\max}$$
    /// using exact KKT water-filling root search.
    pub fn split_order(total_quantity: f64, venues: &[VenueExecutionSpec]) -> Vec<(String, f64)> {
        let m = venues.len();
        if m == 0 || total_quantity <= 0.0 {
            return vec![];
        }

        let mut allocations = vec![0.0; m];
        let mut lower_bound = 0.0;
        let mut upper_bound = 1e8;

        // Binary search for Lagrange multiplier mu equating marginal costs
        for _ in 0..64 {
            let mu = 0.5 * (lower_bound + upper_bound);
            let mut sum_q = 0.0;

            for i in 0..m {
                let v = &venues[i];
                let unconstrained_q =
                    (mu - v.half_spread_plus_fee) / v.impact_parameter_lambda.max(1e-6);
                let q = unconstrained_q.clamp(0.0, v.max_capacity);
                allocations[i] = q;
                sum_q += q;
            }

            if sum_q < total_quantity {
                lower_bound = mu;
            } else {
                upper_bound = mu;
            }

            if (sum_q - total_quantity).abs() < 1e-7 {
                break;
            }
        }

        venues
            .iter()
            .enumerate()
            .map(|(i, v)| (v.venue_id.clone(), allocations[i]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vpin_balanced_vs_toxic_sweep() {
        let mut engine = VpinEngine::new(10.0, 10, 0.70);

        // Feed balanced flow: 5 buy, 5 sell per bucket
        for _ in 0..20 {
            engine.ingest_trade(5.0, true);
            engine.ingest_trade(5.0, false);
        }
        // VPIN should be near 0.0 for perfectly balanced flow
        assert!(engine.current_vpin < 0.10);
        assert!(!engine.is_toxic());

        // Now simulate a toxic sell sweep: 10 sell consecutively for 10 buckets
        for _ in 0..10 {
            engine.ingest_trade(10.0, false);
        }
        // VPIN should approach 1.0
        assert!(engine.current_vpin > 0.80);
        assert!(engine.is_toxic());
    }

    #[test]
    fn test_multi_venue_quadratic_sor() {
        let venues = vec![
            VenueExecutionSpec {
                venue_id: "hyperliquid".to_string(),
                half_spread_plus_fee: 0.0003, // 3 bps
                impact_parameter_lambda: 0.00001,
                max_capacity: 50.0,
            },
            VenueExecutionSpec {
                venue_id: "lighter".to_string(),
                half_spread_plus_fee: 0.0004, // 4 bps
                impact_parameter_lambda: 0.000015,
                max_capacity: 40.0,
            },
            VenueExecutionSpec {
                venue_id: "binance".to_string(),
                half_spread_plus_fee: 0.0005,      // 5 bps
                impact_parameter_lambda: 0.000005, // very deep book, low impact
                max_capacity: 100.0,
            },
        ];

        let splits = MultiVenueQuadraticSor::split_order(60.0, &venues);
        assert_eq!(splits.len(), 3);

        let total_allocated: f64 = splits.iter().map(|(_, q)| q).sum();
        assert!((total_allocated - 60.0).abs() < 1e-4);

        // Hyperliquid has lowest fee, should receive allocation
        let hl_alloc = splits.iter().find(|(v, _)| v == "hyperliquid").unwrap().1;
        assert!(hl_alloc > 0.0);
    }
}
