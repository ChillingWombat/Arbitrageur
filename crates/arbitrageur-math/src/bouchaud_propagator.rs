//! Adaptive TWAP/VWAP Execution Engine with Non-Linear Bouchaud Transient Impact Decay.
//!
//! Models market impact as a transient propagator process:
//! I(t) = \int_0^t G(t - s) \dot{x}(s) ds
//! where G(\tau) = \Gamma_0 (\tau + \tau_0)^{-\gamma} is the Bouchaud power-law kernel (\gamma \approx 0.5).
//!
//! Unlike naive linear Almgren-Chriss or static TWAP, the transient propagator kernel captures
//! the empirical resilience and liquidity replenishment of order books, deriving the optimal
//! U-shaped execution trajectory \mathbf{q}^* = Q \frac{\mathbf{M}^{-1} \mathbf{1}}{\mathbf{1}^T \mathbf{M}^{-1} \mathbf{1}}
//! that avoids book exhaustion while minimizing expected slippage and timing variance.

use serde::{Deserialize, Serialize};

/// Bouchaud power-law transient propagator kernel parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BouchaudPropagatorModel {
    /// Decay exponent \gamma in (0, 1), typically 0.40 - 0.60
    pub gamma_decay: f64,
    /// Impact scaling coefficient \Gamma_0 > 0
    pub gamma_0: f64,
    /// Short-term regularization offset \tau_0 > 0 (prevents singularity at \tau = 0)
    pub tau_0: f64,
}

impl Default for BouchaudPropagatorModel {
    fn default() -> Self {
        Self {
            gamma_decay: 0.50,
            gamma_0: 0.05,
            tau_0: 0.10,
        }
    }
}

impl BouchaudPropagatorModel {
    pub fn new(gamma_decay: f64, gamma_0: f64, tau_0: f64) -> Self {
        Self {
            gamma_decay: gamma_decay.clamp(0.05, 0.95),
            gamma_0: gamma_0.max(1e-6),
            tau_0: tau_0.max(1e-4),
        }
    }

    /// Evaluates propagator kernel G(\tau) = \Gamma_0 (\tau + \tau_0)^{-\gamma}:
    pub fn kernel_value(&self, tau_sec: f64) -> f64 {
        let t = tau_sec.max(0.0) + self.tau_0;
        self.gamma_0 * t.powf(-self.gamma_decay)
    }

    /// Computes cumulative decayed transient price impact I(t) from a sequence of past executions:
    /// I(t) = \sum_{j} G(t - t_j) q_j
    pub fn compute_transient_impact(
        &self,
        past_trades: &[(f64, f64)], // (timestamp_sec, signed_quantity)
        current_time_sec: f64,
    ) -> f64 {
        let mut total_impact = 0.0;
        for &(t_j, q_j) in past_trades {
            if current_time_sec >= t_j {
                let tau = current_time_sec - t_j;
                total_impact += self.kernel_value(tau) * q_j;
            }
        }
        total_impact
    }

    /// Returns dynamic impact scaling inflated by instantaneous Hawkes co-jump intensity:
    /// Gamma_0(lambda_t) = Gamma_0_base * (1 + kappa * max(0, lambda_t - mu_base) / mu_base)^delta
    pub fn inflated_gamma_0(&self, hawkes_intensity: f64, baseline_mu: f64, kappa: f64) -> f64 {
        if hawkes_intensity <= baseline_mu || baseline_mu <= 0.0 {
            self.gamma_0
        } else {
            let excess = (hawkes_intensity - baseline_mu) / baseline_mu;
            self.gamma_0 * (1.0 + kappa.max(0.0) * excess).sqrt()
        }
    }

    /// Evaluates kernel with live Hawkes intensity coupling.
    pub fn kernel_value_with_hawkes(
        &self,
        tau_sec: f64,
        hawkes_intensity: f64,
        baseline_mu: f64,
    ) -> f64 {
        let g0_eff = self.inflated_gamma_0(hawkes_intensity, baseline_mu, 1.5);
        let t = tau_sec.max(0.0) + self.tau_0;
        g0_eff * t.powf(-self.gamma_decay)
    }
}

/// Computes the discrete optimal execution trajectory under the Bouchaud propagator kernel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptimalBouchaudTrajectory {
    pub propagator: BouchaudPropagatorModel,
    /// Instantaneous temporary impact coefficient \eta
    pub temporary_impact_eta: f64,
    /// Timing risk aversion \lambda_{risk} \sigma^2
    pub risk_variance_coeff: f64,
}

impl Default for OptimalBouchaudTrajectory {
    fn default() -> Self {
        Self {
            propagator: BouchaudPropagatorModel::default(),
            temporary_impact_eta: 0.02,
            risk_variance_coeff: 1e-4,
        }
    }
}

impl OptimalBouchaudTrajectory {
    pub fn new(
        propagator: BouchaudPropagatorModel,
        temporary_impact_eta: f64,
        risk_variance_coeff: f64,
    ) -> Self {
        Self {
            propagator,
            temporary_impact_eta: temporary_impact_eta.max(1e-6),
            risk_variance_coeff: risk_variance_coeff.max(0.0),
        }
    }

    /// Solves the optimal execution schedule [q_1, q_2, ..., q_N] for total size Q
    /// over horizon T with N steps:
    /// \mathbf{M} \mathbf{y} = \mathbf{1}, \quad \mathbf{q}^* = Q \frac{\mathbf{y}}{\mathbf{1}^T \mathbf{y}}
    #[allow(clippy::needless_range_loop)]
    pub fn compute_schedule(&self, total_size: f64, horizon_sec: f64, steps: usize) -> Vec<f64> {
        let n = steps.max(1);
        if n == 1 || horizon_sec <= 0.0 || total_size.abs() < 1e-12 {
            return vec![total_size];
        }

        let dt = horizon_sec / n as f64;
        let mut matrix = vec![vec![0.0; n]; n];

        // Construct symmetric Toeplitz kernel impact matrix M
        for i in 0..n {
            let t_i = i as f64 * dt;
            for j in 0..n {
                let t_j = j as f64 * dt;
                let tau = (t_i - t_j).abs();
                let kernel_term = self.propagator.kernel_value(tau);
                let diag_term = if i == j {
                    self.temporary_impact_eta
                } else {
                    0.0
                };
                let risk_term = self.risk_variance_coeff * dt * ((n - i.max(j)) as f64);
                matrix[i][j] = kernel_term + diag_term + risk_term;
            }
        }

        // Solve M * y = 1 via Gaussian Elimination with partial pivoting
        let rhs = vec![1.0; n];
        let y = match solve_linear_system(&matrix, &rhs) {
            Some(sol) => sol,
            None => vec![1.0 / n as f64; n], // Fallback to equal slices
        };

        let sum_y: f64 = y.iter().sum();
        if sum_y.abs() < 1e-12 {
            return vec![total_size / n as f64; n];
        }

        y.iter().map(|&yi| total_size * (yi / sum_y)).collect()
    }

    /// Solves optimal execution schedule adapting impact scale to live Hawkes toxic intensity.
    pub fn compute_schedule_with_hawkes(
        &self,
        total_size: f64,
        horizon_sec: f64,
        steps: usize,
        hawkes_intensity: f64,
        baseline_mu: f64,
    ) -> Vec<f64> {
        let mut model = self.propagator;
        model.gamma_0 = model.inflated_gamma_0(hawkes_intensity, baseline_mu, 1.5);
        let traj = OptimalBouchaudTrajectory {
            propagator: model,
            temporary_impact_eta: self.temporary_impact_eta,
            risk_variance_coeff: self.risk_variance_coeff,
        };
        traj.compute_schedule(total_size, horizon_sec, steps)
    }
}

/// Gaussian elimination solver with partial pivoting for dense N x N systems.
#[allow(clippy::needless_range_loop)]
fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut mat = a.to_vec();
    let mut rhs = b.to_vec();

    for i in 0..n {
        // Find pivot
        let mut max_row = i;
        let mut max_val = mat[i][i].abs();
        for k in (i + 1)..n {
            let v = mat[k][i].abs();
            if v > max_val {
                max_val = v;
                max_row = k;
            }
        }
        if max_val < 1e-12 {
            return None; // Singular matrix
        }
        mat.swap(i, max_row);
        rhs.swap(i, max_row);

        // Eliminate
        let pivot = mat[i][i];
        for k in (i + 1)..n {
            let factor = mat[k][i] / pivot;
            for j in i..n {
                mat[k][j] -= factor * mat[i][j];
            }
            rhs[k] -= factor * rhs[i];
        }
    }

    // Back-substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = rhs[i];
        for j in (i + 1)..n {
            sum -= mat[i][j] * x[j];
        }
        if mat[i][i].abs() < 1e-12 {
            return None;
        }
        x[i] = sum / mat[i][i];
    }

    Some(x)
}

/// Parameters for closed-form optimal child slice pacing interval under viscoelastic resilience.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChildSlicePacingParams {
    /// Characteristic order book replenishment relaxation timescale tau in seconds (e.g. 0.25s)
    pub tau_replenish_sec: f64,
    /// Kohlrausch-Williams-Watts stretched exponential exponent beta in (0, 1] (e.g. 0.65)
    pub beta_exponent: f64,
    /// Permanent market impact fraction rho_inf in [0, 1) (e.g. 0.15)
    pub rho_permanent: f64,
    /// Basis or spread price volatility per second (e.g. 0.0002)
    pub basis_volatility_per_sec: f64,
    /// Minimum allowed pacing interval in milliseconds (e.g. 25.0 ms)
    pub min_interval_ms: f64,
    /// Maximum allowed pacing interval in milliseconds (e.g. 2500.0 ms)
    pub max_interval_ms: f64,
}

impl Default for ChildSlicePacingParams {
    fn default() -> Self {
        Self {
            tau_replenish_sec: 0.25,
            beta_exponent: 0.65,
            rho_permanent: 0.15,
            basis_volatility_per_sec: 0.0002,
            min_interval_ms: 25.0,
            max_interval_ms: 2500.0,
        }
    }
}

impl ChildSlicePacingParams {
    pub fn new(
        tau_replenish_sec: f64,
        beta_exponent: f64,
        rho_permanent: f64,
        basis_volatility_per_sec: f64,
        min_interval_ms: f64,
        max_interval_ms: f64,
    ) -> Self {
        Self {
            tau_replenish_sec: tau_replenish_sec.clamp(0.01, 10.0),
            beta_exponent: beta_exponent.clamp(0.1, 1.0),
            rho_permanent: rho_permanent.clamp(0.0, 0.95),
            basis_volatility_per_sec: basis_volatility_per_sec.max(1e-6),
            min_interval_ms: min_interval_ms.max(1.0),
            max_interval_ms: max_interval_ms.max(min_interval_ms),
        }
    }

    /// Evaluates closed-form optimal child slice replenishment pacing interval in milliseconds:
    /// Delta t_slice* = tau * [ sigma_basis * sqrt(tau) / (2 * beta * Delta P_0 * (1 - rho_inf)) ]^(2/3)
    pub fn compute_optimal_pacing_interval_ms(
        &self,
        slice_qty: f64,
        temporary_impact_eta: f64,
        reference_price: f64,
    ) -> f64 {
        let q = slice_qty.abs().max(1e-6);
        let p_ref = reference_price.max(1.0);
        let eta = temporary_impact_eta.max(1e-6);

        // Instantaneous price displacement in USD: Delta P_0 = eta * sqrt(q)
        let delta_p0 = (eta * q.sqrt() / p_ref).max(1e-6);

        let tau = self.tau_replenish_sec;
        let beta = self.beta_exponent;
        let rho = self.rho_permanent;
        let sigma = self.basis_volatility_per_sec;

        let denom = 2.0 * beta * delta_p0 * (1.0 - rho).max(0.05);
        let num = sigma * tau.sqrt();

        let ratio = (num / denom).max(1e-6);
        let dt_sec = tau * ratio.powf(2.0 / 3.0);
        let dt_ms = dt_sec * 1000.0;

        dt_ms.clamp(self.min_interval_ms, self.max_interval_ms)
    }
}

/// Adaptive volume-matching and non-linear slippage-bounded execution router.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptivePropagatorRouter {
    pub base_trajectory: OptimalBouchaudTrajectory,
    /// Maximum allowed marginal slippage in basis points
    pub max_slippage_bps: f64,
    /// Volume adaptation responsiveness parameter (e.g. 0.5)
    pub volume_participation_power: f64,
    /// Child slice pacing parameters under order book resilience
    pub child_pacing_params: ChildSlicePacingParams,
}

impl Default for AdaptivePropagatorRouter {
    fn default() -> Self {
        Self {
            base_trajectory: OptimalBouchaudTrajectory::default(),
            max_slippage_bps: 15.0,
            volume_participation_power: 0.5,
            child_pacing_params: ChildSlicePacingParams::default(),
        }
    }
}

impl AdaptivePropagatorRouter {
    pub fn new(
        base_trajectory: OptimalBouchaudTrajectory,
        max_slippage_bps: f64,
        volume_participation_power: f64,
    ) -> Self {
        Self {
            base_trajectory,
            max_slippage_bps: max_slippage_bps.max(1.0),
            volume_participation_power: volume_participation_power.clamp(0.1, 1.5),
            child_pacing_params: ChildSlicePacingParams::default(),
        }
    }

    pub fn with_child_pacing_params(mut self, params: ChildSlicePacingParams) -> Self {
        self.child_pacing_params = params;
        self
    }

    /// Computes optimal replenishment delay before the next child slice dispatch in milliseconds.
    pub fn compute_optimal_slice_pacing_ms(&self, slice_qty: f64, reference_price: f64) -> f64 {
        self.child_pacing_params.compute_optimal_pacing_interval_ms(
            slice_qty,
            self.base_trajectory.temporary_impact_eta,
            reference_price,
        )
    }

    /// Adapts the theoretical slice size to live market conditions:
    /// 1. Scales by the realized volume surge/dips: (V_{live} / V_{expected})^\alpha
    /// 2. Verifies non-linear slippage bound: \text{SlippageBps}(q) \le \text{MaxSlippageBps}
    pub fn compute_adapted_slice(
        &self,
        nominal_slice: f64,
        remaining_order_qty: f64,
        realized_volume_ratio: f64, // (realized_volume / expected_volume), > 1.0 indicates elevated liquidity
        spread_bps: f64,
        reference_price: f64,
    ) -> Result<f64, String> {
        if remaining_order_qty.abs() < 1e-9 {
            return Ok(0.0);
        }

        let vol_ratio = realized_volume_ratio.clamp(0.2, 5.0);
        let vol_scale = vol_ratio.powf(self.volume_participation_power);
        let desired_slice = (nominal_slice * vol_scale).min(remaining_order_qty);

        // Estimate non-linear market impact slippage in bps:
        // Slippage = 0.5 * Spread + \eta_{eff} * desired_slice^{0.5} / RefPrice * 10^4
        let half_spread = spread_bps.max(0.1) * 0.5;
        let p_ref = reference_price.max(1.0);
        let impact_bps = (self.base_trajectory.temporary_impact_eta * desired_slice.abs().sqrt()
            / p_ref)
            * 10_000.0;
        let total_est_slippage_bps = half_spread + impact_bps;

        if total_est_slippage_bps > self.max_slippage_bps {
            // Clamp slice to fit within slippage budget
            let allowable_impact_bps = (self.max_slippage_bps - half_spread).max(0.0);
            let allowable_impact_usd = allowable_impact_bps * 1e-4 * p_ref;
            let eta = self.base_trajectory.temporary_impact_eta.max(1e-6);
            let max_allowed_slice = (allowable_impact_usd / eta).powi(2);

            let clamped_slice = desired_slice.min(max_allowed_slice);
            if clamped_slice < nominal_slice * 0.20 {
                return Err(format!(
                    "Excessive slippage risk: est {:.2} bps > max {:.2} bps",
                    total_est_slippage_bps, self.max_slippage_bps
                ));
            }
            Ok(clamped_slice)
        } else {
            Ok(desired_slice)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propagator_decay_monotonicity() {
        let model = BouchaudPropagatorModel::default();
        let g0 = model.kernel_value(0.0);
        let g1 = model.kernel_value(1.0);
        let g10 = model.kernel_value(10.0);

        assert!(
            g0 > g1,
            "Propagator kernel must decay with time lag: g0={} vs g1={}",
            g0,
            g1
        );
        assert!(
            g1 > g10,
            "Propagator kernel must decay further at 10s: g1={} vs g10={}",
            g1,
            g10
        );
    }

    #[test]
    fn test_transient_impact_accumulation_and_decay() {
        let model = BouchaudPropagatorModel::default();
        let trades = vec![
            (0.0, 1.0), // 1 unit bought at t=0
            (1.0, 1.0), // 1 unit bought at t=1
        ];

        let impact_at_1_5 = model.compute_transient_impact(&trades, 1.5);
        let impact_at_5_0 = model.compute_transient_impact(&trades, 5.0);

        assert!(
            impact_at_1_5 > impact_at_5_0,
            "Impact must decay over time as memory fades"
        );
    }

    #[test]
    fn test_optimal_u_shaped_trajectory() {
        let traj = OptimalBouchaudTrajectory::default();
        let schedule = traj.compute_schedule(100.0, 10.0, 5);

        assert_eq!(schedule.len(), 5);
        let sum: f64 = schedule.iter().sum();
        assert!(
            (sum - 100.0).abs() < 1e-6,
            "Schedule must sum to total size: {}",
            sum
        );

        // U-shaped verification: first and last steps are larger than interior steps
        let first = schedule[0];
        let mid = schedule[2];
        let last = schedule[4];

        assert!(
            first > mid,
            "Optimal propagator trajectory is U-shaped: first ({}) > mid ({})",
            first,
            mid
        );
        assert!(
            last > mid,
            "Optimal propagator trajectory is U-shaped: last ({}) > mid ({})",
            last,
            mid
        );
    }

    #[test]
    fn test_adaptive_router_volume_adaptation_and_slippage_clamping() {
        let router = AdaptivePropagatorRouter::default();

        // 1. High volume surge (vol_ratio = 2.0): should increase slice size
        let slice_high_vol = router
            .compute_adapted_slice(10.0, 100.0, 2.0, 2.0, 50_000.0)
            .expect("Should adapt slice");
        assert!(
            slice_high_vol > 10.0,
            "High volume ratio should increase participation slice"
        );

        // 2. Huge slice in illiquid conditions: should trigger clamping or error
        let res_illiquid = router.compute_adapted_slice(1000.0, 1000.0, 0.2, 25.0, 100.0);
        assert!(
            res_illiquid.is_err(),
            "Excessive slippage risk should return error"
        );
    }

    #[test]
    fn test_hawkes_inflated_gamma_0_and_kernel() {
        let model = BouchaudPropagatorModel::default();
        let base_g0 = model.gamma_0;

        // At baseline intensity (lambda <= mu), gamma_0 remains uninflated
        let g0_normal = model.inflated_gamma_0(1.0, 1.0, 1.5);
        assert!((g0_normal - base_g0).abs() < 1e-9);

        // Under toxic surge (lambda = 5.0, mu = 1.0), gamma_0 is inflated
        let g0_toxic = model.inflated_gamma_0(5.0, 1.0, 1.5);
        assert!(
            g0_toxic > base_g0,
            "Toxic Hawkes intensity must inflate impact scale: {g0_toxic} > {base_g0}"
        );

        // Kernel values scale accordingly
        let k_base = model.kernel_value(0.5);
        let k_hawkes = model.kernel_value_with_hawkes(0.5, 5.0, 1.0);
        assert!(
            k_hawkes > k_base,
            "Kernel value under toxic Hawkes must exceed base kernel"
        );

        // Optimal trajectory schedule with Hawkes
        let traj = OptimalBouchaudTrajectory::default();
        let sched = traj.compute_schedule_with_hawkes(100.0, 10.0, 5, 4.0, 1.0);
        assert_eq!(sched.len(), 5);
        let sum: f64 = sched.iter().sum();
        assert!((sum - 100.0).abs() < 1e-4);
    }

    #[test]
    fn test_child_slice_optimal_pacing_interval() {
        let params = ChildSlicePacingParams::default();
        let eta = 0.02;
        let p_ref = 60_000.0;

        // Small slice (1.0 unit) vs large slice (25.0 units)
        let delay_small = params.compute_optimal_pacing_interval_ms(1.0, eta, p_ref);
        let delay_large = params.compute_optimal_pacing_interval_ms(25.0, eta, p_ref);

        assert!(delay_small >= params.min_interval_ms);
        assert!(delay_large <= params.max_interval_ms);
        assert!(delay_small <= delay_large, "Larger slice needs longer or equal replenishment pacing: {delay_small} vs {delay_large}");

        // Router method
        let router = AdaptivePropagatorRouter::default();
        let pacer_delay = router.compute_optimal_slice_pacing_ms(5.0, p_ref);
        assert!((25.0..=2500.0).contains(&pacer_delay));
    }
}
