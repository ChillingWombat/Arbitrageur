//! High-Frequency Continuous Hidden Markov Model (HMM) Liquidity & Volatility Regime Filter.
//!
//! Implements an online recursive forward-filtering algorithm over 4 microstructural observables:
//! 1. Log relative spread width: $\ln(S_t / \text{TickSize})$
//! 2. Order Flow Imbalance (OFI) rolling volatility: $\sigma_{\text{OFI}, t}$
//! 3. Trade arrival intensity: $\lambda_{\text{trade}, t}$ (Hz)
//! 4. Depth cancellation-to-trade ratio: $R_{\text{cancel}, t} = \frac{v_{\text{cancel}, t}}{v_{\text{trade}, t} + \epsilon}$
//!
//! Filters 3 discrete hidden microstructure states:
//! - State 0: `MeanReverting` (Calm, tight spread, balanced flow, high queue fill rates)
//! - State 1: `DirectionalTrending` (Momentum, toxic directional sweep, elevated OFI volatility)
//! - State 2: `LiquidityShock` (Spread blowout, depth vacuum, cancel surge, cascading liquidations)
//!
//! Outputs dynamic execution urgency protocols and execution horizon scaling factors $\phi_{\text{urgency}}$.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Discrete hidden microstructural market regime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HmmRegimeState {
    /// Calm, stationary, mean-reverting book. High queue stability.
    MeanReverting = 0,
    /// Directional momentum and toxic flow sweeps. Moderate-to-high adverse selection.
    DirectionalTrending = 1,
    /// Extreme liquidity shock, spread blowout, order book vacuum.
    LiquidityShock = 2,
}

/// Execution urgency protocol driving smart order routing and order placement posture.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionUrgencyProtocol {
    /// Protocol A: Passive inventory quoting with Avellaneda-Stoikov skew.
    /// Used when State 0 posterior $\ge 0.60$.
    ProtocolA_PassiveQuoting,
    /// Protocol B: Aggressive taker sweep IOC. Cancel resting maker quotes on vulnerable side.
    /// Used when State 1 posterior $\ge 0.55$.
    ProtocolB_AggressiveTakerIOC,
    /// Protocol C: Protective circuit hold. Instant cancel-all resting orders; widen hurdles $5\times$.
    /// Used when State 2 posterior $\ge 0.35$.
    ProtocolC_CircuitHold,
}

/// Continuous Hidden Markov Model regime filter for high-frequency order books.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousHmmRegimeFilter {
    /// State transition probability matrix $A_{ij} = P(S_t = j \mid S_{t-1} = i)$.
    pub transition_matrix: [[f64; 3]; 3],
    /// Mean vectors for each state across 4 observables.
    pub means: [[f64; 4]; 3],
    /// Diagonal variances for each state across 4 observables.
    pub variances: [[f64; 4]; 3],
    /// Filtered posterior state probabilities $[\pi_{0, t}, \pi_{1, t}, \pi_{2, t}]$.
    pub posterior_probs: [f64; 3],
    /// Prior state probabilities before measurement update.
    pub prior_probs: [f64; 3],
    /// Dynamic execution urgency multiplier $\phi_{\text{urgency}} \in [0.2, 5.0]$.
    pub urgency_multiplier: f64,
    /// Minimum probability floor to prevent numerical zero absorption.
    pub prob_floor: f64,
}

impl Default for ContinuousHmmRegimeFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ContinuousHmmRegimeFilter {
    /// Initializes an HMM regime filter with calibrated crypto order book priors.
    pub fn new() -> Self {
        Self {
            // Highly persistent microstructure regimes (diagonal dominance)
            transition_matrix: [
                [0.96, 0.03, 0.01], // State 0 (Mean-Reverting) -> persistent
                [0.05, 0.92, 0.03], // State 1 (Trending) -> bursts of momentum
                [0.08, 0.04, 0.88], // State 2 (Liquidity Shock) -> transient cascades
            ],
            // Observable features: [ln(spread/tick), sigma_OFI, lambda_trade_Hz, R_cancel]
            means: [
                [0.5, 1.0, 5.0, 1.2], // S0: Tight spread, low OFI vol, steady trades, low cancels
                [1.2, 4.0, 25.0, 2.5], // S1: Elevated spread, strong OFI vol, high trade velocity
                [2.8, 6.5, 2.0, 14.0], // S2: Blown out spread, extreme vol, low prints, massive cancel vacuum
            ],
            variances: [
                [0.2, 0.5, 4.0, 0.6],
                [0.4, 1.5, 16.0, 1.5],
                [0.8, 2.5, 2.0, 8.0],
            ],
            posterior_probs: [0.85, 0.10, 0.05], // Initialized in calm state
            prior_probs: [0.85, 0.10, 0.05],
            urgency_multiplier: 0.5,
            prob_floor: 1e-5,
        }
    }

    /// Evaluates the 4D Gaussian emission density $B_k(\mathbf{y}) = P(\mathbf{y} \mid S = k)$.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn gaussian_density(&self, state: usize, y: &[f64; 4]) -> f64 {
        let mut mahalanobis = 0.0;
        let mut det = 1.0;

        for f in 0..4 {
            let diff = y[f] - self.means[state][f];
            let var = self.variances[state][f].max(1e-6);
            det *= var;
            mahalanobis += (diff * diff) / var;
        }

        let norm_const = (2.0 * PI).powi(2) * det.sqrt();
        let density = (-0.5 * mahalanobis).exp() / norm_const.max(1e-12);
        if density.is_finite() && density > 1e-30 {
            density
        } else {
            1e-30
        }
    }

    /// Online recursive forward-filtering step per tick.
    /// Updates posterior state probabilities and returns the assigned execution urgency protocol.
    #[allow(clippy::needless_range_loop)]
    pub fn update(&mut self, observables: &[f64; 4]) -> ExecutionUrgencyProtocol {
        // 1. Prediction step: \pi_{j, t|t-1} = \sum_{i=0}^2 \pi_{i, t-1} A_{ij}
        let mut predicted_prior = [0.0; 3];
        for j in 0..3 {
            for i in 0..3 {
                predicted_prior[j] += self.posterior_probs[i] * self.transition_matrix[i][j];
            }
        }
        self.prior_probs = predicted_prior;

        // 2. Bayesian update: \alpha_j(t) = \pi_{j, t|t-1} \cdot B_j(\mathbf{y}_t)
        let mut unnorm_posterior = [0.0; 3];
        let mut sum_density = 0.0;
        for j in 0..3 {
            let density = self.gaussian_density(j, observables);
            unnorm_posterior[j] = predicted_prior[j] * density;
            sum_density += unnorm_posterior[j];
        }

        // 3. Normalization with underflow protection and probability floor
        if sum_density > 1e-35 && sum_density.is_finite() {
            let mut sum_filtered = 0.0;
            for j in 0..3 {
                self.posterior_probs[j] = (unnorm_posterior[j] / sum_density).max(self.prob_floor);
                sum_filtered += self.posterior_probs[j];
            }
            for j in 0..3 {
                self.posterior_probs[j] /= sum_filtered;
            }
        } else {
            // Degenerate measurement fallback to prior
            self.posterior_probs = self.prior_probs;
        }

        // 4. Compute dynamic execution urgency multiplier:
        // S0: 0.3x (patient), S1: 1.2x (moderately aggressive), S2: 3.5x (immediate emergency)
        self.urgency_multiplier = (self.posterior_probs[0] * 0.30
            + self.posterior_probs[1] * 1.30
            + self.posterior_probs[2] * 3.50)
            .clamp(0.20, 5.0);

        // 5. Evaluate execution urgency protocol
        if self.posterior_probs[2] >= 0.35 {
            ExecutionUrgencyProtocol::ProtocolC_CircuitHold
        } else if self.posterior_probs[1] >= 0.55 {
            ExecutionUrgencyProtocol::ProtocolB_AggressiveTakerIOC
        } else {
            ExecutionUrgencyProtocol::ProtocolA_PassiveQuoting
        }
    }

    /// Returns the currently dominant regime state.
    pub fn dominant_state(&self) -> HmmRegimeState {
        if self.posterior_probs[2] >= self.posterior_probs[1]
            && self.posterior_probs[2] >= self.posterior_probs[0]
        {
            HmmRegimeState::LiquidityShock
        } else if self.posterior_probs[1] >= self.posterior_probs[0] {
            HmmRegimeState::DirectionalTrending
        } else {
            HmmRegimeState::MeanReverting
        }
    }

    /// Calibrates custom transition persistence and feature parameters.
    pub fn with_params(
        mut self,
        transition_matrix: [[f64; 3]; 3],
        means: [[f64; 4]; 3],
        variances: [[f64; 4]; 3],
    ) -> Self {
        self.transition_matrix = transition_matrix;
        self.means = means;
        self.variances = variances;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calm_ticks_converge_to_mean_reverting_passive_quoting() {
        let mut filter = ContinuousHmmRegimeFilter::new();

        // Feed 15 calm ticks: tight spread (0.5), low OFI vol (0.8), steady trades (5.0), low cancel (1.0)
        let calm_tick = [0.5, 0.8, 5.0, 1.0];
        let mut last_protocol = ExecutionUrgencyProtocol::ProtocolC_CircuitHold;

        for _ in 0..15 {
            last_protocol = filter.update(&calm_tick);
        }

        assert_eq!(
            last_protocol,
            ExecutionUrgencyProtocol::ProtocolA_PassiveQuoting
        );
        assert_eq!(filter.dominant_state(), HmmRegimeState::MeanReverting);
        assert!(filter.posterior_probs[0] > 0.85);
        assert!(filter.urgency_multiplier < 0.60);
    }

    #[test]
    fn test_momentum_sweep_triggers_aggressive_taker_ioc() {
        let mut filter = ContinuousHmmRegimeFilter::new();

        // Start calm
        let calm_tick = [0.5, 1.0, 5.0, 1.2];
        for _ in 0..5 {
            filter.update(&calm_tick);
        }

        // Sudden toxic directional surge: elevated spread (1.3), high OFI vol (4.8), 30 Hz trades, cancel 2.5
        let momentum_tick = [1.3, 4.8, 30.0, 2.5];
        let mut protocol = ExecutionUrgencyProtocol::ProtocolA_PassiveQuoting;

        for _ in 0..10 {
            protocol = filter.update(&momentum_tick);
        }

        assert_eq!(
            protocol,
            ExecutionUrgencyProtocol::ProtocolB_AggressiveTakerIOC
        );
        assert_eq!(filter.dominant_state(), HmmRegimeState::DirectionalTrending);
        assert!(filter.posterior_probs[1] > 0.65);
        assert!(filter.urgency_multiplier >= 1.0);
    }

    #[test]
    fn test_liquidity_shock_triggers_circuit_hold_and_high_urgency() {
        let mut filter = ContinuousHmmRegimeFilter::new();

        // Massive liquidation cascade: spread blowout (3.2), high OFI vol (7.0), prints freeze (1.0 Hz), cancel ratio 16.0
        let cascade_tick = [3.2, 7.0, 1.0, 16.0];
        let mut protocol = ExecutionUrgencyProtocol::ProtocolA_PassiveQuoting;

        for _ in 0..8 {
            protocol = filter.update(&cascade_tick);
        }

        assert_eq!(protocol, ExecutionUrgencyProtocol::ProtocolC_CircuitHold);
        assert_eq!(filter.dominant_state(), HmmRegimeState::LiquidityShock);
        assert!(filter.posterior_probs[2] > 0.50);
        assert!(filter.urgency_multiplier > 2.0);
    }

    #[test]
    fn test_numerical_stability_under_extreme_inputs() {
        let mut filter = ContinuousHmmRegimeFilter::new();

        // Extreme off-scale observables (huge values, negative values, near-zero)
        let extreme_ticks = [
            [100.0, 1000.0, 0.0, 1000.0],
            [-10.0, 0.00001, 10000.0, -5.0],
            [f64::MAX / 1e10, 0.0, 0.0, 0.0],
        ];

        for tick in &extreme_ticks {
            let protocol = filter.update(tick);
            assert!(
                protocol == ExecutionUrgencyProtocol::ProtocolA_PassiveQuoting
                    || protocol == ExecutionUrgencyProtocol::ProtocolB_AggressiveTakerIOC
                    || protocol == ExecutionUrgencyProtocol::ProtocolC_CircuitHold
            );
            for p in &filter.posterior_probs {
                assert!(p.is_finite());
                assert!(*p >= 0.0 && *p <= 1.0);
            }
            let sum: f64 = filter.posterior_probs.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4);
        }
    }
}
