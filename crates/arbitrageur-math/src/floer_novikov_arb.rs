//! Floer-Novikov Cohomology, Symplectic Flux Homology & Non-Exact Settlement Obstructions
//!
//! # Mathematical Foundations
//! In non-exact symplectic liquidity phase spaces $(M, \omega)$ where $d\omega = 0$ but $[\omega] \neq 0 \in H^2(M; \mathbb{R})$,
//! cross-venue cyclic order routing is modeled on the loop space $\mathcal{L}M = C^\infty(S^1, M)$.
//!
//! Closed multi-venue routing paths $\gamma \in \mathcal{L}M$ evaluate against the symplectic form $\omega$,
//! producing non-trivial **Symplectic Flux Homology**:
//! $$\operatorname{Flux}(\tilde{\gamma}) = \int_{\tilde{\gamma}} \omega = \frac{1}{2} \sum_{i < j} \Omega_{ij} (q_i p_j - q_j p_i)$$
//! where $\mathbf{q} \in \mathbb{R}^d$ and $\mathbf{p} \in \mathbb{R}^d$ represent positions and conjugate price momentums across $d \le 4$ venues.
//!
//! # Novikov Ring $\Lambda_\omega$ & Multi-Sheeted Covering Sheets
//! When $\omega$ is non-exact, the action functional $\mathcal{A}_H$ is multi-valued on $\mathcal{L}M$, necessitating passage to
//! the Novikov universal covering $\widetilde{\mathcal{L}M}$. The chain complex is defined over the **Novikov Ring**:
//! $$\Lambda_\omega = \left\{ \sum_{i=0}^{K-1} a_i T^{\lambda_i} \;\middle|\; a_i \in \mathbb{R}, \; 0 \le \lambda_0 \le \lambda_1 \le \dots \le \lambda_{K-1} \right\}$$
//! where $\lambda_i$ denotes the symplectic area energy filtration level of pseudoholomorphic curves, and $a_i$ denotes Floer chain amplitude.
//! The canonical Novikov norm is:
//! $$\|\Lambda_\omega\|_{\text{Novikov}} = \sum_{i=0}^{K-1} |a_i| e^{-\lambda_i}$$
//! and the topological flux obstruction norm is:
//! $$\|\operatorname{obs}\|_{\text{Flux}} = |\operatorname{Flux}(\tilde{\gamma})| + \sum_{i=0}^{K-1} |a_i| (1 - e^{-\lambda_i})$$
//!
//! # Floer-Novikov Flux Obstruction Settlement Drag Theorem
//! In a fragmented multi-venue liquidity loop, atomic cycle transfers incur non-zero topological dissipation:
//! $$\Delta M_{\text{drag}} = \frac{1}{\sqrt{2\pi}} \|\operatorname{obs}\|_{\text{Flux}} \cdot \text{Spread}$$
//! unless the symplectic flux holonomy vanishes homologically ($\|\operatorname{obs}\|_{\text{Flux}} = 0$).

use serde::{Deserialize, Serialize};

/// Maximum dimension supported for cross-venue symplectic manifolds.
pub const NOVIKOV_MAX_VENUES: usize = 4;

/// Number of discrete terms retained in the truncated Novikov series.
pub const NOVIKOV_SERIES_TERMS: usize = 4;

/// Normalization constant $1 / \sqrt{2\pi}$ for Gaussian flux dissipation.
pub const INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Element of the Novikov Ring $\Lambda_\omega$ parameterizing energy filtration branches.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NovikovRingElement {
    /// Chain amplitudes $a_i$.
    pub coefficients: [f64; NOVIKOV_SERIES_TERMS],
    /// Symplectic area energy levels $\lambda_i \ge 0$.
    pub energy_exponents: [f64; NOVIKOV_SERIES_TERMS],
}

impl Default for NovikovRingElement {
    fn default() -> Self {
        Self {
            coefficients: [1.0, 0.0, 0.0, 0.0],
            energy_exponents: [0.0, 1.0, 2.0, 3.0],
        }
    }
}

impl NovikovRingElement {
    /// Create a new Novikov ring element with given coefficients and energy exponents.
    #[inline]
    pub fn new(
        coefficients: [f64; NOVIKOV_SERIES_TERMS],
        energy_exponents: [f64; NOVIKOV_SERIES_TERMS],
    ) -> Self {
        Self {
            coefficients,
            energy_exponents,
        }
    }

    /// Compute the Novikov filtration norm: $\|\Lambda_\omega\| = \sum |a_i| e^{-\lambda_i}$.
    #[inline]
    pub fn norm(&self) -> f64 {
        let mut total = 0.0;
        for i in 0..NOVIKOV_SERIES_TERMS {
            total += self.coefficients[i].abs() * (-self.energy_exponents[i].max(0.0)).exp();
        }
        total
    }

    /// Compute the topological non-exact obstruction from higher energy sheets:
    /// $\sum |a_i| (1 - e^{-\lambda_i})$.
    #[inline]
    pub fn topological_obstruction(&self) -> f64 {
        let mut obs = 0.0;
        for i in 0..NOVIKOV_SERIES_TERMS {
            let lambda = self.energy_exponents[i].max(0.0);
            obs += self.coefficients[i].abs() * (1.0 - (-lambda).exp());
        }
        obs
    }
}

/// Phase space state for cross-venue symplectic flux evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SymplecticFluxState {
    /// Skew-symmetric symplectic 2-form tensor $\boldsymbol{\Omega} \in \mathbb{R}^{d \times d}$ ($\Omega_{ij} = -\Omega_{ji}$).
    pub omega: [[f64; NOVIKOV_MAX_VENUES]; NOVIKOV_MAX_VENUES],
    /// Configuration/position coordinates $\mathbf{q} \in \mathbb{R}^d$ across venues.
    pub q: [f64; NOVIKOV_MAX_VENUES],
    /// Conjugate price/momentum coordinates $\mathbf{p} \in \mathbb{R}^d$ across venues.
    pub p: [f64; NOVIKOV_MAX_VENUES],
}

impl Default for SymplecticFluxState {
    fn default() -> Self {
        Self {
            omega: [[0.0; NOVIKOV_MAX_VENUES]; NOVIKOV_MAX_VENUES],
            q: [0.0; NOVIKOV_MAX_VENUES],
            p: [0.0; NOVIKOV_MAX_VENUES],
        }
    }
}

impl SymplecticFluxState {
    /// Create a standard canonical 2-form for pairwise venues.
    #[allow(clippy::needless_range_loop)]
    pub fn canonical(
        q: [f64; NOVIKOV_MAX_VENUES],
        p: [f64; NOVIKOV_MAX_VENUES],
        coupling: f64,
    ) -> Self {
        let mut omega = [[0.0; NOVIKOV_MAX_VENUES]; NOVIKOV_MAX_VENUES];
        for i in 0..NOVIKOV_MAX_VENUES {
            for j in (i + 1)..NOVIKOV_MAX_VENUES {
                omega[i][j] = coupling;
                omega[j][i] = -coupling;
            }
        }
        Self { omega, q, p }
    }
}

/// Floer-Novikov engine computing symplectic flux homology, obstruction norms, and settlement drag.
#[derive(Debug, Clone, Copy, Default)]
pub struct FloerNovikovEngine;

impl FloerNovikovEngine {
    /// Create a new Floer-Novikov engine.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// Compute the symplectic flux across cross-venue loop $\gamma$:
    /// $$\operatorname{Flux}(\tilde{\gamma}) = \sum_{i < j} \Omega_{ij} (q_i p_j - q_j p_i)$$
    #[inline]
    pub fn compute_flux(&self, state: &SymplecticFluxState) -> f64 {
        let mut flux = 0.0;
        for i in 0..NOVIKOV_MAX_VENUES {
            for j in (i + 1)..NOVIKOV_MAX_VENUES {
                let term = state.q[i] * state.p[j] - state.q[j] * state.p[i];
                flux += state.omega[i][j] * term;
            }
        }
        flux
    }

    /// Evaluate the total Floer-Novikov flux obstruction norm:
    /// $$\|\operatorname{obs}\|_{\text{Flux}} = |\operatorname{Flux}(\tilde{\gamma})| + \operatorname{obs}_{\text{Novikov}}$$
    #[inline]
    pub fn evaluate_flux_obstruction(
        &self,
        state: &SymplecticFluxState,
        novikov: &NovikovRingElement,
    ) -> f64 {
        let flux = self.compute_flux(state);
        flux.abs() + novikov.topological_obstruction()
    }

    /// Prove and evaluate the Floer-Novikov Flux Obstruction Settlement Drag Theorem:
    /// $$\Delta M_{\text{drag}} = \frac{1}{\sqrt{2\pi}} \|\operatorname{obs}\|_{\text{Flux}} \cdot \text{Spread}$$
    #[inline]
    pub fn settlement_drag(
        &self,
        state: &SymplecticFluxState,
        novikov: &NovikovRingElement,
        spread: f64,
    ) -> f64 {
        let obs = self.evaluate_flux_obstruction(state, novikov);
        INV_SQRT_TWO_PI * obs * spread.max(0.0)
    }

    /// Determine if the cross-venue cyclic transfer is homologically flux-free ($\|\operatorname{obs}\|_{\text{Flux}} \le \varepsilon$).
    #[inline]
    pub fn is_flux_free(
        &self,
        state: &SymplecticFluxState,
        novikov: &NovikovRingElement,
        tolerance: f64,
    ) -> bool {
        self.evaluate_flux_obstruction(state, novikov) <= tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_symplectic_vanishing_flux_zero_drag() {
        let engine = FloerNovikovEngine::new();

        // Exact symplectic form has omega = 0, or symmetric collinear q, p
        let state = SymplecticFluxState {
            omega: [[0.0; NOVIKOV_MAX_VENUES]; NOVIKOV_MAX_VENUES],
            q: [1.0, 2.0, 3.0, 4.0],
            p: [1.0, 2.0, 3.0, 4.0],
        };

        // Novikov element concentrated solely at energy lambda = 0
        let novikov = NovikovRingElement::new([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]);

        let flux = engine.compute_flux(&state);
        assert_eq!(flux, 0.0);

        let obs = engine.evaluate_flux_obstruction(&state, &novikov);
        assert!(obs.abs() < 1e-12);

        let drag = engine.settlement_drag(&state, &novikov, 10.0);
        assert!(drag.abs() < 1e-12);
        assert!(engine.is_flux_free(&state, &novikov, 1e-6));
    }

    #[test]
    fn test_non_exact_flux_obstruction_settlement_drag() {
        let engine = FloerNovikovEngine::new();

        let state = SymplecticFluxState::canonical([1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], 2.0);
        // Non-zero Novikov sheet excitation
        let novikov = NovikovRingElement::new([1.0, 0.5, 0.0, 0.0], [0.0, 1.5, 0.0, 0.0]);

        let flux = engine.compute_flux(&state);
        assert!(flux > 0.0);

        let obs = engine.evaluate_flux_obstruction(&state, &novikov);
        assert!(obs > flux); // includes Novikov obstruction

        let spread = 25.0; // 25 bps spread
        let drag = engine.settlement_drag(&state, &novikov, spread);
        assert!(drag > 0.0);
        let expected = INV_SQRT_TWO_PI * obs * spread;
        assert!((drag - expected).abs() < 1e-10);
    }

    #[test]
    fn test_novikov_ring_energy_filtration_monotonicity() {
        // As lambda -> inf, e^-lambda -> 0, so norm decreases
        let elem_low = NovikovRingElement::new([1.0, 1.0, 1.0, 1.0], [0.1, 0.2, 0.3, 0.4]);
        let elem_high = NovikovRingElement::new([1.0, 1.0, 1.0, 1.0], [2.0, 3.0, 4.0, 5.0]);

        assert!(elem_low.norm() > elem_high.norm());
        // Higher energy excitation has higher obstruction (1 - e^-lambda is larger)
        assert!(elem_high.topological_obstruction() > elem_low.topological_obstruction());
    }
}
