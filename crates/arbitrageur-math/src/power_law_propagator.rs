//! Non-Exponential Power-Law Decay Market Impact Propagators & Multi-Venue Unwind Engine.
//!
//! Implements the Bouchaud-Farmer-Lillo (2009) power-law market impact propagator kernel:
//! G(tau) = Gamma_0 / (1 + tau / t_0)^beta (beta in [0.2, 0.8], default 0.5 for square-root impact),
//! self-induced transient impact filtering for passive unwind quotes, discrete U-shaped optimal
//! execution schedules, and multi-venue inventory liquidation respecting order book replenishment limits.

use serde::{Deserialize, Serialize};

/// Power-law propagator model for a single venue or asset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerLawPropagatorModel {
    /// Instantaneous price impact coefficient Gamma_0 (bps per unit base traded).
    pub gamma_0: f64,
    /// Power-law memory decay exponent beta (typically 0.40 - 0.65; 0.50 for square root law).
    pub beta: f64,
    /// Microscopic order book relaxation timescale t_0 in seconds (typically 0.05 - 0.25s).
    pub t_0_sec: f64,
    /// Order book replenishment depth capacity Lambda (base units).
    pub replenishment_depth: f64,
}

impl Default for PowerLawPropagatorModel {
    fn default() -> Self {
        Self {
            gamma_0: 0.0002, // 2 bps impact per base unit
            beta: 0.50,      // Bouchaud square-root law
            t_0_sec: 0.12,   // 120 ms relaxation time
            replenishment_depth: 5.0,
        }
    }
}

impl PowerLawPropagatorModel {
    /// Evaluates the power-law impact kernel G(tau) at elapsed time tau in seconds.
    pub fn kernel(&self, tau_sec: f64) -> f64 {
        let t0 = self.t_0_sec.max(1e-4);
        let tau = tau_sec.max(0.0);
        self.gamma_0 / (1.0 + tau / t0).powf(self.beta)
    }

    /// Evaluates self-induced transient impact accumulated from a history of child order fills:
    /// Delta P_self(t) = sum_j G(t - t_j) * q_j
    pub fn compute_self_induced_impact(&self, current_time_sec: f64, fills: &[(f64, f64)]) -> f64 {
        let mut total_impact = 0.0;
        for &(fill_time, fill_qty) in fills {
            if current_time_sec >= fill_time {
                let tau = current_time_sec - fill_time;
                total_impact += self.kernel(tau) * fill_qty;
            }
        }
        total_impact
    }

    /// Computes the adjusted mid-price drift in basis points after stripping self-induced market impact.
    /// Returns: observed_drift_bps - self_impact_bps.
    pub fn filter_self_impact(
        &self,
        observed_drift_bps: f64,
        current_time_sec: f64,
        fills: &[(f64, f64)],
        mid_price: f64,
    ) -> f64 {
        if mid_price <= 0.0 {
            return observed_drift_bps;
        }
        let self_impact_usd = self.compute_self_induced_impact(current_time_sec, fills);
        let self_impact_bps = (self_impact_usd / mid_price) * 10000.0;
        observed_drift_bps - self_impact_bps
    }

    /// Computes replenishment-safe trading rate ceiling:
    /// x_dot_max = Lambda * (beta / t_0).
    pub fn replenishment_rate_cap(&self) -> f64 {
        let depth = self.replenishment_depth.max(0.1);
        let t0 = self.t_0_sec.max(1e-4);
        depth * (self.beta / t0)
    }

    /// Computes optimal discrete U-shaped schedule for unwinding inventory Q across N slices.
    /// Inverts the symmetric Toeplitz kernel matrix M_kl = G(|k - l| * Delta t).
    #[allow(clippy::needless_range_loop)]
    pub fn compute_u_shaped_schedule(
        &self,
        total_qty: f64,
        num_slices: usize,
        dt_sec: f64,
    ) -> Vec<f64> {
        if num_slices == 0 {
            return Vec::new();
        }
        if num_slices == 1 {
            return vec![total_qty];
        }

        let n = num_slices;
        // Build symmetric impact matrix M
        let mut m = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                let tau = ((i as isize - j as isize).abs() as f64) * dt_sec;
                m[i][j] = self.kernel(tau);
            }
        }

        // Solve M * y = 1 for weights y (using simple Gauss-Jordan or standard positive definite inversion)
        let ones = vec![1.0; n];
        let mut y =
            solve_symmetric_linear_system(&m, &ones).unwrap_or_else(|| vec![1.0 / (n as f64); n]);

        let y_sum: f64 = y.iter().sum();
        if y_sum.abs() < 1e-12 {
            return vec![total_qty / (n as f64); n];
        }

        // Normalize: q_k = Q * y_k / sum(y)
        for val in &mut y {
            *val = total_qty * (*val / y_sum);
        }

        y
    }
}

/// Plan output for multi-venue inventory liquidation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiVenueUnwindPlan {
    pub venue_names: Vec<String>,
    pub allocated_quantities: Vec<f64>,
    pub slice_trading_rates: Vec<f64>,
    pub expected_total_impact_cost_usd: f64,
    pub is_rate_within_replenishment_cap: bool,
}

/// Coupled multi-venue execution engine under power-law impact memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerLawMultiVenueUnwindEngine {
    pub venue_names: Vec<String>,
    pub models: Vec<PowerLawPropagatorModel>,
}

impl PowerLawMultiVenueUnwindEngine {
    pub fn new(names: Vec<String>, models: Vec<PowerLawPropagatorModel>) -> Self {
        assert_eq!(
            names.len(),
            models.len(),
            "Names and models must have equal length"
        );
        Self {
            venue_names: names,
            models,
        }
    }

    /// Allocates inventory across venues inversely proportional to Gamma_0
    /// and checks replenishment rate limits over horizon `unwind_horizon_sec`.
    pub fn compute_unwind_schedule(
        &self,
        total_inventory: f64,
        unwind_horizon_sec: f64,
    ) -> MultiVenueUnwindPlan {
        let n = self.venue_names.len();
        let tau = unwind_horizon_sec.max(0.1);

        let inv_gamma_sum: f64 = self.models.iter().map(|m| 1.0 / m.gamma_0.max(1e-8)).sum();
        let mut alloc_qty = vec![0.0; n];
        let mut slice_rates = vec![0.0; n];
        let mut total_cost = 0.0;
        let mut all_within_cap = true;

        for m in 0..n {
            let weight = (1.0 / self.models[m].gamma_0.max(1e-8)) / inv_gamma_sum.max(1e-8);
            let q_m = total_inventory * weight;
            let rate_m = q_m / tau;
            let cap_m = self.models[m].replenishment_rate_cap();

            if rate_m > cap_m {
                all_within_cap = false;
            }

            // Power-law impact cost approximation
            let cost_m = 0.5 * self.models[m].gamma_0 * q_m.powi(2)
                / (1.0 + tau / self.models[m].t_0_sec).powf(self.models[m].beta);
            total_cost += cost_m;

            alloc_qty[m] = q_m;
            slice_rates[m] = rate_m;
        }

        MultiVenueUnwindPlan {
            venue_names: self.venue_names.clone(),
            allocated_quantities: alloc_qty,
            slice_trading_rates: slice_rates,
            expected_total_impact_cost_usd: total_cost,
            is_rate_within_replenishment_cap: all_within_cap,
        }
    }
}

/// Helper function to solve A * x = b via Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_symmetric_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut mat = a.to_vec();
    let mut rhs = b.to_vec();

    for i in 0..n {
        // Find pivot
        let mut max_row = i;
        let mut max_val = mat[i][i].abs();
        for k in (i + 1)..n {
            if mat[k][i].abs() > max_val {
                max_val = mat[k][i].abs();
                max_row = k;
            }
        }

        if max_val < 1e-12 {
            return None; // Singular matrix
        }

        mat.swap(i, max_row);
        rhs.swap(i, max_row);

        // Eliminate
        for k in (i + 1)..n {
            let factor = mat[k][i] / mat[i][i];
            for j in i..n {
                mat[k][j] -= factor * mat[i][j];
            }
            rhs[k] -= factor * rhs[i];
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = rhs[i];
        for j in (i + 1)..n {
            sum -= mat[i][j] * x[j];
        }
        x[i] = sum / mat[i][i];
    }

    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_law_kernel_decay() {
        let model = PowerLawPropagatorModel::default();
        let g_0 = model.kernel(0.0);
        let g_100ms = model.kernel(0.10);
        let g_1s = model.kernel(1.0);
        let g_10s = model.kernel(10.0);

        assert_eq!(g_0, model.gamma_0);
        assert!(g_100ms < g_0);
        assert!(g_1s < g_100ms);
        assert!(g_10s < g_1s);
    }

    #[test]
    fn test_self_impact_filtering() {
        let model = PowerLawPropagatorModel::default();
        let mid = 60000.0;

        // At t = 1.0s, we had a buy fill of 1.0 BTC
        let fills = vec![(1.0, 1.0)];

        // At t = 1.05s, observed mid-price drifted upward by 4.0 bps
        let observed_drift_bps = 4.0;
        let filtered_drift_bps = model.filter_self_impact(observed_drift_bps, 1.05, &fills, mid);

        // Self impact should account for a significant portion of the observed drift,
        // so filtered drift is smaller, preventing false adverse selection alarms!
        assert!(filtered_drift_bps < observed_drift_bps);
        assert!(filtered_drift_bps >= 0.0);
    }

    #[test]
    fn test_u_shaped_optimal_schedule() {
        let model = PowerLawPropagatorModel::default();
        let schedule = model.compute_u_shaped_schedule(10.0, 5, 0.5);

        assert_eq!(schedule.len(), 5);
        let sum: f64 = schedule.iter().sum();
        assert!((sum - 10.0).abs() < 1e-4);

        // Bouchaud impact optimal schedule is U-shaped: higher at the ends (0 and 4), lower in middle (2)
        assert!(schedule[0] > schedule[2]);
        assert!(schedule[4] > schedule[2]);
    }

    #[test]
    fn test_multi_venue_unwind_allocation() {
        let names = vec!["hyperliquid".to_string(), "binance".to_string()];
        let models = vec![
            PowerLawPropagatorModel {
                gamma_0: 0.0001, // Deep book, low impact
                beta: 0.50,
                t_0_sec: 0.10,
                replenishment_depth: 10.0,
            },
            PowerLawPropagatorModel {
                gamma_0: 0.0002, // Shallower book, 2x impact
                beta: 0.50,
                t_0_sec: 0.10,
                replenishment_depth: 5.0,
            },
        ];

        let engine = PowerLawMultiVenueUnwindEngine::new(names, models);
        let plan = engine.compute_unwind_schedule(3.0, 10.0);

        // Hyperliquid (lower impact) must receive ~2x quantity of Binance
        assert!(plan.allocated_quantities[0] > plan.allocated_quantities[1]);
        assert!((plan.allocated_quantities[0] - 2.0).abs() < 0.1);
        assert!((plan.allocated_quantities[1] - 1.0).abs() < 0.1);
        assert!(plan.is_rate_within_replenishment_cap);
    }
}
