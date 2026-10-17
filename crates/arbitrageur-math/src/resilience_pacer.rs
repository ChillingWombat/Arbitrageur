//! Multi-Asset High-Frequency Order Book Resilience Pacing & Replenishment Dynamics.
//!
//! Models the transient viscoelastic relaxation of limit order book depth post-aggressive
//! execution, preventing mutual adverse selection and book exhaustion across concurrent
//! multi-leg arbitrage legs.
//!
//! # Mathematical Foundations
//!
//! 1. **Viscoelastic Order Book Relaxation**:
//!    Post-sweep order book recovery follows a Kohlrausch-Williams-Watts (KWW) stretched
//!    exponential decay:
//!    $$R(t) = (1 - \rho_\infty) \exp\left(-\left(\frac{t}{\tau_{\text{replenish}}}\right)^\beta\right) + \rho_\infty$$
//!    where:
//!    - $\rho_\infty \in [0.10, 0.45]$: permanent price impact fraction remaining after full book recovery.
//!    - $\tau_{\text{replenish}} > 0$: characteristic order book replenishment time scale (typically 50ms - 500ms).
//!    - $\beta \in (0.0, 1.0]$: stretched exponential relaxation exponent ($\beta \approx 0.5$ for viscoelastic liquidity).
//!
//! 2. **Instantaneous Price Displacement**:
//!    Initial displacement for child slice quantity $q$:
//!    $$\Delta P_0 = \eta \cdot q^\alpha, \quad \alpha \in [0.5, 0.8]$$
//!    Residual price displacement at elapsed time $t$:
//!    $$\Delta P_{\text{residual}}(t) = \Delta P_0 \cdot R(t)$$
//!
//! 3. **Closed-Form Optimal Child Slice Pacing Interval**:
//!    Balancing unrelaxed adverse cross-impact cost against spread variance drift $\sigma_{\text{basis}} \sqrt{\Delta t}$:
//!    $$\mathcal{C}(\Delta t) = \Delta P_0 (1 - \rho_\infty) \exp\left(-\left(\frac{\Delta t}{\tau_{\text{replenish}}}\right)^\beta\right) + \sigma_{\text{basis}} \sqrt{\Delta t}$$
//!    Balancing marginal costs gives the optimal pacing interval:
//!    $$\Delta t_{\text{slice}}^* = \tau_{\text{replenish}} \cdot \left(\frac{\sigma_{\text{basis}} \sqrt{\tau_{\text{replenish}}}}{2 \beta \Delta P_0 (1 - \rho_\infty)}\right)^{2/3}$$
//!    clamped to operational safety bounds $[\Delta t_{\min}, \Delta t_{\max}]$ (e.g. $[20\text{ms}, 2500\text{ms}]$).
//!
//! 4. **Multi-Asset Cross-Impact Coupling**:
//!    Cross-asset displacement on asset $i$ from execution on asset $j$:
//!    $$\Delta P_{i \leftarrow j}(t) = \mathbf{\Gamma}_{ij} \cdot \Delta P_{0, j} \cdot R_j(t)$$

use serde::{Deserialize, Serialize};

/// Order book resilience and replenishment parameters for a specific trading venue / instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderBookResilienceParam {
    /// Venue or instrument identifier.
    pub venue_id: String,
    /// Characteristic replenishment time scale $\tau_{\text{replenish}}$ in seconds.
    pub tau_replenish_sec: f64,
    /// Stretched exponential exponent $\beta \in (0.0, 1.0]$.
    pub beta_exponent: f64,
    /// Permanent price impact fraction $\rho_\infty \in [0.0, 1.0]$.
    pub permanent_impact_fraction: f64,
    /// Instantaneous price impact coefficient $\eta$.
    pub eta_impact: f64,
    /// Sub-linear power-law impact exponent $\alpha \in [0.5, 1.0]$.
    pub alpha_exponent: f64,
}

impl Default for OrderBookResilienceParam {
    fn default() -> Self {
        Self {
            venue_id: "default".into(),
            tau_replenish_sec: 0.150, // 150 ms typical perp replenishment
            beta_exponent: 0.50,      // Kohlrausch stretched exponential
            permanent_impact_fraction: 0.25, // 25% permanent impact
            eta_impact: 0.00012,      // Baseline impact coefficient
            alpha_exponent: 0.65,     // Sub-linear concavity
        }
    }
}

impl OrderBookResilienceParam {
    /// Computes the instantaneous price displacement $\Delta P_0 = \eta \cdot q^\alpha$.
    pub fn instantaneous_displacement(&self, slice_qty: f64) -> f64 {
        let q = slice_qty.abs();
        if q <= 1e-12 {
            0.0
        } else {
            self.eta_impact * q.powf(self.alpha_exponent)
        }
    }

    /// Evaluates the viscoelastic relaxation function:
    /// $$R(t) = (1 - \rho_\infty) \exp\left(-\left(\frac{t}{\tau}\right)^\beta\right) + \rho_\infty$$
    pub fn relaxation_factor(&self, elapsed_sec: f64) -> f64 {
        if elapsed_sec <= 0.0 {
            return 1.0;
        }
        let tau = self.tau_replenish_sec.max(1e-4);
        let beta = self.beta_exponent.clamp(0.1, 1.0);
        let rho_inf = self.permanent_impact_fraction.clamp(0.0, 1.0);

        let arg = (elapsed_sec / tau).powf(beta);
        (1.0 - rho_inf) * (-arg).exp() + rho_inf
    }

    /// Computes the residual price displacement at time $t$ post-execution:
    /// $$\Delta P_{\text{residual}}(t) = \Delta P_0 \cdot R(t)$$
    pub fn residual_displacement(&self, slice_qty: f64, elapsed_sec: f64) -> f64 {
        let dp0 = self.instantaneous_displacement(slice_qty);
        dp0 * self.relaxation_factor(elapsed_sec)
    }

    /// Computes the closed-form optimal child slice pacing interval $\Delta t_{\text{slice}}^*$ (in seconds):
    /// $$\Delta t_{\text{slice}}^* = \tau \cdot \left(\frac{\sigma_{\text{basis}} \sqrt{\tau}}{2 \beta \Delta P_0 (1 - \rho_\infty)}\right)^{2/3}$$
    ///
    /// # Arguments
    /// - `slice_qty`: Quantity of the executed slice.
    /// - `sigma_basis_sec`: Spread volatility in units per $\sqrt{\text{second}}$.
    /// - `min_interval_sec`: Minimum operational pacing floor (e.g. 0.020 s = 20 ms).
    /// - `max_interval_sec`: Maximum allowable shelf-life delay (e.g. 1.500 s).
    pub fn optimal_pacing_interval_sec(
        &self,
        slice_qty: f64,
        sigma_basis_sec: f64,
        min_interval_sec: f64,
        max_interval_sec: f64,
    ) -> f64 {
        let dp0 = self.instantaneous_displacement(slice_qty);
        let tau = self.tau_replenish_sec.max(1e-4);
        let beta = self.beta_exponent.clamp(0.1, 1.0);
        let rho_inf = self.permanent_impact_fraction.clamp(0.0, 0.95);
        let trans_impact = dp0 * (1.0 - rho_inf);

        if trans_impact <= 1e-12 || sigma_basis_sec <= 1e-12 {
            return min_interval_sec;
        }

        let num = sigma_basis_sec * tau.sqrt();
        let denom = 2.0 * beta * trans_impact;
        let ratio = (num / denom).abs();
        let optimal_dt = tau * ratio.powf(2.0 / 3.0);

        optimal_dt.clamp(min_interval_sec, max_interval_sec)
    }
}

/// Order book replenishment health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookReplenishmentStatus {
    /// Order book is substantially relaxed ($R(t) \le \rho_\infty + 0.10$).
    FullyRelaxed,
    /// Order book is actively replenishing ($0.10 < R(t) - \rho_\infty \le 0.40$).
    Replenishing,
    /// Order book is under heavy transient displacement ($R(t) - \rho_\infty > 0.40$).
    Strained,
}

/// Child slice execution decision and pacing recommendation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildSliceRecommendation {
    /// Recommended quantity for the next child slice.
    pub slice_quantity: f64,
    /// Remaining parent quantity after this slice.
    pub remaining_quantity: f64,
    /// Recommended pacing wait interval before the subsequent slice (in seconds).
    pub pacing_interval_sec: f64,
    /// Expected residual price displacement of this slice in USD/unit.
    pub expected_residual_impact: f64,
    /// Current book replenishment state.
    pub book_status: BookReplenishmentStatus,
    /// Estimated adverse selection hurdle penalty in basis points.
    pub adverse_selection_penalty_bps: f64,
}

/// Controller coordinating multi-leg order book resilience pacing and child slicing.
#[derive(Debug, Clone)]
pub struct CrossImpactResiliencePacer {
    /// Venue-specific resilience parameters.
    pub params_a: OrderBookResilienceParam,
    pub params_b: OrderBookResilienceParam,
    /// Minimum operational pacing floor in seconds (default 20 ms).
    pub min_pacing_sec: f64,
    /// Maximum operational pacing ceiling in seconds (default 1.50 s).
    pub max_pacing_sec: f64,
    /// Safe top-of-book depth consumption fraction $\theta_{\text{depth}} \in [0.10, 0.80]$.
    pub depth_fraction: f64,
    /// Cross-impact coupling matrix entry $\mathbf{\Gamma}_{AB}$ (default 0.65).
    pub cross_coupling_gamma: f64,
}

impl Default for CrossImpactResiliencePacer {
    fn default() -> Self {
        Self {
            params_a: OrderBookResilienceParam {
                venue_id: "venue_a".into(),
                ..Default::default()
            },
            params_b: OrderBookResilienceParam {
                venue_id: "venue_b".into(),
                ..Default::default()
            },
            min_pacing_sec: 0.020, // 20 ms
            max_pacing_sec: 1.500, // 1.50 s
            depth_fraction: 0.50,  // Consume at most 50% of available top level
            cross_coupling_gamma: 0.65,
        }
    }
}

impl CrossImpactResiliencePacer {
    pub fn new(
        params_a: OrderBookResilienceParam,
        params_b: OrderBookResilienceParam,
        depth_fraction: f64,
    ) -> Self {
        Self {
            params_a,
            params_b,
            min_pacing_sec: 0.020,
            max_pacing_sec: 1.500,
            depth_fraction: depth_fraction.clamp(0.10, 0.80),
            cross_coupling_gamma: 0.65,
        }
    }

    /// Evaluates current replenishment status on venue $A$ given elapsed time since last sweep.
    pub fn evaluate_replenishment_status(&self, elapsed_sec: f64) -> BookReplenishmentStatus {
        let r = self.params_a.relaxation_factor(elapsed_sec);
        let excess = (r - self.params_a.permanent_impact_fraction).max(0.0);
        if excess <= 0.10 {
            BookReplenishmentStatus::FullyRelaxed
        } else if excess <= 0.40 {
            BookReplenishmentStatus::Replenishing
        } else {
            BookReplenishmentStatus::Strained
        }
    }

    /// Calculates the optimal synchronized child slice size and pacing interval for a two-leg pair.
    ///
    /// # Arguments
    /// - `parent_qty_remaining`: Remaining un-executed parent trade quantity.
    /// - `depth_l1_venue_a`: Instantaneous available L1 depth on Venue A.
    /// - `depth_l1_venue_b`: Instantaneous available L1 depth on Venue B.
    /// - `spread_volatility_sec`: Spread volatility per $\sqrt{\text{second}}$.
    /// - `mid_price`: Prevailing instrument mid price.
    pub fn compute_child_slice_recommendation(
        &self,
        parent_qty_remaining: f64,
        depth_l1_venue_a: f64,
        depth_l1_venue_b: f64,
        spread_volatility_sec: f64,
        mid_price: f64,
    ) -> ChildSliceRecommendation {
        let max_depth_a = depth_l1_venue_a.max(0.0) * self.depth_fraction;
        let max_depth_b = depth_l1_venue_b.max(0.0) * self.depth_fraction;
        let depth_cap = max_depth_a.min(max_depth_b).max(1e-6);

        // Optimal slice is bounded by available top-of-book depth and remaining parent size
        let slice_qty = parent_qty_remaining.abs().min(depth_cap);
        let rem_qty = (parent_qty_remaining.abs() - slice_qty).max(0.0);

        // Pacing balances slowest replenishment between both venues
        let dt_a = self.params_a.optimal_pacing_interval_sec(
            slice_qty,
            spread_volatility_sec,
            self.min_pacing_sec,
            self.max_pacing_sec,
        );
        let dt_b = self.params_b.optimal_pacing_interval_sec(
            slice_qty,
            spread_volatility_sec,
            self.min_pacing_sec,
            self.max_pacing_sec,
        );
        let pacing_sec = dt_a.max(dt_b);

        // Residual impact includes cross-impact coupling
        let direct_impact = self.params_a.instantaneous_displacement(slice_qty);
        let cross_impact =
            self.cross_coupling_gamma * self.params_b.instantaneous_displacement(slice_qty);
        let combined_impact = direct_impact + cross_impact;

        let adverse_bps = if mid_price > 1e-12 {
            (combined_impact / mid_price) * 10_000.0
        } else {
            0.0
        };

        let status = self.evaluate_replenishment_status(pacing_sec);

        ChildSliceRecommendation {
            slice_quantity: slice_qty,
            remaining_quantity: rem_qty,
            pacing_interval_sec: pacing_sec,
            expected_residual_impact: combined_impact,
            book_status: status,
            adverse_selection_penalty_bps: adverse_bps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viscoelastic_relaxation_factor() {
        let param = OrderBookResilienceParam {
            tau_replenish_sec: 0.200,
            beta_exponent: 0.50,
            permanent_impact_fraction: 0.20,
            ..Default::default()
        };

        // At t = 0, R(0) = 1.0 (100% displacement)
        let r0 = param.relaxation_factor(0.0);
        assert!((r0 - 1.0).abs() < 1e-6);

        // At t = tau = 0.200 s:
        // arg = (0.200 / 0.200)^0.5 = 1.0
        // R(tau) = 0.80 * exp(-1) + 0.20 = 0.80 * 0.367879 + 0.20 = 0.4943
        let r_tau = param.relaxation_factor(0.200);
        assert!((r_tau - 0.4943).abs() < 0.01);

        // At t -> inf, R(t) -> rho_inf = 0.20
        let r_inf = param.relaxation_factor(100.0);
        assert!((r_inf - 0.20).abs() < 1e-4);
    }

    #[test]
    fn test_instantaneous_and_residual_displacement() {
        let param = OrderBookResilienceParam {
            eta_impact: 0.001,
            alpha_exponent: 0.50,
            tau_replenish_sec: 0.100,
            beta_exponent: 0.50,
            permanent_impact_fraction: 0.25,
            ..Default::default()
        };

        // q = 4.0 -> sqrt(4.0) = 2.0 -> dp0 = 0.001 * 2.0 = 0.002
        let dp0 = param.instantaneous_displacement(4.0);
        assert!((dp0 - 0.002).abs() < 1e-6);

        // Immediate residual at t = 0 is 0.002
        let res0 = param.residual_displacement(4.0, 0.0);
        assert!((res0 - 0.002).abs() < 1e-6);

        // Long time residual is 0.002 * 0.25 = 0.0005
        let res_inf = param.residual_displacement(4.0, 10.0);
        assert!((res_inf - 0.0005).abs() < 1e-5);
    }

    #[test]
    fn test_optimal_pacing_interval_bounds() {
        let param = OrderBookResilienceParam::default();

        let dt = param.optimal_pacing_interval_sec(1.0, 0.0005, 0.020, 1.500);
        assert!(dt >= 0.020);
        assert!(dt <= 1.500);

        // Zero volatility collapses to minimum operational floor
        let dt_zero_vol = param.optimal_pacing_interval_sec(1.0, 0.0, 0.020, 1.500);
        assert!((dt_zero_vol - 0.020).abs() < 1e-6);
    }

    #[test]
    fn test_cross_impact_resilience_pacer() {
        let pacer = CrossImpactResiliencePacer::default();

        let rec = pacer.compute_child_slice_recommendation(
            10.0,   // parent qty
            2.0,    // L1 depth venue A
            3.0,    // L1 depth venue B
            0.0002, // spread vol
            3000.0, // mid price
        );

        // Available depth min is 2.0 * 0.50 = 1.0
        assert!((rec.slice_quantity - 1.0).abs() < 1e-6);
        assert!((rec.remaining_quantity - 9.0).abs() < 1e-6);
        assert!(rec.pacing_interval_sec >= 0.020);
        assert!(rec.pacing_interval_sec <= 1.500);
        assert!(rec.expected_residual_impact > 0.0);
        assert!(rec.adverse_selection_penalty_bps > 0.0);
    }
}
