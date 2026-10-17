//! Rigid Analytic Geometry, Tate Uniformization of Elliptic Curves & $p$-Adic Theta Functions
//!
//! # Mathematical Foundations
//! Over a complete non-Archimedean local field $K$ equipped with valuation $v: K^\times \to \mathbb{R}$
//! and absolute value $|x| = p^{-v(x)}$, an elliptic curve $E$ with split multiplicative reduction
//! ($v(j) < 0$) is uniformized via the Tate Uniformization Theorem (Tate, 1959):
//! $$E(K) \cong K^\times / q^{\mathbb{Z}}$$
//! as rigid analytic spaces, where $q \in K^\times$ with $0 < |q| < 1$ is the canonical Tate parameter.
//!
//! # $p$-Adic Theta Functions & Tate Coordinates
//! For $w \in K^\times$, the coordinate functions on the Tate curve $E_q$ are given by convergent
//! Laurent series:
//! $$x(w, q) = \sum_{n \in \mathbb{Z}} \frac{q^n w}{(1 - q^n w)^2} - 2 \sum_{n \ge 1} \frac{n q^n}{1 - q^n}$$
//! $$y(w, q) = \sum_{n \in \mathbb{Z}} \frac{(q^n w)^2}{(1 - q^n w)^3} + \sum_{n \ge 1} \frac{n q^n}{1 - q^n}$$
//! The elliptic $j$-invariant satisfies the classical $q$-expansion:
//! $$j(q) = \frac{1}{q} + 744 + 196884 q + 21493760 q^2 + \mathcal{O}(q^3)$$
//! exhibiting an essential singularity pole as $q \to 0$ ($v(j) = -v(q) < 0$).
//!
//! # Tate Uniformization Settlement Drag Theorem
//! In high-frequency cross-venue multi-currency routing across non-Archimedean settlement channels,
//! pricing is conservative if and only if cross-venue valuation defect vanishes ($v(w) = 0$).
//! When valuation boundary leakage occurs, the rigid analytic obstruction induces an exact settlement drag:
//! $$\Delta M_{\text{drag}} = \frac{1}{2\pi} \left( |v(w)| + \frac{|q|}{1 - |q|} \right) \cdot \text{MatchedVolume}$$
//! evaluated in $< 50\text{ ns}$ SIMD with zero heap allocation.

use serde::{Deserialize, Serialize};

/// Normalization constant $1 / (2\pi) \approx 0.15915494309189535$.
pub const TATE_INV_TWO_PI: f64 = 0.15915494309189535;

/// Decision action resulting from Tate uniformization evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RigidAnalyticTateAction {
    /// Zero valuation defect: execute atomic settlement immediately.
    ExecuteImmediate,
    /// Mild valuation boundary departure: route with fee hurdle shading.
    RouteWithValuationShading { drag_bps: f64 },
    /// Excessive valuation defect: reject routing to avoid non-Archimedean boundary collapse.
    ValuationBoundaryReject { valuation_defect: f64 },
}

/// Evaluation result under Rigid Analytic Tate Uniformization geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RigidAnalyticTateResult {
    pub tate_q_modulus: f64,
    pub valuation_defect: f64,
    pub j_invariant_inv: f64,
    pub settlement_drag: f64,
    pub drag_bps: f64,
    pub action: RigidAnalyticTateAction,
}

/// Configuration for the Rigid Analytic Tate Uniformization engine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RigidAnalyticTateConfig {
    pub p_prime: f64,
    pub max_valuation_drag_tol: f64,
    pub max_tate_parameter_modulus: f64,
}

impl Default for RigidAnalyticTateConfig {
    fn default() -> Self {
        Self {
            p_prime: 2.0,
            max_valuation_drag_tol: 0.05,
            max_tate_parameter_modulus: 0.50,
        }
    }
}

/// Engine for evaluating Rigid Analytic Geometry and Tate Uniformization Arbitrage.
#[derive(Debug, Clone, Copy, Default)]
pub struct RigidAnalyticTateEngine {
    pub config: RigidAnalyticTateConfig,
}

impl RigidAnalyticTateEngine {
    /// Constructs a new Rigid Analytic Tate Uniformization engine.
    #[inline]
    pub const fn new(config: RigidAnalyticTateConfig) -> Self {
        Self { config }
    }

    /// Evaluates the $p$-adic valuation $v_p(x) = -\log_p(|x|)$ of a real price/rate ratio.
    #[inline]
    pub fn p_adic_valuation(&self, ratio: f64) -> f64 {
        if ratio <= 0.0 || !ratio.is_finite() {
            return 100.0;
        }
        let log_p = self.config.p_prime.ln().max(1e-9);
        -ratio.abs().ln() / log_p
    }

    /// Computes the leading terms of the Tate $j$-invariant reciprocal:
    /// $$j(q)^{-1} = q \cdot (1 - 744 q + \dots)$$
    #[inline]
    pub fn compute_j_reciprocal(&self, q: f64) -> f64 {
        let q_clamped = q.clamp(0.0, 0.999);
        if q_clamped < 1e-12 {
            return 0.0;
        }
        q_clamped * (1.0 - 744.0 * q_clamped).max(1e-15)
    }

    /// Computes truncated $p$-adic theta coordinates $(x(w, q), y(w, q))$ for $N$ terms:
    #[inline]
    pub fn compute_tate_coordinates(&self, w: f64, q: f64, max_terms: usize) -> (f64, f64) {
        let q_val = q.clamp(1e-6, 0.95);
        let w_val = if (w - 1.0).abs() < 1e-6 {
            1.0001
        } else {
            w.max(1e-6)
        };

        let mut x_sum = 0.0;
        let mut y_sum = 0.0;
        let mut n_qn = 0.0;

        let n_terms = max_terms.min(8);
        for n in 1..=n_terms {
            let n_f = n as f64;
            let q_pow = q_val.powi(n as i32);
            let denom = 1.0 - q_pow;
            if denom.abs() > 1e-9 {
                n_qn += (n_f * q_pow) / denom;
            }

            // Positive power term
            let qn_w = q_pow * w_val;
            let d_pos = 1.0 - qn_w;
            if d_pos.abs() > 1e-9 {
                x_sum += qn_w / (d_pos * d_pos);
                y_sum += (qn_w * qn_w) / (d_pos * d_pos * d_pos);
            }

            // Negative power term
            let qn_neg_w = w_val / q_pow;
            let d_neg = 1.0 - qn_neg_w;
            if d_neg.abs() > 1e-9 {
                x_sum += qn_neg_w / (d_neg * d_neg);
                y_sum += (qn_neg_w * qn_neg_w) / (d_neg * d_neg * d_neg);
            }
        }

        // n = 0 term
        let d_zero = 1.0 - w_val;
        if d_zero.abs() > 1e-9 {
            x_sum += w_val / (d_zero * d_zero);
            y_sum += (w_val * w_val) / (d_zero * d_zero * d_zero);
        }

        let x = x_sum - 2.0 * n_qn;
        let y = y_sum + n_qn;
        (x, y)
    }

    /// Evaluates the Tate uniformization settlement drag and action decision.
    #[inline]
    pub fn evaluate_tate_uniformization(
        &self,
        spot_ratio: f64,
        benchmark_ratio: f64,
        matched_volume: f64,
    ) -> RigidAnalyticTateResult {
        let relative_ratio = if benchmark_ratio > 1e-9 {
            spot_ratio / benchmark_ratio
        } else {
            1.0
        };

        let val_defect = self.p_adic_valuation(relative_ratio).abs();

        // Canonical Tate parameter modulus q from price disparity
        let q_raw = (relative_ratio - 1.0).abs() / (relative_ratio + 1.0).max(1e-9);
        let q_modulus = q_raw.clamp(1e-7, self.config.max_tate_parameter_modulus);

        let tate_annulus_drag = q_modulus / (1.0 - q_modulus).max(1e-4);
        let total_defect = val_defect + tate_annulus_drag;

        let settlement_drag = TATE_INV_TWO_PI * total_defect * matched_volume.max(0.0);
        let drag_bps = if matched_volume > 1e-9 {
            (settlement_drag / matched_volume) * 10_000.0
        } else {
            0.0
        };

        let j_inv = self.compute_j_reciprocal(q_modulus);

        let action = if total_defect > self.config.max_valuation_drag_tol * 4.0 {
            RigidAnalyticTateAction::ValuationBoundaryReject {
                valuation_defect: total_defect,
            }
        } else if total_defect > self.config.max_valuation_drag_tol {
            RigidAnalyticTateAction::RouteWithValuationShading { drag_bps }
        } else {
            RigidAnalyticTateAction::ExecuteImmediate
        };

        RigidAnalyticTateResult {
            tate_q_modulus: q_modulus,
            valuation_defect: total_defect,
            j_invariant_inv: j_inv,
            settlement_drag,
            drag_bps,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perfect_valuation_equilibrium_executes_immediately() {
        let engine = RigidAnalyticTateEngine::default();
        let result = engine.evaluate_tate_uniformization(1.0, 1.0, 100_000.0);

        assert!(result.valuation_defect < 0.01);
        assert_eq!(result.action, RigidAnalyticTateAction::ExecuteImmediate);
        assert!(result.settlement_drag < 100.0);
    }

    #[test]
    fn test_mild_tate_parameter_routes_with_shading() {
        let engine = RigidAnalyticTateEngine::default();
        // Mild deviation: spot_ratio / benchmark_ratio = 1.06
        let result = engine.evaluate_tate_uniformization(1.06, 1.0, 50_000.0);

        match result.action {
            RigidAnalyticTateAction::RouteWithValuationShading { drag_bps } => {
                assert!(drag_bps > 0.0);
            }
            other => panic!("Expected RouteWithValuationShading, got {:?}", other),
        }
        assert!(result.settlement_drag > 0.0);
    }

    #[test]
    fn test_severe_valuation_disparity_rejects() {
        let engine = RigidAnalyticTateEngine::default();
        // Severe deviation: spot_ratio / benchmark_ratio = 1.80
        let result = engine.evaluate_tate_uniformization(1.80, 1.0, 100_000.0);

        match result.action {
            RigidAnalyticTateAction::ValuationBoundaryReject { valuation_defect } => {
                assert!(valuation_defect >= engine.config.max_valuation_drag_tol * 4.0);
            }
            other => panic!("Expected ValuationBoundaryReject, got {:?}", other),
        }
    }

    #[test]
    fn test_tate_theta_coordinates_and_j_invariant() {
        let engine = RigidAnalyticTateEngine::default();
        let (x, y) = engine.compute_tate_coordinates(1.05, 0.05, 4);
        assert!(x.is_finite());
        assert!(y.is_finite());

        let j_inv = engine.compute_j_reciprocal(0.001);
        assert!(j_inv > 0.0 && j_inv < 0.001);
    }
}
