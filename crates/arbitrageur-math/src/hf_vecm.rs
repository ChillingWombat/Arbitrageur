//! High-Frequency Vector Error Correction Model (VECM) with Hawkes-Modulated Adjustment Speed.
//!
//! Models cross-venue log perpetual prices $Y_t = [\ln P^{(1)}_t, \dots, \ln P^{(N)}_t]^T$ via
//! $\Delta Y_t = \boldsymbol{\alpha} \boldsymbol{\beta}^T Y_{t-1} + \sum \mathbf{\Gamma}_i \Delta Y_{t-i} + \boldsymbol{\epsilon}_t$,
//! where speed of adjustment $\boldsymbol{\alpha}(t)$ is dynamically accelerated by self- and mutually-exciting
//! Hawkes order arrival intensities during market-making bursts, and gated when cascading liquidations occur.

use serde::{Deserialize, Serialize};

/// High-Frequency VECM Execution Regime Decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VecmRegimeDecision {
    /// Fast-reverting elastic entry regime: half-life is ultra-short, capture basis convergence.
    FastRevertingElasticEntry {
        basis_spread: f64,
        effective_half_life_ms: f64,
        mean_alpha: f64,
        recommended_direction: &'static str,
    },
    /// Normal continuous reversion regime: trade standard mean-reverting basis.
    NormalReversion {
        basis_spread: f64,
        effective_half_life_ms: f64,
    },
    /// Supercritical Hawkes cascade circuit halt: liquidation cascade breaking cointegration.
    CircuitHaltSupercriticalBurst {
        basis_spread: f64,
        spectral_radius: f64,
        reason: &'static str,
    },
    /// Equilibrium regime: spread within deadband around zero.
    Equilibrium { basis_spread: f64 },
}

/// High-Frequency VECM Engine with Hawkes-Modulated Adjustment Speed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfVecmHawkesEngine {
    /// Base adjustment speed vector $\boldsymbol{\alpha}_0$ per venue
    pub base_adjustment_speed_alpha: Vec<f64>,
    /// Cointegrating vector $\boldsymbol{\beta}$ (e.g. [+1.0, -1.0] for pair or normalized weights)
    pub cointegrating_vector_beta: Vec<f64>,
    /// Elasticity sensitivity multiplier $\kappa_{\text{elastic}} \ge 0$
    pub elasticity_sensitivity_kappa: f64,
    /// Baseline Hawkes intensity $\bar{\lambda} > 0$
    pub baseline_hawkes_intensity: f64,
    /// Hawkes spectral radius threshold $\rho^* \approx 0.92$ triggering cascade circuit halt
    pub supercritical_circuit_threshold: f64,
    /// Minimum basis spread hurdle to trigger entry (e.g. 3.0 bps)
    pub min_basis_hurdle_bps: f64,
}

impl Default for HfVecmHawkesEngine {
    fn default() -> Self {
        Self {
            base_adjustment_speed_alpha: vec![16.5, 16.5],
            cointegrating_vector_beta: vec![1.0, -1.0],
            elasticity_sensitivity_kappa: 1.8,
            baseline_hawkes_intensity: 5.0,
            supercritical_circuit_threshold: 0.92,
            min_basis_hurdle_bps: 2.5,
        }
    }
}

impl HfVecmHawkesEngine {
    pub fn new(
        base_alpha: Vec<f64>,
        beta: Vec<f64>,
        elasticity_kappa: f64,
        base_hawkes: f64,
    ) -> Self {
        Self {
            base_adjustment_speed_alpha: base_alpha,
            cointegrating_vector_beta: beta,
            elasticity_sensitivity_kappa: elasticity_kappa.max(0.1),
            baseline_hawkes_intensity: base_hawkes.max(0.1),
            supercritical_circuit_threshold: 0.92,
            min_basis_hurdle_bps: 2.5,
        }
    }

    /// Evaluates cointegration basis spread: $Z_t = \boldsymbol{\beta}^T Y_t$.
    pub fn compute_basis_spread(&self, log_prices: &[f64]) -> f64 {
        log_prices
            .iter()
            .zip(self.cointegrating_vector_beta.iter())
            .map(|(p, b)| p * b)
            .sum()
    }

    /// Modulates adjustment speed $\boldsymbol{\alpha}(t)$ based on real-time Hawkes intensity:
    ///
    /// $$\boldsymbol{\alpha}(t) = \boldsymbol{\alpha}_0 \odot \left( \mathbf{1} + \boldsymbol{\kappa}_{\text{elastic}} \frac{\boldsymbol{\lambda}_{\text{Hawkes}}(t) - \bar{\boldsymbol{\lambda}}}{\bar{\boldsymbol{\lambda}}} \right)$$
    pub fn modulated_alpha(&self, hawkes_intensity: f64) -> Vec<f64> {
        let intensity_ratio = (hawkes_intensity - self.baseline_hawkes_intensity)
            / self.baseline_hawkes_intensity.max(1e-6);
        let multiplier = (1.0 + self.elasticity_sensitivity_kappa * intensity_ratio).max(0.1);

        self.base_adjustment_speed_alpha
            .iter()
            .map(|&a0| a0 * multiplier)
            .collect()
    }

    /// Computes empirical half-life of basis mean-reversion in milliseconds:
    ///
    /// $$\tau_{1/2} = \frac{\ln 2}{\bar{\alpha}(t)} \cdot 1000\text{ ms}$$
    pub fn compute_half_life_ms(&self, hawkes_intensity: f64) -> f64 {
        let alpha_eff = self.modulated_alpha(hawkes_intensity);
        let mean_alpha: f64 = alpha_eff.iter().sum::<f64>() / alpha_eff.len().max(1) as f64;
        (2.0f64.ln() / mean_alpha.max(1e-6)) * 1000.0
    }

    /// Evaluates whether current basis dislocation represents an ultra-fast elastic mean-reverting entry,
    /// a normal reversion, an equilibrium state, or a toxic supercritical dislocation cascade.
    pub fn evaluate_execution_regime(
        &self,
        log_prices: &[f64],
        hawkes_intensity: f64,
        hawkes_spectral_radius: f64,
    ) -> VecmRegimeDecision {
        let spread = self.compute_basis_spread(log_prices);

        // 1. Supercritical Hawkes circuit check
        if hawkes_spectral_radius >= self.supercritical_circuit_threshold {
            return VecmRegimeDecision::CircuitHaltSupercriticalBurst {
                basis_spread: spread,
                spectral_radius: hawkes_spectral_radius,
                reason: "Supercritical Hawkes cascade: self-exciting liquidation spiral breaking cointegration",
            };
        }

        let alpha_eff = self.modulated_alpha(hawkes_intensity);
        let mean_alpha: f64 = alpha_eff.iter().sum::<f64>() / alpha_eff.len().max(1) as f64;
        let half_life_ms = (2.0f64.ln() / mean_alpha.max(1e-6)) * 1000.0;

        let spread_bps = spread.abs() * 10_000.0;

        if spread_bps < self.min_basis_hurdle_bps {
            return VecmRegimeDecision::Equilibrium {
                basis_spread: spread,
            };
        }

        let direction = if spread > 0.0 {
            "ShortVenueA_LongVenueB" // Venue A overpriced relative to Venue B
        } else {
            "LongVenueA_ShortVenueB" // Venue A underpriced relative to Venue B
        };

        if half_life_ms < 25.0 {
            VecmRegimeDecision::FastRevertingElasticEntry {
                basis_spread: spread,
                effective_half_life_ms: half_life_ms,
                mean_alpha,
                recommended_direction: direction,
            }
        } else {
            VecmRegimeDecision::NormalReversion {
                basis_spread: spread,
                effective_half_life_ms: half_life_ms,
            }
        }
    }
}

/// Execution action for multi-asset VECM residual sniper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VecmSignalAction {
    EnterLongSpread,
    EnterShortSpread,
    ExitEquilibrium,
    HoldNeutral,
}

/// High-Frequency Multi-Asset Cointegrated VECM Residual Sniper.
///
/// Implements:
/// 1. Stationary error-correction residual spread $z_t = \boldsymbol{\beta}^T Y_t$ across $K \ge 2$ assets.
/// 2. Ornstein-Uhlenbeck continuous half-life $\tau_{1/2} = \frac{\ln 2}{\kappa}$.
/// 3. Leung-Li (2015) double optimal stopping boundaries:
///    $$d^* = \theta - \sqrt{\frac{\sigma_z^2}{2\kappa}} \left( \sqrt{2 \ln\left(\frac{1}{r c_{\text{entry}}}\right)} - \frac{c_{\text{entry}} \sqrt{2\kappa}}{\sigma_z} \right)$$
///    $$b^* = \theta + \sqrt{\frac{\sigma_z^2}{2\kappa}} \left( \sqrt{2 \ln\left(\frac{1}{r c_{\text{exit}}}\right)} - \frac{c_{\text{exit}} \sqrt{2\kappa}}{\sigma_z} \right)$$
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfMultiAssetVecmResidualSniper {
    pub cointegrating_vector_beta: Vec<f64>,
    pub mean_reversion_kappa: f64,
    pub equilibrium_theta: f64,
    pub spread_volatility_sigma: f64,
    pub discount_rate_r: f64,
    pub crossing_fee: f64,
}

impl Default for HfMultiAssetVecmResidualSniper {
    fn default() -> Self {
        Self {
            cointegrating_vector_beta: vec![1.0, -1.0],
            mean_reversion_kappa: 8.5,
            equilibrium_theta: 0.0,
            spread_volatility_sigma: 0.0015,
            discount_rate_r: 0.05,
            crossing_fee: 0.0004, // 4 bps round-trip fee
        }
    }
}

impl HfMultiAssetVecmResidualSniper {
    pub fn new(beta: Vec<f64>, kappa: f64, theta: f64, sigma: f64, crossing_fee: f64) -> Self {
        Self {
            cointegrating_vector_beta: beta,
            mean_reversion_kappa: kappa.max(1e-4),
            equilibrium_theta: theta,
            spread_volatility_sigma: sigma.max(1e-5),
            discount_rate_r: 0.05,
            crossing_fee: crossing_fee.max(1e-5),
        }
    }

    /// Evaluates current error-correction spread residual: z_t = beta^T Y_t
    pub fn compute_residual(&self, log_prices: &[f64]) -> f64 {
        log_prices
            .iter()
            .zip(self.cointegrating_vector_beta.iter())
            .map(|(p, b)| p * b)
            .sum()
    }

    /// Evaluates continuous OU half-life: tau_1/2 = ln 2 / kappa
    pub fn half_life_sec(&self) -> f64 {
        2.0f64.ln() / self.mean_reversion_kappa
    }

    /// Computes Leung-Li (2015) double optimal stopping entry (d*) and exit (b*) thresholds
    pub fn compute_optimal_thresholds(&self) -> (f64, f64) {
        let kappa = self.mean_reversion_kappa;
        let sigma = self.spread_volatility_sigma;
        let theta = self.equilibrium_theta;
        let c = self.crossing_fee;
        let r = self.discount_rate_r;

        let scale = (sigma * sigma / (2.0 * kappa)).sqrt();
        let log_term = (1.0 / (r * c).max(1e-6)).ln().max(1.0);
        let band_width = scale * ((2.0 * log_term).sqrt() - (c * (2.0 * kappa).sqrt() / sigma));

        let entry_d = theta - band_width.max(0.5 * c);
        let exit_b = theta + band_width.max(0.5 * c);

        (entry_d, exit_b)
    }

    /// Evaluates execution signal given synchronous log prices
    pub fn evaluate_signal(&self, log_prices: &[f64]) -> (VecmSignalAction, f64, (f64, f64)) {
        let z = self.compute_residual(log_prices);
        let (d_star, b_star) = self.compute_optimal_thresholds();

        if z <= d_star {
            (VecmSignalAction::EnterLongSpread, z, (d_star, b_star))
        } else if z >= b_star {
            (VecmSignalAction::EnterShortSpread, z, (d_star, b_star))
        } else if (z - self.equilibrium_theta).abs() < 0.25 * self.crossing_fee {
            (VecmSignalAction::ExitEquilibrium, z, (d_star, b_star))
        } else {
            (VecmSignalAction::HoldNeutral, z, (d_star, b_star))
        }
    }
}

/// Online Granger Causality Lead-Lag Matrix across $K$ instruments.
///
/// Quantifies directional information transfer $G_{ij}$ and computes the Granger Asymmetry Ratio:
/// $$\mathcal{A}_{ij} = \frac{G_{ij} - G_{ji}}{G_{ij} + G_{ji} + \epsilon} \in [-1, 1]$$
///
/// - $\mathcal{A}_{ij} > 0.35$: Asset $j$ Granger-leads Asset $i$ (Asset $j$ is the price leader).
/// - $\mathcal{A}_{ij} < -0.35$: Asset $i$ Granger-leads Asset $j$.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrangerCausalityMatrix {
    pub num_assets: usize,
    /// Pairwise impact matrix G[i][j]: impact of asset j on asset i
    pub impact_matrix: Vec<Vec<f64>>,
    pub lead_threshold: f64,
}

impl GrangerCausalityMatrix {
    pub fn new(num_assets: usize, lead_threshold: f64) -> Self {
        Self {
            num_assets,
            impact_matrix: vec![vec![0.0; num_assets]; num_assets],
            lead_threshold: lead_threshold.clamp(0.1, 0.9),
        }
    }

    /// Updates pairwise impact from short-run VECM gamma coefficient matrices
    pub fn update_from_gammas(&mut self, short_run_gammas: &[Vec<Vec<f64>>]) {
        let n = self.num_assets;
        for i in 0..n {
            for j in 0..n {
                let mut sum_impact = 0.0;
                for gamma in short_run_gammas {
                    if i < gamma.len() && j < gamma[i].len() {
                        sum_impact += gamma[i][j].abs();
                    }
                }
                self.impact_matrix[i][j] = sum_impact;
            }
        }
    }

    /// Computes asymmetry ratio A_ij = (G_ij - G_ji) / (G_ij + G_ji + eps)
    /// Positive A_ij indicates asset j leads asset i.
    pub fn asymmetry_ratio(&self, i: usize, j: usize) -> f64 {
        if i >= self.num_assets || j >= self.num_assets {
            return 0.0;
        }
        let g_ij = self.impact_matrix[i][j];
        let g_ji = self.impact_matrix[j][i];
        let denom = g_ij + g_ji + 1e-8;
        (g_ij - g_ji) / denom
    }

    /// Returns true if asset `leader_idx` Granger-leads asset `follower_idx`
    pub fn is_leader(&self, leader_idx: usize, follower_idx: usize) -> bool {
        self.asymmetry_ratio(follower_idx, leader_idx) >= self.lead_threshold
    }

    /// Returns the global leading asset index (highest net leadership score)
    pub fn global_leader_index(&self) -> usize {
        let mut best_score = f64::NEG_INFINITY;
        let mut best_idx = 0;
        for j in 0..self.num_assets {
            let mut score = 0.0;
            for i in 0..self.num_assets {
                if i != j {
                    score += self.asymmetry_ratio(i, j);
                }
            }
            if score > best_score {
                best_score = score;
                best_idx = j;
            }
        }
        best_idx
    }
}

/// 3-State Markov-Switching Vector Error Correction Model (MS-VECM).
/// Classifies cross-venue basis dynamics into:
/// - State 0: Cointegrated Calm (tight spread, fast half-life)
/// - State 1: Persistent Divergence / Carry Trend
/// - State 2: Structural Breakdown / Cascading Liquidation Dislocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkovSwitchingVecmEngine {
    pub num_venues: usize,
    /// State-dependent cointegrating vectors beta(s) for s in 0..3
    pub beta: Vec<Vec<f64>>,
    /// State-dependent adjustment speeds alpha(s) for s in 0..3
    pub alpha: Vec<Vec<f64>>,
    /// Transition probability matrix P[i][j] = P(S_t = j | S_{t-1} = i)
    pub transition_matrix: [[f64; 3]; 3],
    /// Current filtered state probabilities [P(S_t = 0), P(S_t = 1), P(S_t = 2)]
    pub filtered_state_probs: [f64; 3],
    /// State volatilities sigma(s)
    pub regime_sigmas: [f64; 3],
    /// Historical basis jump z-score threshold (e.g. 3.0)
    pub jump_z_threshold: f64,
}

impl Default for MarkovSwitchingVecmEngine {
    fn default() -> Self {
        Self {
            num_venues: 2,
            beta: vec![
                vec![1.0, -1.0], // Calm: balanced pair
                vec![1.0, -1.0], // Trend: balanced pair
                vec![1.0, -1.0], // Breakdown: divergent
            ],
            alpha: vec![
                vec![18.5, 18.5], // Fast adjustment (Calm)
                vec![4.2, 4.2],   // Slow adjustment (Trend)
                vec![0.0, 0.0],   // Zero adjustment / unanchored (Breakdown)
            ],
            transition_matrix: [
                [0.96, 0.035, 0.005], // Calm -> mostly Calm
                [0.05, 0.93, 0.02],   // Trend -> mostly Trend
                [0.15, 0.10, 0.75],   // Breakdown -> sticky until calmed
            ],
            filtered_state_probs: [0.85, 0.12, 0.03],
            regime_sigmas: [0.0005, 0.0015, 0.0050],
            jump_z_threshold: 3.0,
        }
    }
}

impl MarkovSwitchingVecmEngine {
    pub fn new(
        num_venues: usize,
        beta: Vec<Vec<f64>>,
        alpha: Vec<Vec<f64>>,
        transition_matrix: [[f64; 3]; 3],
        initial_probs: [f64; 3],
        regime_sigmas: [f64; 3],
        jump_z_threshold: f64,
    ) -> Self {
        Self {
            num_venues,
            beta,
            alpha,
            transition_matrix,
            filtered_state_probs: initial_probs,
            regime_sigmas,
            jump_z_threshold: jump_z_threshold.max(1.0),
        }
    }

    /// Recursively updates state probabilities given current log prices and returns (decision, basis_jump_shock).
    #[allow(clippy::needless_range_loop)]
    pub fn update_log_prices(&mut self, log_prices: &[f64]) -> (VecmRegimeDecision, Option<f64>) {
        // 1. Prediction step: xi_{t|t-1} = P^T * xi_{t-1}
        let mut predicted_probs = [0.0; 3];
        for j in 0..3 {
            for i in 0..3 {
                predicted_probs[j] += self.transition_matrix[i][j] * self.filtered_state_probs[i];
            }
        }

        // 2. Compute state-dependent basis error and Gaussian likelihoods
        let mut densities = [0.0; 3];
        for s in 0..3 {
            let basis = log_prices
                .iter()
                .zip(&self.beta[s])
                .map(|(p, b)| p * b)
                .sum::<f64>();
            let sigma = self.regime_sigmas[s].max(1e-6);
            let z = basis / sigma;
            // Densities scaled by Gaussian pdf
            densities[s] = ((-0.5 * z * z).exp() / sigma).max(1e-12);
        }

        // 3. Bayes update: xi_{t|t} = (xi_{t|t-1} * eta) / sum
        let mut sum_prob = 0.0;
        for s in 0..3 {
            self.filtered_state_probs[s] = predicted_probs[s] * densities[s];
            sum_prob += self.filtered_state_probs[s];
        }
        if sum_prob > 1e-12 {
            for s in 0..3 {
                self.filtered_state_probs[s] /= sum_prob;
            }
        } else {
            self.filtered_state_probs = predicted_probs;
        }

        // 4. Extract continuous basis jump shock relative to calm sigma
        let current_basis = log_prices
            .iter()
            .zip(&self.beta[0])
            .map(|(p, b)| p * b)
            .sum::<f64>();
        let calm_sigma = self.regime_sigmas[0].max(1e-6);
        let basis_z = current_basis.abs() / calm_sigma;

        let jump_shock = if basis_z >= self.jump_z_threshold {
            Some(current_basis)
        } else {
            None
        };

        let decision = if self.filtered_state_probs[2] >= 0.35 {
            VecmRegimeDecision::CircuitHaltSupercriticalBurst {
                basis_spread: current_basis,
                spectral_radius: 0.95,
                reason:
                    "MS-VECM: High probability of structural cointegration breakdown (Regime 2)",
            }
        } else if self.filtered_state_probs[0] >= 0.60 {
            VecmRegimeDecision::FastRevertingElasticEntry {
                basis_spread: current_basis,
                effective_half_life_ms: 350.0,
                mean_alpha: self.alpha[0][0],
                recommended_direction: if current_basis > 0.0 {
                    "ShortVenueA_LongVenueB"
                } else {
                    "LongVenueA_ShortVenueB"
                },
            }
        } else {
            VecmRegimeDecision::NormalReversion {
                basis_spread: current_basis,
                effective_half_life_ms: 2500.0,
            }
        };

        (decision, jump_shock)
    }

    /// Returns the current filtered probabilities [P(Calm), P(Trend), P(Breakdown)].
    pub fn current_regime_probabilities(&self) -> [f64; 3] {
        self.filtered_state_probs
    }

    /// Evaluates if current state is in structural cointegration breakdown.
    pub fn is_structural_breakdown(&self) -> bool {
        self.filtered_state_probs[2] >= 0.35
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vecm_basis_spread_and_alpha_modulation() {
        let engine = HfVecmHawkesEngine::default();
        let log_px = vec![50_010.0f64.ln(), 50_000.0f64.ln()];

        let spread = engine.compute_basis_spread(&log_px);
        assert!(
            spread > 0.0,
            "Venue A > Venue B should yield positive basis"
        );

        // Baseline intensity (5.0) -> modulated alpha equals base alpha (16.5)
        let alpha_base = engine.modulated_alpha(5.0);
        assert!((alpha_base[0] - 16.5).abs() < 1e-6);

        // High intensity burst (20.0) -> Hawkes elasticity increases alpha
        let alpha_burst = engine.modulated_alpha(20.0);
        assert!(alpha_burst[0] > alpha_base[0] * 3.0);

        let hl_burst = engine.compute_half_life_ms(20.0);
        let hl_base = engine.compute_half_life_ms(5.0);
        assert!(
            hl_burst < hl_base,
            "High intensity must shorten reversion half-life"
        );
    }

    #[test]
    fn test_vecm_circuit_halt_on_supercritical_branching() {
        let engine = HfVecmHawkesEngine::default();
        let log_px = vec![50_100.0f64.ln(), 50_000.0f64.ln()];

        // Supercritical branching ratio rho = 0.96 >= 0.92 -> Circuit halt
        let decision = engine.evaluate_execution_regime(&log_px, 10.0, 0.96);
        match decision {
            VecmRegimeDecision::CircuitHaltSupercriticalBurst {
                spectral_radius, ..
            } => {
                assert_eq!(spectral_radius, 0.96);
            }
            _ => panic!("Expected CircuitHaltSupercriticalBurst"),
        }
    }

    #[test]
    fn test_vecm_fast_reverting_elastic_entry() {
        let engine = HfVecmHawkesEngine::default();
        // 20 bps spread between venues
        let log_px = vec![50_100.0f64.ln(), 50_000.0f64.ln()];

        // Intense maker liquidity replenishment (intensity = 40.0), stable branching (rho = 0.40)
        let decision = engine.evaluate_execution_regime(&log_px, 40.0, 0.40);
        match decision {
            VecmRegimeDecision::FastRevertingElasticEntry {
                effective_half_life_ms,
                recommended_direction,
                ..
            } => {
                assert!(effective_half_life_ms < 25.0);
                assert_eq!(recommended_direction, "ShortVenueA_LongVenueB");
            }
            _ => panic!("Expected FastRevertingElasticEntry"),
        }
    }

    #[test]
    fn test_hf_multi_asset_vecm_residual_sniper_and_leung_li() {
        // 3-asset cointegrating vector: BTC, ETH, SOL
        let beta = vec![1.0, -0.65, -0.35];
        let sniper = HfMultiAssetVecmResidualSniper::new(beta, 10.0, 0.0, 0.002, 0.0005);

        assert!(sniper.half_life_sec() < 0.1, "Half-life should be ~0.069s");

        let (d_star, b_star) = sniper.compute_optimal_thresholds();
        assert!(d_star < 0.0, "Entry d* must be negative for long spread");
        assert!(b_star > 0.0, "Exit b* must be positive for short spread");
        assert!(b_star > d_star);

        // Test signal evaluation
        let log_px_equilibrium = vec![10.0, 10.0, 10.0];
        let (sig_eq, z_eq, _) = sniper.evaluate_signal(&log_px_equilibrium);
        assert_eq!(sig_eq, VecmSignalAction::ExitEquilibrium);
        assert_eq!(z_eq, 0.0);

        // Extreme negative dislocation
        let log_px_oversold = vec![9.98, 10.0, 10.0]; // BTC dropped -> residual ~ -0.02
        let (sig_long, z_long, _) = sniper.evaluate_signal(&log_px_oversold);
        assert_eq!(sig_long, VecmSignalAction::EnterLongSpread);
        assert!(z_long < d_star);

        // Extreme positive dislocation
        let log_px_overbought = vec![10.02, 10.0, 10.0]; // BTC jumped -> residual ~ +0.02
        let (sig_short, z_short, _) = sniper.evaluate_signal(&log_px_overbought);
        assert_eq!(sig_short, VecmSignalAction::EnterShortSpread);
        assert!(z_short > b_star);
    }

    #[test]
    fn test_granger_causality_directionality_and_leadership() {
        let mut gc = GrangerCausalityMatrix::new(3, 0.30);

        // Simulated short-run impact: Asset 0 (Binance) strongly impacts Asset 1 (Hyperliquid) and Asset 2 (Lighter)
        // gamma[i][j] is impact of j on i
        let gamma1 = vec![
            vec![0.10, 0.02, 0.01], // impact on Asset 0 (from 0, 1, 2)
            vec![0.45, 0.08, 0.04], // impact on Asset 1 (from 0 is high!)
            vec![0.55, 0.05, 0.06], // impact on Asset 2 (from 0 is high!)
        ];

        gc.update_from_gammas(&[gamma1]);

        // Asset 0 leads Asset 1:
        // G_10 = 0.45, G_01 = 0.02 -> Asymmetry ratio > 0.30
        assert!(gc.is_leader(0, 1), "Asset 0 should lead Asset 1");
        assert!(!gc.is_leader(1, 0), "Asset 1 should not lead Asset 0");
        assert!(gc.is_leader(0, 2), "Asset 0 should lead Asset 2");

        // Global leader should be Asset 0
        assert_eq!(gc.global_leader_index(), 0);
    }

    #[test]
    fn test_markov_switching_vecm_regimes_and_jumps() {
        let mut engine = MarkovSwitchingVecmEngine::default();

        // 1. In Calm state, tight basis (e.g. 0.0001) should yield FastRevertingElasticEntry and no jump
        let calm_px = vec![10.0001, 10.0000];
        let (decision_calm, jump_calm) = engine.update_log_prices(&calm_px);
        assert!(jump_calm.is_none());
        match decision_calm {
            VecmRegimeDecision::FastRevertingElasticEntry { basis_spread, .. } => {
                assert!(basis_spread > 0.0);
            }
            other => panic!("Expected FastRevertingElasticEntry, got {:?}", other),
        }
        assert!(!engine.is_structural_breakdown());

        // 2. Dislocation / large jump (e.g. 0.0080 = 80 bps >> 3 * 0.0005)
        let jump_px = vec![10.0080, 10.0000];
        let (_decision_jump, jump_detected) = engine.update_log_prices(&jump_px);
        assert!(jump_detected.is_some());
        assert!(jump_detected.unwrap() > 0.007);

        // Feed several extreme dislocations to drive regime to Breakdown
        for _ in 0..5 {
            engine.update_log_prices(&[10.0200, 10.0000]);
        }
        assert!(engine.is_structural_breakdown());
        let probs = engine.current_regime_probabilities();
        assert!(probs[2] >= 0.35, "Regime 2 probability should be elevated");
        let (decision_breakdown, _) = engine.update_log_prices(&[10.0200, 10.0000]);
        match decision_breakdown {
            VecmRegimeDecision::CircuitHaltSupercriticalBurst { .. } => {}
            other => panic!("Expected CircuitHaltSupercriticalBurst, got {:?}", other),
        }
    }
}
