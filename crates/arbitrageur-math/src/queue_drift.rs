//! Cross-Venue Latency Arbitrage & Top-of-Book Queue Drift Simulation.
//!
//! Provides high-frequency microstructure simulation of limit order book queues:
//! 1. Discrete Markov pure-death absorbing chain for limit order fill probabilities
//!    competing against cancellations ahead and adverse price sweeps.
//! 2. Continuous diffusion first-passage fill probability (Cont-de Larrard limit).
//! 3. Expected time-to-fill harmonic summation under position-dependent cancellation intensity.
//! 4. Dynamic sub-tick quote shading engine (`QuoteShadingEngine`) adjusting maker bid/ask
//!    distance based on queue depletion velocity, informed trading hazard rate, inventory skew,
//!    and multivariate Hawkes branching stability circuit breakers.

use crate::black_scholes::standard_normal_cdf;
use serde::{Deserialize, Serialize};

/// High-frequency queue depletion and fill probability simulator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueueDriftSimulator {
    /// Expected arrival intensity of trade executions (market orders) hitting the queue (\lambda_{trade} > 0).
    pub trade_arrival_rate: f64,
    /// Aggregate cancellation intensity of limit orders in the queue (\lambda_{cancel} >= 0).
    pub cancel_rate: f64,
    /// Intensity of adverse price jumps / sweeps wiping out the level (\lambda_{jump} >= 0).
    pub adverse_jump_rate: f64,
}

impl Default for QueueDriftSimulator {
    fn default() -> Self {
        Self {
            trade_arrival_rate: 5.0,
            cancel_rate: 3.0,
            adverse_jump_rate: 0.2,
        }
    }
}

impl QueueDriftSimulator {
    pub fn new(trade_arrival_rate: f64, cancel_rate: f64, adverse_jump_rate: f64) -> Self {
        Self {
            trade_arrival_rate: trade_arrival_rate.max(1e-4),
            cancel_rate: cancel_rate.max(0.0),
            adverse_jump_rate: adverse_jump_rate.max(0.0),
        }
    }

    /// Discrete Markov pure-death absorbing chain fill probability:
    ///
    /// At each state j (orders ahead, from k down to 1), the order transitions to j - 1
    /// via a trade (rate \lambda_{trade}) or a cancellation among the j orders ahead
    /// (rate \lambda_{cancel} * j / Q). The competing risk is an adverse jump (rate \lambda_{jump}).
    ///
    /// The probability of being filled before an adverse jump wipes out the position is:
    /// P_{fill}(k, Q) = \prod_{j=1}^k \frac{\lambda_{trade} + \lambda_{cancel} \frac{j}{Q}}{\lambda_{trade} + \lambda_{cancel} \frac{j}{Q} + \lambda_{jump}}
    pub fn discrete_markov_fill_prob(&self, queue_pos: usize, total_queue: usize) -> f64 {
        if queue_pos == 0 {
            return 1.0;
        }
        let q_tot = (total_queue.max(queue_pos)) as f64;
        let mut prob = 1.0;

        for j in 1..=queue_pos {
            let j_f64 = j as f64;
            let dep_rate = self.trade_arrival_rate + self.cancel_rate * (j_f64 / q_tot);
            let total_rate = dep_rate + self.adverse_jump_rate;
            if total_rate > 1e-9 {
                prob *= dep_rate / total_rate;
            }
        }

        prob.clamp(0.0, 1.0)
    }

    /// Continuous diffusion limit first-passage fill probability within horizon \tau:
    /// P_{fill}(\tau | k) = \Phi( \frac{\mu_Q \tau - k}{\sigma_Q \sqrt{\tau}} )
    ///                   - \exp( \frac{2 \mu_Q k}{\sigma_Q^2} ) \Phi( \frac{-\mu_Q \tau - k}{\sigma_Q \sqrt{\tau}} )
    pub fn continuous_diffusion_fill_prob(
        &self,
        horizon_sec: f64,
        queue_pos: f64,
        drift: f64,
        vol: f64,
    ) -> f64 {
        if queue_pos <= 0.0 {
            return 1.0;
        }
        if horizon_sec <= 1e-6 {
            return 0.0;
        }

        let tau = horizon_sec;
        let mu = drift.max(1e-4);
        let sigma = vol.max(1e-4);
        let denom = sigma * tau.sqrt();

        let arg1 = (mu * tau - queue_pos) / denom;
        let arg2 = (-mu * tau - queue_pos) / denom;

        let phi1 = standard_normal_cdf(arg1);
        let phi2 = standard_normal_cdf(arg2);

        let exp_power = (2.0 * mu * queue_pos) / (sigma * sigma);
        let exp_term = if exp_power > 60.0 {
            0.0
        } else {
            exp_power.exp()
        };

        let p = phi1 - exp_term * phi2;
        p.clamp(0.0, 1.0)
    }

    /// Computes expected time-to-fill (dwell time harmonic summation) in seconds:
    /// \mathbb{E}[\tau_{fill}] = \sum_{j=1}^k \frac{1}{\lambda_{trade} + \lambda_{cancel} \frac{j}{Q}}
    pub fn expected_fill_time(&self, queue_pos: usize, total_queue: usize) -> f64 {
        if queue_pos == 0 {
            return 0.0;
        }
        let q_tot = (total_queue.max(queue_pos)) as f64;
        let mut expected_time = 0.0;

        for j in 1..=queue_pos {
            let j_f64 = j as f64;
            let dep_rate = self.trade_arrival_rate + self.cancel_rate * (j_f64 / q_tot);
            if dep_rate > 1e-6 {
                expected_time += 1.0 / dep_rate;
            }
        }

        expected_time
    }
}

/// Action decision resulting from dynamic quote shading evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuoteAction {
    /// Resting maker quotes must be pulled immediately to avoid toxic sweep.
    Pull { reason: String },
    /// Quote can be placed with dynamic shaded distance.
    Post {
        bid_price: f64,
        ask_price: f64,
        shade_bps: f64,
    },
}

/// Dynamic sub-tick quote shading engine for maker quoting and passive unwinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteShadingEngine {
    /// Sensitivity to queue depletion velocity \alpha_{drift} (e.g. 0.05 bps per unit/sec)
    pub alpha_drift: f64,
    /// Sensitivity to adverse toxic hazard rate \beta_{hazard} (e.g. 2.0 bps)
    pub beta_hazard: f64,
    /// Inventory skew parameter \gamma_{inv} (e.g. 0.10 bps per inventory unit)
    pub gamma_inv: f64,
    /// Maximum allowed spread before quotes are pulled (in bps)
    pub max_spread_bps: f64,
    /// Hawkes process spectral radius threshold for supercritical blowout abort (default 0.95)
    pub supercritical_branching_ratio_threshold: f64,
    /// Toxic hazard rate threshold commanding immediate quote pulling (default 0.85)
    pub toxic_hazard_threshold: f64,
}

impl Default for QuoteShadingEngine {
    fn default() -> Self {
        Self {
            alpha_drift: 0.05,
            beta_hazard: 2.5,
            gamma_inv: 0.10,
            max_spread_bps: 40.0,
            supercritical_branching_ratio_threshold: 0.95,
            toxic_hazard_threshold: 0.85,
        }
    }
}

impl QuoteShadingEngine {
    pub fn new(
        alpha_drift: f64,
        beta_hazard: f64,
        gamma_inv: f64,
        max_spread_bps: f64,
        supercritical_branching_ratio_threshold: f64,
        toxic_hazard_threshold: f64,
    ) -> Self {
        Self {
            alpha_drift: alpha_drift.max(0.0),
            beta_hazard: beta_hazard.max(0.0),
            gamma_inv: gamma_inv.max(0.0),
            max_spread_bps: max_spread_bps.max(1.0),
            supercritical_branching_ratio_threshold: supercritical_branching_ratio_threshold
                .clamp(0.5, 1.2),
            toxic_hazard_threshold: toxic_hazard_threshold.clamp(0.1, 1.0),
        }
    }

    /// Evaluates dynamic quote shading and maker quote safety:
    ///
    /// \delta^*(t) = \delta_0 + \alpha_{drift} \max(0, -v_Q) + \beta_{hazard} \cdot h(t) \pm \gamma_{inv} \cdot q_{inv}
    ///
    /// Pulls quotes if:
    /// 1. Multivariate Hawkes spectral radius \rho(\mathbf{\Gamma}) \ge \rho_{thresh} (cascading jump regime)
    /// 2. Microstructure toxic hazard h(t) \ge h_{thresh}
    /// 3. Computed spread exceeds max_spread_bps
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_quote(
        &self,
        mid_price: f64,
        half_spread_bps: f64,
        tick_size: f64,
        queue_depletion_velocity: f64, // negative if queue is draining rapidly
        adverse_hazard_rate: f64,      // [0.0, 1.0]
        inventory_units: f64,          // positive if long inventory, negative if short
        hawkes_spectral_radius: f64,   // \rho(\Gamma)
    ) -> QuoteAction {
        if mid_price <= 0.0 || tick_size <= 0.0 {
            return QuoteAction::Pull {
                reason: "Invalid mid price or tick size".to_string(),
            };
        }

        // 1. Hawkes stability circuit breaker
        if hawkes_spectral_radius >= self.supercritical_branching_ratio_threshold {
            return QuoteAction::Pull {
                reason: format!(
                    "Supercritical Hawkes branching cascade: rho={:.3} >= {:.3}",
                    hawkes_spectral_radius, self.supercritical_branching_ratio_threshold
                ),
            };
        }

        // 2. Toxic flow hazard circuit breaker
        if adverse_hazard_rate >= self.toxic_hazard_threshold {
            return QuoteAction::Pull {
                reason: format!(
                    "Adverse toxic flow hazard breach: hazard={:.3} >= {:.3}",
                    adverse_hazard_rate, self.toxic_hazard_threshold
                ),
            };
        }

        // 3. Dynamic shading components
        let v_deplete = (-queue_depletion_velocity).max(0.0);
        let drift_shade_bps = (self.alpha_drift * v_deplete).min(30.0);
        let hazard_shade_bps = self.beta_hazard * adverse_hazard_rate.clamp(0.0, 1.0);
        let inv_skew_bps = self.gamma_inv * inventory_units;

        let base_half_bps = half_spread_bps.max(0.5);
        let total_half_bps = base_half_bps + drift_shade_bps + hazard_shade_bps;

        if total_half_bps * 2.0 > self.max_spread_bps {
            return QuoteAction::Pull {
                reason: format!(
                    "Shaded spread {:.2} bps exceeds max limit {:.2} bps",
                    total_half_bps * 2.0,
                    self.max_spread_bps
                ),
            };
        }

        // Long inventory: shade quotes down (bid lower, ask lower) to encourage selling
        // Short inventory: shade quotes up (bid higher, ask higher) to encourage buying
        let bid_half_bps = (total_half_bps + inv_skew_bps).max(0.2);
        let ask_half_bps = (total_half_bps - inv_skew_bps).max(0.2);

        let unrounded_bid = mid_price * (1.0 - bid_half_bps * 1e-4);
        let unrounded_ask = mid_price * (1.0 + ask_half_bps * 1e-4);

        // Snap to tick size
        let bid_price = (unrounded_bid / tick_size).floor() * tick_size;
        let ask_price = (unrounded_ask / tick_size).ceil() * tick_size;

        QuoteAction::Post {
            bid_price,
            ask_price,
            shade_bps: drift_shade_bps + hazard_shade_bps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discrete_markov_fill_prob_monotonicity() {
        let sim = QueueDriftSimulator::new(5.0, 2.0, 0.5);
        let p_front = sim.discrete_markov_fill_prob(1, 100);
        let p_mid = sim.discrete_markov_fill_prob(50, 100);
        let p_back = sim.discrete_markov_fill_prob(99, 100);

        assert!(
            p_front > p_mid,
            "Front of queue should have higher fill probability than mid"
        );
        assert!(
            p_mid > p_back,
            "Mid of queue should have higher fill probability than back"
        );
        assert_eq!(sim.discrete_markov_fill_prob(0, 100), 1.0);
    }

    #[test]
    fn test_continuous_diffusion_fill_prob() {
        let sim = QueueDriftSimulator::default();
        let p_shallow = sim.continuous_diffusion_fill_prob(2.0, 5.0, 3.0, 1.5);
        let p_deep = sim.continuous_diffusion_fill_prob(2.0, 20.0, 3.0, 1.5);

        assert!(p_shallow > p_deep);
        assert!(p_shallow > 0.0 && p_shallow <= 1.0);
    }

    #[test]
    fn test_expected_fill_time_harmonic_sum() {
        let sim = QueueDriftSimulator::new(10.0, 0.0, 0.0);
        // With cancel_rate = 0, rate is fixed at 10.0, so time = pos / 10.0
        let t1 = sim.expected_fill_time(10, 100);
        assert!((t1 - 1.0).abs() < 1e-6);

        let t2 = sim.expected_fill_time(20, 100);
        assert!((t2 - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_quote_shading_normal_vs_supercritical_pull() {
        let engine = QuoteShadingEngine::default();

        // Normal subcritical conditions
        let res_normal = engine.evaluate_quote(
            50_000.0, 2.0, 0.5, -2.0, // draining at 2 units/sec
            0.15, // low hazard
            0.0,  // flat inventory
            0.45, // subcritical Hawkes
        );

        match res_normal {
            QuoteAction::Post {
                bid_price,
                ask_price,
                shade_bps,
            } => {
                assert!(bid_price < 50_000.0);
                assert!(ask_price > 50_000.0);
                assert!(shade_bps > 0.0);
            }
            QuoteAction::Pull { .. } => panic!("Should post in normal conditions"),
        }

        // Supercritical Hawkes blowout
        let res_super = engine.evaluate_quote(50_000.0, 2.0, 0.5, -2.0, 0.15, 0.0, 0.98);
        match res_super {
            QuoteAction::Pull { reason } => {
                assert!(reason.contains("Supercritical Hawkes branching cascade"));
            }
            QuoteAction::Post { .. } => panic!("Should pull in supercritical regime"),
        }
    }

    #[test]
    fn test_inventory_skew_quote_shading() {
        let engine = QuoteShadingEngine::default();

        // Long inventory: should shade prices downward to unload
        let res_long = engine.evaluate_quote(100.0, 5.0, 0.01, 0.0, 0.0, 10.0, 0.2);
        // Short inventory: should shade prices upward to cover
        let res_short = engine.evaluate_quote(100.0, 5.0, 0.01, 0.0, 0.0, -10.0, 0.2);

        if let (
            QuoteAction::Post {
                bid_price: bid_long,
                ask_price: ask_long,
                ..
            },
            QuoteAction::Post {
                bid_price: bid_short,
                ask_price: ask_short,
                ..
            },
        ) = (res_long, res_short)
        {
            assert!(
                bid_long < bid_short,
                "Long inventory should lower bids to avoid buying more"
            );
            assert!(
                ask_long < ask_short,
                "Long inventory should lower asks to facilitate selling"
            );
        } else {
            panic!("Both should post");
        }
    }
}
