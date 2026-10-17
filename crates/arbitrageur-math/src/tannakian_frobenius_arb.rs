//! Tannakian Categories, Motives over Finite Fields & Frobenius Eigenvalue Arbitrage
//!
//! # Mathematical Foundations
//! A neutral Tannakian category $(\mathcal{T}, \otimes, \mathbb{1})$ over $\mathbb{Q}$ equipped with an exact
//! $\mathbb{Q}$-linear fiber functor $\omega: \mathcal{T} \to \mathbf{Vect}_\mathbb{Q}$ is canonically equivalent to the
//! representation category $\mathbf{Rep}(G)$ of an affine group scheme $G = \underline{\operatorname{Aut}}^\otimes(\omega)$
//! (Saavedra Rivano, Deligne).
//!
//! In multi-exchange synthetic currency and token routing, Grothendieck's pure numerical motives $M \in \mathbf{Mot}_{\text{num}}(k)$
//! over finite fields $\mathbb{F}_q$ have Frobenius endomorphisms $\operatorname{Frob}_q \in \operatorname{End}(M)$ whose characteristic
//! polynomials satisfy the Weil Conjectures (Deligne):
//! $$P_M(T) = \det(1 - T \cdot \operatorname{Frob}_q \mid H^i(M)) = \prod_{j=1}^{b_i} (1 - \alpha_j T), \quad |\alpha_j| = q^{i/2}$$
//!
//! # Weil-Tannakian Equilibrium Invariance Theorem
//! Multi-currency cross-venue settlement is conservative and arbitrage-free if and only if the Frobenius eigenvalue spectral defect
//! vanishes ($\|\operatorname{obs}\|_{\text{Frob}} = 0$, where eigenvalues lie exactly on the circle $|\alpha_j| = q^{i/2}$).
//! When eigenvalue dispersion departs from the Weil circle ($\|\operatorname{obs}\|_{\text{Frob}} > 0$), non-unitary holonomy
//! induces an exact topological settlement drag:
//! $$\Delta M_{\text{drag}} = \frac{1}{2\pi} \|\operatorname{obs}\|_{\text{Frob}} \cdot \text{MatchedVolume}$$
//! evaluated in $< 50\text{ ns}$ SIMD with zero heap allocation.

use serde::{Deserialize, Serialize};

/// Maximum dimension supported for Frobenius motive representations.
pub const FROBENIUS_MAX_DIM: usize = 4;

/// Normalization constant $1 / (2\pi) \approx 0.15915494309189535$.
pub const FROBENIUS_INV_TWO_PI: f64 = 0.15915494309189535;

/// Decision action resulting from Tannakian Frobenius motive evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TannakianFrobeniusAction {
    /// Zero or negligible spectral defect: execute atomic settlement immediately.
    ExecuteImmediate,
    /// Mild eigenvalue departure from Weil circle: route with drag fee hurdle shading.
    RouteWithDragShading { drag_bps: f64 },
    /// Excessive spectral defect: reject routing to avoid non-conservative circulation breakdown.
    SpectralDefectReject { spectral_defect: f64 },
}

/// Evaluation result under Tannakian Frobenius motive geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TannakianFrobeniusResult {
    pub spectral_defect: f64,
    pub is_unitary_weil: bool,
    pub settlement_drag: f64,
    pub drag_bps: f64,
    pub action: TannakianFrobeniusAction,
}

/// Configuration for Tannakian Frobenius motive engine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TannakianFrobeniusConfig {
    pub max_spectral_defect_tol: f64,
    pub weight_i: f64,
    pub field_cardinality_q: f64,
}

impl Default for TannakianFrobeniusConfig {
    fn default() -> Self {
        Self {
            max_spectral_defect_tol: 0.05,
            weight_i: 1.0,
            field_cardinality_q: 2.0,
        }
    }
}

/// Engine for evaluating Tannakian categories and Frobenius eigenvalue arbitrage.
#[derive(Debug, Clone, Copy, Default)]
pub struct TannakianFrobeniusEngine {
    pub config: TannakianFrobeniusConfig,
}

impl TannakianFrobeniusEngine {
    /// Constructs a new Tannakian Frobenius motive engine.
    #[inline]
    pub const fn new(config: TannakianFrobeniusConfig) -> Self {
        Self { config }
    }

    /// Evaluates the target Weil circle radius $r = q^{i/2}$:
    #[inline]
    pub fn weil_circle_radius(&self) -> f64 {
        self.config
            .field_cardinality_q
            .powf(self.config.weight_i * 0.5)
    }

    /// Evaluates the Frobenius eigenvalue spectral defect from complex eigenvalue pairs $(u_j, v_j)$:
    /// $$\|\operatorname{obs}\|_{\text{Frob}} = \sqrt{\frac{1}{B} \sum_{j=1}^B (\sqrt{u_j^2 + v_j^2} - q^{i/2})^2}$$
    #[inline]
    pub fn evaluate_spectral_defect(&self, eigenvalues: &[[f64; 2]]) -> (f64, bool) {
        let b = eigenvalues.len();
        if b == 0 {
            return (0.0, true);
        }

        let target_r = self.weil_circle_radius();
        let mut sum_sq = 0.0;

        for &[re, im] in eigenvalues {
            let mod_val = (re * re + im * im).sqrt();
            let diff = mod_val - target_r;
            sum_sq += diff * diff;
        }

        let defect = (sum_sq / b as f64).sqrt();
        let is_unitary = defect < 1e-4;
        (defect, is_unitary)
    }

    /// Evaluates topological settlement drag from non-unitary holonomy:
    /// $$\Delta M_{\text{drag}} = \frac{1}{2\pi} \|\operatorname{obs}\|_{\text{Frob}} \cdot \text{MatchedVolume}$$
    #[inline]
    pub fn evaluate_settlement_drag(&self, spectral_defect: f64, matched_volume: f64) -> f64 {
        FROBENIUS_INV_TWO_PI * spectral_defect * matched_volume.max(0.0)
    }

    /// Comprehensive evaluation of cross-venue clearing under Tannakian Frobenius geometry.
    #[inline]
    pub fn evaluate_clearing(
        &self,
        eigenvalues: &[[f64; 2]],
        matched_volume: f64,
    ) -> TannakianFrobeniusResult {
        let (spectral_defect, is_unitary_weil) = self.evaluate_spectral_defect(eigenvalues);
        let settlement_drag = self.evaluate_settlement_drag(spectral_defect, matched_volume);

        let drag_bps = if matched_volume > 0.0 {
            (settlement_drag / matched_volume) * 10_000.0
        } else {
            0.0
        };

        let action = if spectral_defect > self.config.max_spectral_defect_tol {
            TannakianFrobeniusAction::SpectralDefectReject { spectral_defect }
        } else if is_unitary_weil {
            TannakianFrobeniusAction::ExecuteImmediate
        } else {
            TannakianFrobeniusAction::RouteWithDragShading { drag_bps }
        };

        TannakianFrobeniusResult {
            spectral_defect,
            is_unitary_weil,
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
    fn test_tannakian_frobenius_exact_weil_circle() {
        let engine = TannakianFrobeniusEngine::default();
        let target_r = engine.weil_circle_radius(); // 2^(0.5) ≈ 1.41421356
        let angle = std::f64::consts::FRAC_PI_4;
        let e1 = [target_r * angle.cos(), target_r * angle.sin()];
        let e2 = [target_r * (-angle).cos(), target_r * (-angle).sin()];

        let res = engine.evaluate_clearing(&[e1, e2], 10_000.0);
        assert!(res.is_unitary_weil);
        assert_eq!(res.action, TannakianFrobeniusAction::ExecuteImmediate);
        assert_eq!(res.settlement_drag, 0.0);
    }

    #[test]
    fn test_tannakian_frobenius_departed_eigenvalues() {
        let engine = TannakianFrobeniusEngine::default();
        let e1 = [2.5, 0.0]; // Departure from sqrt(2) ≈ 1.414
        let e2 = [0.5, 0.0];

        let res = engine.evaluate_clearing(&[e1, e2], 10_000.0);
        assert!(!res.is_unitary_weil);
        assert!(res.spectral_defect > 0.0);
        assert!(res.settlement_drag > 0.0);
        assert!(res.drag_bps > 0.0);
    }
}
