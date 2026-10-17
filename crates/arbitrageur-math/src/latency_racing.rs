//! Cross-Venue Microstructure Latency Racing and Pre-Flight Abort Controller.
//!
//! Models the stochastic execution race against competing high-frequency snipers across
//! geographically disparate venues (e.g. Tokyo AWS, Virginia AWS, Singapore, L2 rollups).
//!
//! Formulates:
//! 1. Stochastic network latency distributions: **Shifted Weibull** and **Shifted Log-Normal**
//!    capturing physical fiber propagation floors $t_{\min}$ and heavy right-tailed jitter.
//! 2. Competitive racing win probability $P(\text{Win} \mid \Delta t_{\text{detection}})$ under asymmetric detection lags.
//! 3. Closed-form pre-flight abort decision boundary: aborting multi-leg cross-venue order dispatches
//!    when $P(\text{Win}) < P^* = \frac{C_{\text{failed\_leg}}}{E[\text{GrossEdge}] + C_{\text{failed\_leg}}}$.

use serde::{Deserialize, Serialize};

/// High-frequency stochastic latency distribution model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LatencyDistribution {
    /// Shifted Weibull: $F(t) = 1 - \exp\left(-\left(\frac{t - t_0}{\lambda}\right)^k\right)$ for $t \ge t_0$.
    ShiftedWeibull {
        t0_ms: f64,
        scale_lambda: f64,
        shape_k: f64,
    },
    /// Shifted Log-Normal: $t - t_0 \sim \operatorname{Lognormal}(\mu, \sigma)$ for $t > t_0$.
    ShiftedLogNormal { t0_ms: f64, mu: f64, sigma: f64 },
    /// Empirical summary from real-world ping / WebSocket round-trip measurements.
    EmpiricalSummary {
        t0_ms: f64,
        mean_ms: f64,
        std_ms: f64,
    },
}

impl LatencyDistribution {
    /// Creates a Shifted Weibull distribution from minimum wire latency $t_0$, scale, and shape.
    pub fn new_shifted_weibull(t0_ms: f64, scale_lambda: f64, shape_k: f64) -> Self {
        Self::ShiftedWeibull {
            t0_ms: t0_ms.max(0.0),
            scale_lambda: scale_lambda.max(0.01),
            shape_k: shape_k.clamp(0.2, 10.0),
        }
    }

    /// Creates a Shifted Log-Normal distribution.
    pub fn new_shifted_lognormal(t0_ms: f64, mu: f64, sigma: f64) -> Self {
        Self::ShiftedLogNormal {
            t0_ms: t0_ms.max(0.0),
            mu,
            sigma: sigma.clamp(0.01, 5.0),
        }
    }

    /// Creates an empirical latency distribution from sample mean, standard deviation, and floor.
    pub fn new_empirical(t0_ms: f64, mean_ms: f64, std_ms: f64) -> Self {
        Self::EmpiricalSummary {
            t0_ms: t0_ms.max(0.0),
            mean_ms: mean_ms.max(t0_ms),
            std_ms: std_ms.max(0.01),
        }
    }

    /// Expected execution latency in milliseconds: $\mathbb{E}[\tau]$.
    pub fn mean(&self) -> f64 {
        match *self {
            Self::ShiftedWeibull {
                t0_ms,
                scale_lambda,
                shape_k,
            } => {
                // E[t] = t0 + lambda * Gamma(1 + 1/k)
                t0_ms + scale_lambda * gamma_approx(1.0 + 1.0 / shape_k)
            }
            Self::ShiftedLogNormal { t0_ms, mu, sigma } => {
                // E[t] = t0 + exp(mu + 0.5 * sigma^2)
                t0_ms + (mu + 0.5 * sigma * sigma).exp()
            }
            Self::EmpiricalSummary { mean_ms, .. } => mean_ms,
        }
    }

    /// Latency variance in $\text{ms}^2$: $\operatorname{Var}(\tau)$.
    pub fn variance(&self) -> f64 {
        match *self {
            Self::ShiftedWeibull {
                scale_lambda,
                shape_k,
                ..
            } => {
                let g1 = gamma_approx(1.0 + 1.0 / shape_k);
                let g2 = gamma_approx(1.0 + 2.0 / shape_k);
                (scale_lambda * scale_lambda * (g2 - g1 * g1)).max(1e-6)
            }
            Self::ShiftedLogNormal { mu, sigma, .. } => {
                let s2 = sigma * sigma;
                ((2.0 * mu + s2).exp() * (s2.exp() - 1.0)).max(1e-6)
            }
            Self::EmpiricalSummary { std_ms, .. } => (std_ms * std_ms).max(1e-6),
        }
    }

    /// Latency standard deviation (jitter) in milliseconds.
    pub fn std_dev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Cumulative Distribution Function: $F(t) = P(\tau \le t)$.
    pub fn cdf(&self, t_ms: f64) -> f64 {
        match *self {
            Self::ShiftedWeibull {
                t0_ms,
                scale_lambda,
                shape_k,
            } => {
                if t_ms <= t0_ms {
                    0.0
                } else {
                    let z = (t_ms - t0_ms) / scale_lambda;
                    let exp_term = (-z.powf(shape_k)).exp();
                    (1.0 - exp_term).clamp(0.0, 1.0)
                }
            }
            Self::ShiftedLogNormal { t0_ms, mu, sigma } => {
                if t_ms <= t0_ms {
                    0.0
                } else {
                    let z = ((t_ms - t0_ms).ln() - mu) / sigma;
                    crate::autotune::norm_cdf(z).unwrap_or(0.5).clamp(0.0, 1.0)
                }
            }
            Self::EmpiricalSummary {
                mean_ms, std_ms, ..
            } => {
                let z = (t_ms - mean_ms) / std_ms.max(1e-4);
                crate::autotune::norm_cdf(z).unwrap_or(0.5).clamp(0.0, 1.0)
            }
        }
    }
}

/// Stirling approximation for Gamma function $\Gamma(z)$ accurate for $z \in [0.5, 5.0]$.
fn gamma_approx(z: f64) -> f64 {
    if z <= 0.0 {
        return 1.0;
    }
    // Nemes (2010) closed-form Gamma approximation
    let term1 = (2.0 * std::f64::consts::PI / z).sqrt();
    let term2 = (1.0 / std::f64::consts::E) * (z + 1.0 / (12.0 * z - 1.0 / (10.0 * z)));
    term1 * term2.powf(z)
}

/// Stochastic Latency Racing Model between our system and competing snipers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossVenueRacingModel {
    pub our_latency: LatencyDistribution,
    pub competitor_latency: LatencyDistribution,
}

impl CrossVenueRacingModel {
    pub fn new(our_latency: LatencyDistribution, competitor_latency: LatencyDistribution) -> Self {
        Self {
            our_latency,
            competitor_latency,
        }
    }

    /// Probability that our order reaches the target exchange matching engine ahead of the competitor:
    ///
    /// $$P(\text{Win} \mid \Delta t_{\text{det}}) = P(\tau_{\text{our}} < \tau_{\text{comp}} + \Delta t_{\text{det}})$$
    ///
    /// where:
    /// - $\Delta t_{\text{det}} > 0$: We detected the opportunity before the competitor.
    /// - $\Delta t_{\text{det}} < 0$: The competitor detected the opportunity before us.
    pub fn win_probability(&self, detection_lead_ms: f64) -> f64 {
        let mu_our = self.our_latency.mean();
        let mu_comp = self.competitor_latency.mean();
        let var_our = self.our_latency.variance();
        let var_comp = self.competitor_latency.variance();

        // Difference variable D = tau_our - tau_comp.
        // E[D] = mu_our - mu_comp
        // Var(D) = var_our + var_comp
        let mu_d = mu_our - mu_comp;
        let sigma_d = (var_our + var_comp).sqrt().max(1e-4);

        // Win condition: D < detection_lead_ms
        let z = (detection_lead_ms - mu_d) / sigma_d;
        let p_win = crate::autotune::norm_cdf(z).unwrap_or(0.5);
        p_win.clamp(0.0001, 0.9999)
    }

    /// Computes optimal dispatch lead-time offset $\Delta \tau^*$ to synchronize arrival:
    ///
    /// $$\Delta \tau^* = \max(0, \mathbb{E}[\tau_{\text{slow}}] - \mathbb{E}[\tau_{\text{fast}}])$$
    pub fn compute_arrival_alignment_offset_ms(
        mean_latency_a_ms: f64,
        mean_latency_b_ms: f64,
    ) -> f64 {
        (mean_latency_b_ms - mean_latency_a_ms).abs()
    }
}

/// Pre-flight decision outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreFlightDecision {
    /// Safe to dispatch: estimated win probability exceeds economic hurdle $P^*$.
    Dispatch {
        win_probability: f64,
        required_hurdle_prob: f64,
        expected_pnl_bps: f64,
        optimal_stagger_ms: f64,
    },
    /// Abort execution: win probability is insufficient to cover failed-leg unwinding cost.
    Abort {
        win_probability: f64,
        required_hurdle_prob: f64,
        expected_pnl_bps: f64,
        reason: String,
    },
}

/// Pre-flight Abort Controller protecting multi-leg cross-venue trades from adverse selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreFlightRacingAbortGuard {
    /// Absolute minimum win probability floor (default 0.50 = 50%).
    pub min_win_probability_floor: f64,
    /// Multiplier on failed-leg cost to account for toxic market dislocation during runs (default 1.25).
    pub adverse_cost_multiplier: f64,
}

impl Default for PreFlightRacingAbortGuard {
    fn default() -> Self {
        Self {
            min_win_probability_floor: 0.50,
            adverse_cost_multiplier: 1.25,
        }
    }
}

impl PreFlightRacingAbortGuard {
    pub fn new(min_win_probability_floor: f64, adverse_cost_multiplier: f64) -> Self {
        Self {
            min_win_probability_floor: min_win_probability_floor.clamp(0.10, 0.95),
            adverse_cost_multiplier: adverse_cost_multiplier.max(1.0),
        }
    }

    /// Evaluates pre-flight abort decision:
    ///
    /// Expected payoff:
    /// $$\mathbb{E}[\Pi] = P(\text{Win}) \cdot \text{GrossEdge} - (1 - P(\text{Win})) \cdot C_{\text{failed\_leg}}$$
    ///
    /// Critical break-even hurdle probability:
    /// $$P^* = \frac{C_{\text{failed\_leg}}}{\text{GrossEdge} + C_{\text{failed\_leg}}}$$
    ///
    /// If $P(\text{Win}) < \max(P^*, P_{\text{floor}})$, returns `PreFlightDecision::Abort`.
    pub fn evaluate(
        &self,
        gross_edge_bps: f64,
        failed_leg_cost_bps: f64,
        detection_lead_ms: f64,
        racing_model: &CrossVenueRacingModel,
    ) -> PreFlightDecision {
        if gross_edge_bps <= 0.0 {
            return PreFlightDecision::Abort {
                win_probability: 0.0,
                required_hurdle_prob: 1.0,
                expected_pnl_bps: -failed_leg_cost_bps,
                reason: "Gross edge is non-positive".into(),
            };
        }

        let effective_cost = (failed_leg_cost_bps * self.adverse_cost_multiplier).max(0.1);
        let p_hurdle = effective_cost / (gross_edge_bps + effective_cost);
        let min_required = p_hurdle.max(self.min_win_probability_floor);

        let p_win = racing_model.win_probability(detection_lead_ms);
        let expected_pnl = p_win * gross_edge_bps - (1.0 - p_win) * effective_cost;

        let diff_mean = racing_model.our_latency.mean() - racing_model.competitor_latency.mean();
        let stagger_ms = diff_mean.abs();

        if p_win < min_required {
            PreFlightDecision::Abort {
                win_probability: p_win,
                required_hurdle_prob: min_required,
                expected_pnl_bps: expected_pnl,
                reason: format!(
                    "Win probability {:.2}% below hurdle {:.2}% (expected PnL {:.2} bps)",
                    p_win * 100.0,
                    min_required * 100.0,
                    expected_pnl
                ),
            }
        } else {
            PreFlightDecision::Dispatch {
                win_probability: p_win,
                required_hurdle_prob: min_required,
                expected_pnl_bps: expected_pnl,
                optimal_stagger_ms: stagger_ms,
            }
        }
    }
}

/// Asymmetric Latency Racing & Optimal Limit Cancellation Drift Engine.
///
/// Models the microstructural race between maker cancellation orders and competitive taker sweeps:
///
/// $$P(\tau_{\text{cancel}} < \tau_{\text{taker}}) = \Phi\left( \frac{\mu_{\text{taker}} + \Delta t_{\text{lead}} - \mu_{\text{cancel}}}{\sqrt{\sigma_{\text{cancel}}^2 + \sigma_{\text{taker}}^2}} \right)$$
///
/// Solves:
/// 1. Dynamic quote withdrawal threshold $\Delta P^*(t)$:
///    $$\Delta P^*(t) = \frac{C_{\text{cancel-replace}}}{(1 - P(\text{Cancel Win})) \cdot q_{\text{quote}}} + \frac{1}{2} S_{\text{follow}} + \text{Rebate}$$
/// 2. Continuous drift-adjusted quoting:
///    $$P_{\text{limit}}^*(t) = P_{\text{mid, follow}}(t) \pm \delta_{\text{base}} + \beta_{\text{lead}} \cdot \Delta P_{\text{lead}} e^{-\kappa t} \operatorname{sign}(\text{OFI}_{\text{lead}})$$
/// 3. Adverse drift evaluation in basis points and cancellation triggers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsymmetricLatencyRacingEngine {
    pub mean_cancel_latency_ms: f64,
    pub std_cancel_latency_ms: f64,
    pub mean_taker_latency_ms: f64,
    pub std_taker_latency_ms: f64,
    pub wire_cancel_cost_usd: f64,
    pub queue_seniority_weight: f64,
    pub drift_pass_through_beta: f64,
    pub drift_decay_kappa: f64,
}

/// Type alias for LimitCancelDriftEngine.
pub type LimitCancelDriftEngine = AsymmetricLatencyRacingEngine;

impl Default for AsymmetricLatencyRacingEngine {
    fn default() -> Self {
        Self {
            mean_cancel_latency_ms: 3.5,
            std_cancel_latency_ms: 0.8,
            mean_taker_latency_ms: 4.2,
            std_taker_latency_ms: 1.0,
            wire_cancel_cost_usd: 0.05,
            queue_seniority_weight: 2.5,
            drift_pass_through_beta: 0.85,
            drift_decay_kappa: 4.0,
        }
    }
}

impl AsymmetricLatencyRacingEngine {
    pub fn new(
        mean_cancel_ms: f64,
        std_cancel_ms: f64,
        mean_taker_ms: f64,
        std_taker_ms: f64,
    ) -> Self {
        Self {
            mean_cancel_latency_ms: mean_cancel_ms.max(0.1),
            std_cancel_latency_ms: std_cancel_ms.max(0.01),
            mean_taker_latency_ms: mean_taker_ms.max(0.1),
            std_taker_latency_ms: std_taker_ms.max(0.01),
            wire_cancel_cost_usd: 0.05,
            queue_seniority_weight: 2.5,
            drift_pass_through_beta: 0.85,
            drift_decay_kappa: 4.0,
        }
    }

    /// Evaluates probability of cancellation beating competitive taker sweep:
    ///
    /// $$P(\tau_{\text{cancel}} < \tau_{\text{taker}} + \Delta t_{\text{lead}})$$
    pub fn cancel_win_probability(&self, sniper_detection_lead_ms: f64) -> f64 {
        let diff_mean =
            self.mean_taker_latency_ms + sniper_detection_lead_ms - self.mean_cancel_latency_ms;
        let diff_std =
            (self.std_cancel_latency_ms.powi(2) + self.std_taker_latency_ms.powi(2)).sqrt();
        let z = diff_mean / diff_std.max(1e-6);
        crate::autotune::norm_cdf(z).unwrap_or(0.5).clamp(0.0, 1.0)
    }

    /// Computes dynamic price jump threshold Delta P* triggering quote cancellation
    pub fn compute_withdrawal_threshold(
        &self,
        quote_size_usd: f64,
        queue_pos: usize,
        total_depth: usize,
        spread_usd: f64,
        rebate_usd: f64,
        sniper_lead_ms: f64,
    ) -> f64 {
        let p_win = self.cancel_win_probability(sniper_lead_ms);
        let q_loss = self.queue_seniority_weight * (queue_pos as f64 / total_depth.max(1) as f64);
        let c_cancel = self.wire_cancel_cost_usd + q_loss;

        let risk_factor = (1.0 - p_win).max(0.02);
        let hurdle = c_cancel / (risk_factor * quote_size_usd.max(1.0));

        hurdle + 0.5 * spread_usd + rebate_usd
    }

    /// Computes continuous drift-adjusted limit price
    pub fn drift_adjusted_price(
        &self,
        base_peg_price: f64,
        leader_price_delta: f64,
        leader_ofi: f64,
        elapsed_sec: f64,
    ) -> f64 {
        let decay = (-self.drift_decay_kappa * elapsed_sec).exp();
        let ofi_sign = if leader_ofi > 0.0 {
            1.0
        } else if leader_ofi < 0.0 {
            -1.0
        } else {
            0.0
        };
        let drift = self.drift_pass_through_beta * leader_price_delta * decay * ofi_sign;
        base_peg_price + drift
    }

    /// Computes adverse price drift in basis points of the current market mid against the resting limit price.
    ///
    /// For a Buy order: if current mid falls below limit price, buyer is overpaying (adverse).
    /// For a Sell order: if current mid rises above limit price, seller is underselling (adverse).
    pub fn adverse_drift_bps(side: &str, limit_price: f64, current_mid: f64) -> f64 {
        if limit_price <= 0.0 || current_mid <= 0.0 {
            return 0.0;
        }
        let s = side.to_ascii_lowercase();
        if s == "buy" {
            if current_mid < limit_price {
                ((limit_price - current_mid) / current_mid) * 10_000.0
            } else {
                0.0
            }
        } else if s == "sell" {
            if current_mid > limit_price {
                ((current_mid - limit_price) / current_mid) * 10_000.0
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    /// Evaluates whether an existing resting limit order should be canceled immediately.
    /// Returns (should_cancel, adverse_drift_bps, withdrawal_hurdle_bps).
    #[allow(clippy::too_many_arguments)]
    pub fn should_cancel_resting_order(
        &self,
        side: &str,
        limit_price: f64,
        current_mid: f64,
        quote_size_usd: f64,
        queue_pos: usize,
        total_depth: usize,
        spread_usd: f64,
        rebate_usd: f64,
        sniper_lead_ms: f64,
    ) -> (bool, f64, f64) {
        let adverse_bps = Self::adverse_drift_bps(side, limit_price, current_mid);
        let withdrawal_threshold_usd = self.compute_withdrawal_threshold(
            quote_size_usd,
            queue_pos,
            total_depth,
            spread_usd,
            rebate_usd,
            sniper_lead_ms,
        );
        let hurdle_bps = (withdrawal_threshold_usd / limit_price.max(1e-6)) * 10_000.0;
        let should_cancel = adverse_bps >= hurdle_bps;
        (should_cancel, adverse_bps, hurdle_bps)
    }
}

/// Multi-Venue Latency Staggering Coordinator for N >= 2 concurrent child orders.
/// Calculates staggered dispatch delays Delta t_k to synchronize order arrivals
/// at geographically distributed exchange matching engines, preventing information leakage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiVenueLatencyStaggerEngine {
    pub max_spatial_stagger_ms: f64,
}

impl Default for MultiVenueLatencyStaggerEngine {
    fn default() -> Self {
        Self {
            max_spatial_stagger_ms: 250.0,
        }
    }
}

impl MultiVenueLatencyStaggerEngine {
    pub fn new(max_spatial_stagger_ms: f64) -> Self {
        Self {
            max_spatial_stagger_ms: max_spatial_stagger_ms.clamp(10.0, 1000.0),
        }
    }

    /// Computes per-venue dispatch delay offsets given one-way transit latency estimates in milliseconds.
    /// Returns vector of (venue_name, delay_ms).
    pub fn compute_stagger_offsets(
        &self,
        venues: &[String],
        one_way_latency_ms: &[f64],
    ) -> Vec<(String, f64)> {
        assert_eq!(venues.len(), one_way_latency_ms.len());
        if venues.is_empty() {
            return Vec::new();
        }

        let max_owt_ms = one_way_latency_ms.iter().cloned().fold(0.0, f64::max);

        venues
            .iter()
            .zip(one_way_latency_ms.iter())
            .map(|(v, &owt)| {
                let diff_ms = (max_owt_ms - owt).clamp(0.0, self.max_spatial_stagger_ms);
                (v.clone(), diff_ms)
            })
            .collect()
    }

    /// Computes synchronized arrivals using LatencyDistribution profiles.
    pub fn compute_distribution_stagger_offsets(
        &self,
        venues: &[String],
        distributions: &[LatencyDistribution],
    ) -> Vec<(String, f64)> {
        assert_eq!(venues.len(), distributions.len());
        let expected_latencies: Vec<f64> = distributions.iter().map(|d| d.mean()).collect();
        self.compute_stagger_offsets(venues, &expected_latencies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shifted_weibull_and_lognormal_moments() {
        let weibull = LatencyDistribution::new_shifted_weibull(2.0, 1.5, 1.8);
        assert!(weibull.mean() > 2.0, "mean must exceed floor");
        assert!(weibull.variance() > 0.0, "variance must be positive");
        assert_eq!(weibull.cdf(1.0), 0.0, "cdf below t0 must be 0");
        assert!(weibull.cdf(5.0) > 0.5, "cdf at 5ms should be significant");

        let lognormal = LatencyDistribution::new_shifted_lognormal(1.5, 0.5, 0.4);
        assert!(lognormal.mean() > 1.5);
        assert!(lognormal.std_dev() > 0.0);
        assert_eq!(lognormal.cdf(1.0), 0.0);
    }

    #[test]
    fn test_racing_win_probability_monotonicity() {
        // Our latency: mean ~ 4ms, std ~ 1ms
        let our = LatencyDistribution::new_empirical(2.0, 4.0, 1.0);
        // Competitor latency: mean ~ 5ms, std ~ 1ms
        let comp = LatencyDistribution::new_empirical(2.5, 5.0, 1.0);

        let race = CrossVenueRacingModel::new(our, comp);

        // When detection is simultaneous (lead = 0ms), our system is 1ms faster on average -> P(Win) > 50%
        let p_neutral = race.win_probability(0.0);
        assert!(p_neutral > 0.70, "1ms latency lead gives high win prob");

        // When we have 2ms detection lead, win prob increases
        let p_ahead = race.win_probability(2.0);
        assert!(p_ahead > p_neutral);

        // When competitor has 3ms detection lead (-3.0ms), win prob drops
        let p_behind = race.win_probability(-3.0);
        assert!(p_behind < 0.20);
    }

    #[test]
    fn test_preflight_abort_guard_decisions() {
        let our = LatencyDistribution::new_empirical(2.0, 4.0, 1.0);
        let comp = LatencyDistribution::new_empirical(2.0, 4.0, 1.0);
        let race = CrossVenueRacingModel::new(our, comp);

        let guard = PreFlightRacingAbortGuard::default();

        // Scenario 1: Gross edge 10 bps, failed leg cost 5 bps. Hurdle ~ 5 / (10 + 5) = 33%, capped at floor 50%.
        // Detection lead +2.0 ms -> win prob > 80% -> Safe to dispatch!
        let dec_dispatch = guard.evaluate(10.0, 5.0, 2.0, &race);
        match dec_dispatch {
            PreFlightDecision::Dispatch {
                win_probability,
                expected_pnl_bps,
                ..
            } => {
                assert!(win_probability > 0.80);
                assert!(expected_pnl_bps > 0.0);
            }
            _ => panic!("Expected dispatch"),
        }

        // Scenario 2: Competitor detected 3 ms ahead (-3.0ms) -> win prob ~ 1.6% -> Abort!
        let dec_abort = guard.evaluate(10.0, 5.0, -3.0, &race);
        match dec_abort {
            PreFlightDecision::Abort {
                win_probability,
                reason,
                ..
            } => {
                assert!(win_probability < 0.10);
                assert!(reason.contains("below hurdle"));
            }
            _ => panic!("Expected abort"),
        }
    }

    #[test]
    fn test_asymmetric_latency_cancellation_race_and_drift() {
        let engine = AsymmetricLatencyRacingEngine::new(3.0, 0.5, 4.0, 0.5);

        // Cancel is 1ms faster than taker on average with simultaneous detection -> win prob > 90%
        let p_win = engine.cancel_win_probability(0.0);
        assert!(
            p_win > 0.90,
            "1ms cancel advantage should yield high win probability"
        );

        // When sniper has 2ms lead, cancel win probability drops
        let p_win_sniper_ahead = engine.cancel_win_probability(-2.0);
        assert!(p_win_sniper_ahead < 0.20);

        // Dynamic withdrawal threshold
        let threshold = engine.compute_withdrawal_threshold(
            10_000.0, // $10k quote
            1,        // position 1
            10,       // depth 10
            1.0,      // spread $1.0
            0.1,      // rebate $0.1
            0.0,      // simultaneous lead
        );
        assert!(
            threshold > 0.5,
            "Threshold must cover spread half and rebate"
        );

        // Adverse drift check
        let adverse_buy = AsymmetricLatencyRacingEngine::adverse_drift_bps("buy", 3000.0, 2990.0);
        assert!(adverse_buy > 30.0, "Buy limit above current mid is adverse");

        let adverse_sell = AsymmetricLatencyRacingEngine::adverse_drift_bps("sell", 3000.0, 3010.0);
        assert!(
            adverse_sell > 30.0,
            "Sell limit below current mid is adverse"
        );

        // Non-adverse conditions
        assert_eq!(
            AsymmetricLatencyRacingEngine::adverse_drift_bps("buy", 3000.0, 3010.0),
            0.0
        );
        assert_eq!(
            AsymmetricLatencyRacingEngine::adverse_drift_bps("sell", 3000.0, 2990.0),
            0.0
        );

        // Continuous drift adjustment
        let adj_price = engine.drift_adjusted_price(3000.0, 10.0, 1.0, 0.1);
        assert!(adj_price > 3000.0, "Positive drift should push price up");

        // Cancellation decision
        let (should_cancel, adverse_bps, _) = engine.should_cancel_resting_order(
            "buy", 3000.0, 2980.0, // $20 crash (67 bps adverse)
            10_000.0, 2, 10, 1.0, 0.1, -1.0, // sniper ahead
        );
        assert!(should_cancel);
        assert!(adverse_bps > 60.0);
    }

    #[test]
    fn test_multi_venue_latency_stagger_engine() {
        let engine = MultiVenueLatencyStaggerEngine::default();
        let venues = vec![
            "binance_tokyo".to_string(),
            "okx_hongkong".to_string(),
            "hyperliquid_us".to_string(),
            "deribit_frankfurt".to_string(),
        ];
        // One-way transit latencies (ms)
        let latencies = vec![2.0, 42.0, 135.0, 220.0];

        let offsets = engine.compute_stagger_offsets(&venues, &latencies);
        assert_eq!(offsets.len(), 4);

        // Slowest is Deribit at 220.0 ms -> delay should be 0.0 ms
        assert_eq!(offsets[3].0, "deribit_frankfurt");
        assert_eq!(offsets[3].1, 0.0);

        // Fastest is Binance at 2.0 ms -> delay should be 220.0 - 2.0 = 218.0 ms
        assert_eq!(offsets[0].0, "binance_tokyo");
        assert!((offsets[0].1 - 218.0).abs() < 1e-6);

        // Hyperliquid at 135.0 ms -> delay should be 220.0 - 135.0 = 85.0 ms
        assert_eq!(offsets[2].0, "hyperliquid_us");
        assert!((offsets[2].1 - 85.0).abs() < 1e-6);

        // Distribution-based calculation
        let dists = vec![
            LatencyDistribution::new_empirical(1.0, 2.0, 0.5),
            LatencyDistribution::new_empirical(30.0, 42.0, 3.0),
            LatencyDistribution::new_empirical(120.0, 135.0, 8.0),
            LatencyDistribution::new_empirical(200.0, 220.0, 10.0),
        ];
        let dist_offsets = engine.compute_distribution_stagger_offsets(&venues, &dists);
        assert!((dist_offsets[0].1 - 218.0).abs() < 1e-6);
    }
}
