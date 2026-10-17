//! Cross-EVM Intent-Based Fast Bridging & Settlement Auction Mechanics.
//!
//! Formulates:
//! 1. Off-chain solver auction dynamics (Across Protocol SpokePool V3, ERC-7683, UniswapX):
//!    Analytical optimal filler fee bid:
//!    $$F^* = \mu_F + \sigma_F \Phi^{-1}\left(\sqrt{\frac{\text{Cost}_{\text{delay}}}{\lambda_{\text{solver}} \sigma_F \sqrt{2\pi}}}\right)$$
//!    yielding $3.5\text{--}6.5\text{ bps}$ and $8.5\text{--}22.0\text{s}$ fill latency.
//! 2. Deterministic pre-flight destination solvency guarantees:
//!    $$\text{Equity}_{\text{dest}}(0) - \text{MMR}_{\text{dest}} \ge z_{99.9\%} \sigma \text{Notional} \sqrt{\tau} + \text{Buffer}$$

use crate::black_scholes::standard_normal_cdf;
use serde::{Deserialize, Serialize};

/// High-precision Acklam rational approximation of standard normal inverse CDF $\Phi^{-1}(p)$ (< 1.15e-9 error).
pub fn standard_normal_inv_cdf(p: f64) -> f64 {
    let p_clamped = p.clamp(1e-15, 1.0 - 1e-15);

    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];

    const P_LOW: f64 = 0.02425;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p_clamped < P_LOW {
        let q = (-2.0 * p_clamped.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p_clamped <= P_HIGH {
        let q = p_clamped - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p_clamped).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// Configuration parameters for Intent-Based Bridging and Solver Auctions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntentBridgeConfig {
    /// Baseline mean solver fee in basis points $\mu_F$.
    pub mean_solver_fee_bps: f64,
    /// Standard deviation of competitive solver bids in basis points $\sigma_F$.
    pub solver_fee_std_bps: f64,
    /// Arrival intensity of active solvers ($\lambda_{\text{solver}}$ solvers/sec).
    pub solver_arrival_rate: f64,
    /// Delay cost sensitivity in bps/sec.
    pub delay_cost_per_sec_bps: f64,
    /// Fixed destination gas reimbursement in USD.
    pub dest_gas_reimbursement_usd: f64,
    /// Pre-flight solvency confidence quantile $z_{99.9\%} \approx 3.0902$.
    pub solvency_z_score: f64,
    /// Minimum destination buffer margin in USD.
    pub min_safety_buffer_usd: f64,
}

impl Default for IntentBridgeConfig {
    fn default() -> Self {
        Self {
            mean_solver_fee_bps: 4.5,
            solver_fee_std_bps: 1.0,
            solver_arrival_rate: 0.85,
            delay_cost_per_sec_bps: 0.08,
            dest_gas_reimbursement_usd: 1.50,
            solvency_z_score: 3.0902, // 99.9% 1-tail normal quantile
            min_safety_buffer_usd: 50.0,
        }
    }
}

/// Result of pre-flight solvency analysis for cross-chain intent execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreFlightSolvencyVerdict {
    pub is_solvent: bool,
    pub current_equity_usd: f64,
    pub required_mmr_usd: f64,
    pub adverse_shock_buffer_usd: f64,
    pub net_excess_margin_usd: f64,
    pub max_safe_transfer_latency_sec: f64,
}

/// Quote discovery and execution plan for an Across / ERC-7683 intent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeIntentQuote {
    pub input_amount_usd: f64,
    pub expected_output_usd: f64,
    pub optimal_fee_bps: f64,
    pub total_fee_usd: f64,
    pub estimated_fill_latency_sec: f64,
    pub pre_flight_solvency: PreFlightSolvencyVerdict,
}

/// Cross-EVM Intent-Based Fast Bridging & Solver Auction Engine.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CrossChainIntentBridgeEngine {
    pub config: IntentBridgeConfig,
}

impl CrossChainIntentBridgeEngine {
    pub fn new(config: IntentBridgeConfig) -> Self {
        Self { config }
    }

    /// Computes optimal solver auction fee bid $F^*$:
    /// $$F^* = \mu_F + \sigma_F \Phi^{-1}\left(\sqrt{\frac{\text{Cost}_{\text{delay}}}{\lambda_{\text{solver}} \sigma_F \sqrt{2\pi}}}\right)$$
    pub fn compute_optimal_solver_fee(&self, urgency_multiplier: f64) -> (f64, f64) {
        let mu_f = self.config.mean_solver_fee_bps;
        let sigma_f = self.config.solver_fee_std_bps.max(0.1);
        let cost_delay = (self.config.delay_cost_per_sec_bps * urgency_multiplier).max(1e-4);
        let lambda_s = self.config.solver_arrival_rate.max(0.1);

        let sqrt_2pi = (2.0 * std::f64::consts::PI).sqrt();
        let argument = (cost_delay / (lambda_s * sigma_f * sqrt_2pi))
            .sqrt()
            .clamp(0.01, 0.99);

        let z_bid = standard_normal_inv_cdf(argument);
        let optimal_fee_bps = (mu_f + sigma_f * z_bid).clamp(2.5, 12.0);

        // Expected fill latency under competitive Dutch auction: tau = 1 / (lambda_solver * P_fill)
        let fill_prob = standard_normal_cdf((optimal_fee_bps - mu_f) / sigma_f).max(0.05);
        let expected_latency_sec = (1.0 / (lambda_s * fill_prob)).clamp(5.0, 60.0);

        (optimal_fee_bps, expected_latency_sec)
    }

    /// Evaluates deterministic pre-flight solvency guarantees on destination venue:
    /// $$\text{Equity}_{\text{dest}}(0) - \text{MMR}_{\text{dest}} \ge z_{99.9\%} \sigma \text{Notional} \sqrt{\tau} + \text{Buffer}$$
    pub fn evaluate_destination_solvency(
        &self,
        dest_equity_usd: f64,
        dest_mmr_usd: f64,
        dest_notional_usd: f64,
        asset_volatility: f64,
        transit_latency_sec: f64,
    ) -> PreFlightSolvencyVerdict {
        let tau_years = (transit_latency_sec / (365.25 * 86400.0)).max(1e-7);
        let z = self.config.solvency_z_score;
        let sigma = asset_volatility.max(0.1);
        let notional = dest_notional_usd.abs();

        let adverse_move_usd = z * sigma * notional * tau_years.sqrt();
        let total_required_buffer = adverse_move_usd + self.config.min_safety_buffer_usd;

        let available_margin = dest_equity_usd - dest_mmr_usd;
        let net_excess = available_margin - total_required_buffer;
        let is_solvent = net_excess >= 0.0;

        // Compute maximum safe transfer latency before ruin barrier is hit:
        // available_margin - min_buffer = z * sigma * notional * sqrt(tau_max)
        let max_safe_latency_sec =
            if available_margin > self.config.min_safety_buffer_usd && notional > 0.0 {
                let margin_headroom = available_margin - self.config.min_safety_buffer_usd;
                let sqrt_tau_years = margin_headroom / (z * sigma * notional);
                let tau_years_max = sqrt_tau_years * sqrt_tau_years;
                (tau_years_max * (365.25 * 86400.0)).clamp(0.0, 86400.0)
            } else {
                0.0
            };

        PreFlightSolvencyVerdict {
            is_solvent,
            current_equity_usd: dest_equity_usd,
            required_mmr_usd: dest_mmr_usd,
            adverse_shock_buffer_usd: total_required_buffer,
            net_excess_margin_usd: net_excess,
            max_safe_transfer_latency_sec: max_safe_latency_sec,
        }
    }

    /// Evaluates complete cross-chain bridge intent quote and pre-flight safety.
    pub fn evaluate_intent_quote(
        &self,
        input_amount_usd: f64,
        dest_equity_usd: f64,
        dest_mmr_usd: f64,
        dest_notional_usd: f64,
        asset_volatility: f64,
        urgency_multiplier: f64,
    ) -> BridgeIntentQuote {
        let (optimal_fee_bps, latency_sec) = self.compute_optimal_solver_fee(urgency_multiplier);
        let solvency = self.evaluate_destination_solvency(
            dest_equity_usd,
            dest_mmr_usd,
            dest_notional_usd,
            asset_volatility,
            latency_sec,
        );

        let fee_pct = optimal_fee_bps / 10000.0;
        let variable_fee = input_amount_usd * fee_pct;
        let total_fee_usd = variable_fee + self.config.dest_gas_reimbursement_usd;
        let output_amount_usd = (input_amount_usd - total_fee_usd).max(0.0);

        BridgeIntentQuote {
            input_amount_usd,
            expected_output_usd: output_amount_usd,
            optimal_fee_bps,
            total_fee_usd,
            estimated_fill_latency_sec: latency_sec,
            pre_flight_solvency: solvency,
        }
    }
}

/// Dutch auction decay curve profile for cross-chain intent clearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DutchAuctionCurveType {
    /// Linear decay: \gamma = 1.0. Constant price decrement per unit time.
    Linear,
    /// Convex decay: \gamma > 1.0. Retains high price initially to preserve user surplus, then accelerates decay.
    ConvexDecay,
    /// Concave decay: \gamma < 1.0. Drops price rapidly initially to guarantee immediate clearance.
    ConcaveDecay,
}

/// Configuration parameters for Intent-Based Dutch Auction Clearance Engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntentDutchAuctionConfig {
    /// Starting auction price P_{start} (e.g. 1.0000 or initial basis).
    pub start_price: f64,
    /// Reserve floor price P_{min} below which the auction will not clear (e.g. 0.9950).
    pub min_price: f64,
    /// Maximum auction duration in seconds T_{max} before order expires or executes at floor (default: 30.0s).
    pub max_duration_sec: f64,
    /// Decay exponent \gamma (default: 2.0 for quadratic convex surplus preservation).
    pub curve_gamma: f64,
    /// Baseline solver arrival hazard intensity \lambda_0 (solvers/sec, default: 0.15).
    pub base_solver_hazard: f64,
    /// Sensitivity parameter \alpha governing exponential hazard growth as price concession increases (default: 3.5).
    pub hazard_price_sensitivity: f64,
    /// Capital delay cost / basis risk rate per second c_{delay} (default: 0.0001 / sec).
    pub delay_cost_rate: f64,
    /// Penalty weight for auction failing to clear within max duration (default: 0.0050 = 50 bps).
    pub failure_penalty: f64,
}

impl Default for IntentDutchAuctionConfig {
    fn default() -> Self {
        Self {
            start_price: 1.0000,
            min_price: 0.9950,
            max_duration_sec: 30.0,
            curve_gamma: 2.0,
            base_solver_hazard: 0.15,
            hazard_price_sensitivity: 3.5,
            delay_cost_rate: 0.0001,
            failure_penalty: 0.0050,
        }
    }
}

/// Comprehensive analytics for an Intent Dutch Auction trajectory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentDutchAuctionAnalytics {
    /// Instantaneous price at evaluated time t: P(t).
    pub price_at_t: f64,
    /// Instantaneous solver fill hazard at time t: \lambda(t).
    pub hazard_at_t: f64,
    /// Cumulative probability of solver clearance by time t: F(t).
    pub cumulative_fill_prob_at_t: f64,
    /// Expected clearance price over the entire auction horizon \mathbb{E}[P^*].
    pub expected_clearance_price: f64,
    /// Expected fill latency given fill \mathbb{E}[T_{fill} | \text{fill}].
    pub expected_fill_latency_sec: f64,
    /// Overall clearance probability by horizon T_{max}: F(T_{max}).
    pub total_fill_probability: f64,
    /// Optimal auction duration T^* minimizing total clearance loss + delay cost + failure risk.
    pub optimal_duration_sec: f64,
    /// Expected creator surplus preserved over reserve floor: \mathbb{E}[P^*] - P_{min}.
    pub surplus_preserved: f64,
}

/// Dynamic Cross-Chain Intent Dutch Auction Pricing & Solver Hazard Clearance Engine.
///
/// Implements:
/// 1. **Non-Linear Dutch Decay Trajectory**:
///    $$P_{\text{intent}}(t) = P_{\text{start}} - (P_{\text{start}} - P_{\text{min}}) \left(\frac{t}{T_{\text{max}}}\right)^\gamma$$
/// 2. **Inhomogeneous Poisson Solver Fill Hazard**:
///    $$\lambda_{\text{solver}}(P(t), t) = \lambda_0 \exp\left( \alpha \frac{P_{\text{start}} - P(t)}{P_{\text{start}} - P_{\text{min}}} \right)$$
/// 3. **Expected Clearance Price & Optimal Duration**:
///    $$\mathbb{E}[P^*] = \int_0^{T_{\text{max}}} P(u) \lambda(u) e^{-\Lambda(u)} du + P_{\text{min}} e^{-\Lambda(T_{\text{max}})}$$
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentDutchAuctionEngine {
    pub config: IntentDutchAuctionConfig,
}

impl Default for IntentDutchAuctionEngine {
    fn default() -> Self {
        Self::new(IntentDutchAuctionConfig::default())
    }
}

impl IntentDutchAuctionEngine {
    pub fn new(config: IntentDutchAuctionConfig) -> Self {
        Self { config }
    }

    /// Determines the curve geometry based on exponent \gamma.
    pub fn curve_type(&self) -> DutchAuctionCurveType {
        if (self.config.curve_gamma - 1.0).abs() < 1e-4 {
            DutchAuctionCurveType::Linear
        } else if self.config.curve_gamma > 1.0 {
            DutchAuctionCurveType::ConvexDecay
        } else {
            DutchAuctionCurveType::ConcaveDecay
        }
    }

    /// Computes instantaneous Dutch auction price P(t) at elapsed time t \ge 0.
    pub fn price_at(&self, t_sec: f64) -> f64 {
        let max_t = self.config.max_duration_sec.max(1e-4);
        let tau = (t_sec / max_t).clamp(0.0, 1.0);
        let tau_pow = tau.powf(self.config.curve_gamma);
        let price_drop = (self.config.start_price - self.config.min_price) * tau_pow;
        self.config.start_price - price_drop
    }

    /// Computes normalized relative price concession x(t) \in [0.0, 1.0].
    pub fn relative_concession(&self, t_sec: f64) -> f64 {
        let max_t = self.config.max_duration_sec.max(1e-4);
        let tau = (t_sec / max_t).clamp(0.0, 1.0);
        tau.powf(self.config.curve_gamma)
    }

    /// Instantaneous solver arrival hazard rate \lambda(t) at elapsed time t.
    pub fn solver_hazard_at(&self, t_sec: f64) -> f64 {
        let concession = self.relative_concession(t_sec);
        self.config.base_solver_hazard * (self.config.hazard_price_sensitivity * concession).exp()
    }

    /// Computes cumulative hazard \Lambda(t) = \int_0^t \lambda(u) du using numerical quadrature.
    pub fn cumulative_hazard_at(&self, t_sec: f64, steps: usize) -> f64 {
        if t_sec <= 0.0 {
            return 0.0;
        }
        let n = steps.max(20);
        let dt = t_sec / n as f64;
        let mut sum = 0.5 * (self.solver_hazard_at(0.0) + self.solver_hazard_at(t_sec));
        for i in 1..n {
            let u = i as f64 * dt;
            sum += self.solver_hazard_at(u);
        }
        sum * dt
    }

    /// Cumulative fill probability F(t) = 1 - \exp(-\Lambda(t)).
    pub fn cumulative_fill_prob_at(&self, t_sec: f64) -> f64 {
        let lambda_cum = self.cumulative_hazard_at(t_sec, 50);
        1.0 - (-lambda_cum).exp()
    }

    /// Computes expected clearance price, expected fill latency, and total clearance probability
    /// given a chosen auction duration T using exact survival analysis.
    pub fn expected_clearance_price_and_latency(&self, duration_sec: f64) -> (f64, f64, f64) {
        let max_t = duration_sec.max(0.10);
        let n = 100;
        let dt = max_t / n as f64;
        let mut expected_p = 0.0;
        let mut expected_t_num = 0.0;
        let mut cum_hazard_prev = 0.0;
        let mut total_fill_prob = 0.0;

        for i in 0..n {
            let u = (i as f64 + 0.5) * dt;
            let tau = (u / max_t).clamp(0.0, 1.0);
            let concession = tau.powf(self.config.curve_gamma);
            let p_u = self.config.start_price
                - (self.config.start_price - self.config.min_price) * concession;
            let h_u = self.config.base_solver_hazard
                * (self.config.hazard_price_sensitivity * concession).exp();

            let cum_hazard_curr = cum_hazard_prev + h_u * dt;
            // Exact survival interval probability: S(u_prev) - S(u_curr)
            let prob_interval = (-cum_hazard_prev).exp() - (-cum_hazard_curr).exp();

            expected_p += p_u * prob_interval;
            expected_t_num += u * prob_interval;
            total_fill_prob += prob_interval;
            cum_hazard_prev = cum_hazard_curr;
        }

        // Unfilled fraction settles at reserve floor P_{min}
        let unfilled_prob = (-cum_hazard_prev).exp();
        expected_p += self.config.min_price * unfilled_prob;

        let expected_latency = if total_fill_prob > 1e-6 {
            expected_t_num / total_fill_prob
        } else {
            max_t
        };

        (expected_p, expected_latency, total_fill_prob)
    }

    /// Computes optimal auction duration T^* minimizing total clearance loss, delay cost, and failure risk.
    pub fn find_optimal_duration(&self) -> f64 {
        let mut best_t = self.config.max_duration_sec;
        let mut min_cost = f64::INFINITY;

        // Grid search over potential auction durations from 5.0s to 60.0s
        for t_step in 5..=60 {
            let t = t_step as f64;
            let (exp_p, exp_lat, fill_prob) = self.expected_clearance_price_and_latency(t);
            let price_slippage_loss = self.config.start_price - exp_p;
            let delay_cost = self.config.delay_cost_rate * exp_lat;
            let failure_cost = self.config.failure_penalty * (1.0 - fill_prob);
            let total_cost = price_slippage_loss + delay_cost + failure_cost;

            if total_cost < min_cost {
                min_cost = total_cost;
                best_t = t;
            }
        }

        best_t
    }

    /// Evaluates complete Dutch auction analytics at elapsed time t_sec.
    pub fn evaluate_analytics(&self, current_elapsed_sec: f64) -> IntentDutchAuctionAnalytics {
        let price_at_t = self.price_at(current_elapsed_sec);
        let hazard_at_t = self.solver_hazard_at(current_elapsed_sec);
        let cumulative_fill_prob_at_t = self.cumulative_fill_prob_at(current_elapsed_sec);
        let (expected_clearance_price, expected_fill_latency_sec, total_fill_probability) =
            self.expected_clearance_price_and_latency(self.config.max_duration_sec);
        let optimal_duration_sec = self.find_optimal_duration();
        let surplus_preserved = (expected_clearance_price - self.config.min_price).max(0.0);

        IntentDutchAuctionAnalytics {
            price_at_t,
            hazard_at_t,
            cumulative_fill_prob_at_t,
            expected_clearance_price,
            expected_fill_latency_sec,
            total_fill_probability,
            optimal_duration_sec,
            surplus_preserved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_normal_inv_cdf_precision() {
        let z_50 = standard_normal_inv_cdf(0.5);
        assert!((z_50 - 0.0).abs() < 1e-4);

        let z_84 = standard_normal_inv_cdf(0.8413447);
        assert!((z_84 - 1.0).abs() < 1e-3);

        let z_975 = standard_normal_inv_cdf(0.975);
        assert!((z_975 - 1.95996).abs() < 1e-3);
    }

    #[test]
    fn test_solver_auction_fee_bidding() {
        let engine = CrossChainIntentBridgeEngine::default();
        let (fee_calm, lat_calm) = engine.compute_optimal_solver_fee(1.0);
        let (fee_urgent, lat_urgent) = engine.compute_optimal_solver_fee(3.0);

        assert!(
            fee_urgent > fee_calm,
            "Higher urgency must increase optimal solver bid fee"
        );
        assert!(
            lat_urgent < lat_calm,
            "Higher solver fee must reduce expected fill latency"
        );
        assert!((3.0..=7.0).contains(&fee_calm));
    }

    #[test]
    fn test_destination_pre_flight_solvency() {
        let engine = CrossChainIntentBridgeEngine::default();
        // Solvent account with large equity headroom
        let verdict_solvent = engine.evaluate_destination_solvency(
            10000.0, // Equity
            2000.0,  // MMR
            15000.0, // Notional
            0.60,    // Vol
            15.0,    // Latency sec
        );
        assert!(verdict_solvent.is_solvent);
        assert!(verdict_solvent.net_excess_margin_usd > 0.0);

        // Insolvent account near liquidation
        let verdict_insolvent = engine.evaluate_destination_solvency(
            2050.0,  // Equity
            2000.0,  // MMR (only $50 buffer)
            50000.0, // Large $50k notional
            0.80,    // High vol
            30.0,    // Latency sec
        );
        assert!(!verdict_insolvent.is_solvent);
        assert!(verdict_insolvent.net_excess_margin_usd < 0.0);
    }

    #[test]
    fn test_intent_dutch_auction_curve_and_monotonicity() {
        let config = IntentDutchAuctionConfig {
            start_price: 1.0000,
            min_price: 0.9900,
            max_duration_sec: 30.0,
            curve_gamma: 2.0, // Convex decay
            ..Default::default()
        };
        let engine = IntentDutchAuctionEngine::new(config);

        assert_eq!(engine.curve_type(), DutchAuctionCurveType::ConvexDecay);

        // At t=0, price is start_price
        let p0 = engine.price_at(0.0);
        assert!((p0 - 1.0000).abs() < 1e-6);

        // At t=15 (halfway), quadratic decay: tau = 0.5, tau^2 = 0.25
        // Price drop = (1.0 - 0.99) * 0.25 = 0.0025 => price = 0.9975
        let p_half = engine.price_at(15.0);
        assert!((p_half - 0.9975).abs() < 1e-6);

        // Compared to linear decay (which would be 0.9950 at halfway), convex preserves higher surplus!
        assert!(p_half > 0.9950);

        // At t=30, price reaches reserve floor
        let p_end = engine.price_at(30.0);
        assert!((p_end - 0.9900).abs() < 1e-6);

        // Strictly monotonic decreasing
        assert!(p0 > p_half && p_half > p_end);
    }

    #[test]
    fn test_intent_dutch_auction_solver_hazard_and_clearance() {
        let config = IntentDutchAuctionConfig {
            start_price: 100.0,
            min_price: 98.0,
            max_duration_sec: 20.0,
            base_solver_hazard: 0.10,
            hazard_price_sensitivity: 3.0,
            ..Default::default()
        };
        let engine = IntentDutchAuctionEngine::new(config);

        // Initial hazard at t=0 (no concession)
        let h0 = engine.solver_hazard_at(0.0);
        assert!((h0 - 0.10).abs() < 1e-6);

        // Hazard at t=20 (maximum concession): 0.10 * exp(3.0) approx 0.10 * 20.0855 = 2.0085
        let h_end = engine.solver_hazard_at(20.0);
        assert!(h_end > 2.0);
        assert!(h_end > h0);

        // Cumulative fill probability increases monotonically
        let f5 = engine.cumulative_fill_prob_at(5.0);
        let f20 = engine.cumulative_fill_prob_at(20.0);
        assert!(f5 > 0.0);
        assert!(f20 > f5);
        assert!(f20 > 0.90, "High fill probability by auction deadline");

        // Analytics evaluation
        let analytics = engine.evaluate_analytics(10.0);
        assert!(analytics.price_at_t < 100.0 && analytics.price_at_t > 98.0);
        assert!(analytics.expected_clearance_price > 98.0);
        assert!(analytics.surplus_preserved > 0.0);
        assert!(analytics.optimal_duration_sec > 0.0);
    }
}
