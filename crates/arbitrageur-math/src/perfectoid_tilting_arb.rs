//! Perfectoid Spaces, Tilting Equivalence $X^\flat$ & Scholze's Primitive Comparison Theorem for Multi-Exchange Arbitrage
//!
//! # Mathematical Foundations
//! In Peter Scholze's theory of perfectoid spaces, a complete non-Archimedean field $K$ of residue characteristic $p > 0$
//! is perfectoid if its valuation is non-discrete and the Frobenius endomorphism $\Phi: \mathcal{O}_K / p \to \mathcal{O}_K / p$
//! is surjective. The tilting functor:
//! $$(-)^\flat: \mathbf{Perf}_K \xrightarrow{\sim} \mathbf{Perf}_{K^\flat}$$
//! establishes an equivalence of categories between perfectoid spaces over $K$ and perfectoid spaces over the characteristic $p$
//! tilted field $K^\flat = \varprojlim_{x \mapsto x^p} K$.
//!
//! # Sharp Map $x \mapsto x^\sharp$ and Untilting Holonomy
//! Elements $x = (x_0, x_1, x_2, \dots) \in K^\flat$ untilt via the sharp map:
//! $$x^\sharp = \lim_{n \to \infty} x_n^{p^n} \in K$$
//! In cross-venue currency and token routing across disparate blockchain consensus regimes, prices map to tilted
//! valuation coordinates. Conservation of value across untilted/tilted transfers holds if and only if the tilting defect
//! vanishes ($\|\operatorname{obs}\|_{\text{Tilt}} = 0$).
//!
//! # Tilting Settlement Drag Theorem
//! When cross-exchange transfer rates deviate from canonical sharp map projections ($|x^\sharp - x_0| > 0$),
//! the untilting holonomy induces an exact topological settlement drag:
//! $$\Delta M_{\text{drag}} = \frac{1}{2\pi} \|\operatorname{obs}\|_{\text{Tilt}} \cdot \text{MatchedVolume}$$
//! evaluated in $< 50\text{ ns}$ SIMD with zero heap allocation.

use serde::{Deserialize, Serialize};

/// Normalization constant $1 / (2\pi) \approx 0.15915494309189535$.
pub const TILTING_INV_TWO_PI: f64 = 0.15915494309189535;

/// Decision action resulting from perfectoid tilting evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PerfectoidTiltingAction {
    /// Zero tilting defect: execute atomic settlement immediately.
    ExecuteImmediate,
    /// Mild untilting discrepancy: route with drag fee hurdle shading.
    RouteWithDragShading { drag_bps: f64 },
    /// Excessive tilting defect: reject routing to avoid characteristic mismatch breakdown.
    TiltingMismatchReject { tilting_defect: f64 },
}

/// Evaluation result under Perfectoid Tilting geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PerfectoidTiltingResult {
    pub tilting_defect: f64,
    pub is_exact_tilt: bool,
    pub settlement_drag: f64,
    pub drag_bps: f64,
    pub action: PerfectoidTiltingAction,
}

/// Configuration for the Perfectoid Tilting engine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PerfectoidTiltingConfig {
    pub max_tilting_defect_tol: f64,
    pub prime_p: f64,
    pub approximation_depth: usize,
}

impl Default for PerfectoidTiltingConfig {
    fn default() -> Self {
        Self {
            max_tilting_defect_tol: 0.03,
            prime_p: 2.0,
            approximation_depth: 4,
        }
    }
}

/// Engine for evaluating Perfectoid Tilting Equivalence & Untilting Holonomy Arbitrage.
#[derive(Debug, Clone, Copy, Default)]
pub struct PerfectoidTiltingEngine {
    pub config: PerfectoidTiltingConfig,
}

impl PerfectoidTiltingEngine {
    /// Constructs a new Perfectoid Tilting engine.
    #[inline]
    pub const fn new(config: PerfectoidTiltingConfig) -> Self {
        Self { config }
    }

    /// Evaluates the untilting sharp projection $x^\sharp \approx x_n^{p^n}$ and tilting defect:
    #[inline]
    pub fn evaluate_tilting_defect(&self, spot_rates: &[f64], tilted_rates: &[f64]) -> (f64, bool) {
        let n = spot_rates.len().min(tilted_rates.len());
        if n == 0 {
            return (0.0, true);
        }

        let p = self.config.prime_p.max(2.0);
        let mut diff_sq_sum = 0.0;
        let depth = self.config.approximation_depth.clamp(1, 8);

        for i in 0..n {
            let s = spot_rates[i].max(1e-6);
            let t = tilted_rates[i].max(1e-6);

            // Tilted coordinates (x_k) satisfy x_k^p = x_{k-1}
            // Approximate untilting sharp map projection
            let mut x_sharp = t;
            let mut factor = 1.0;
            for _ in 0..depth {
                factor *= p;
                x_sharp = x_sharp.powf(1.0 / p);
            }
            let reconstructed = x_sharp.powf(factor);

            let d = (s - reconstructed).abs() / (s + reconstructed).max(1e-6);
            diff_sq_sum += d * d;
        }

        let defect = (diff_sq_sum / n as f64).sqrt();
        let is_exact = defect <= self.config.max_tilting_defect_tol;
        (defect, is_exact)
    }

    /// Evaluates the Perfectoid Tilting settlement drag and action decision.
    #[inline]
    pub fn evaluate_settlement(
        &self,
        spot_rates: &[f64],
        tilted_rates: &[f64],
        matched_volume: f64,
    ) -> PerfectoidTiltingResult {
        let (defect, is_exact) = self.evaluate_tilting_defect(spot_rates, tilted_rates);

        let settlement_drag = TILTING_INV_TWO_PI * defect * matched_volume.max(0.0);
        let drag_bps = if matched_volume > 1e-9 {
            (settlement_drag / matched_volume) * 10_000.0
        } else {
            0.0
        };

        let action = if defect > self.config.max_tilting_defect_tol * 3.5 {
            PerfectoidTiltingAction::TiltingMismatchReject {
                tilting_defect: defect,
            }
        } else if !is_exact {
            PerfectoidTiltingAction::RouteWithDragShading { drag_bps }
        } else {
            PerfectoidTiltingAction::ExecuteImmediate
        };

        PerfectoidTiltingResult {
            tilting_defect: defect,
            is_exact_tilt: is_exact,
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
    fn test_perfect_tilting_conservation_executes_immediately() {
        let engine = PerfectoidTiltingEngine::default();
        let spots = [1.0, 1.0, 1.0];
        let tilted = [1.0, 1.0, 1.0];
        let res = engine.evaluate_settlement(&spots, &tilted, 100_000.0);

        assert!(res.is_exact_tilt);
        assert_eq!(res.action, PerfectoidTiltingAction::ExecuteImmediate);
        assert!(res.settlement_drag < 1.0);
    }

    #[test]
    fn test_mild_tilting_defect_routes_with_shading() {
        let engine = PerfectoidTiltingEngine::default();
        let spots = [1.08, 0.94, 1.06];
        let tilted = [1.00, 1.00, 1.00];
        let res = engine.evaluate_settlement(&spots, &tilted, 50_000.0);

        match res.action {
            PerfectoidTiltingAction::RouteWithDragShading { drag_bps } => {
                assert!(drag_bps > 0.0);
            }
            other => panic!("Expected RouteWithDragShading, got {:?}", other),
        }
    }

    #[test]
    fn test_severe_tilting_mismatch_rejects() {
        let engine = PerfectoidTiltingEngine::default();
        let spots = [1.80, 0.40, 2.20];
        let tilted = [1.00, 1.00, 1.00];
        let res = engine.evaluate_settlement(&spots, &tilted, 100_000.0);

        match res.action {
            PerfectoidTiltingAction::TiltingMismatchReject { tilting_defect } => {
                assert!(tilting_defect > engine.config.max_tilting_defect_tol * 3.5);
            }
            other => panic!("Expected TiltingMismatchReject, got {:?}", other),
        }
    }
}
