//! Supersymmetric Quantum Mechanics (SUSY QM) Instantons for Metastable LOB Tunneling Transitions.
//!
//! Maps high-frequency Fokker-Planck order book queue imbalance dynamics to Witten's SUSY
//! Hamiltonian $H_{\text{SUSY}} = Q^\dagger Q$ with superpotential $W(x) = \frac{-a x + b x^3}{2 \sqrt{D}}$:
//! - Characterizes metastable balance states separated by asymmetric potential barriers.
//! - Closed-form instanton Kramers tunneling transition rate:
//!   $$\Gamma_{\text{tunnel}} = \frac{a \sqrt{2}}{\pi} \exp\left( -\frac{a^2}{4 b D} \right)$$
//! - Evaluates rapid queue depletion risk, triggering resting maker quote evacuation in $< 65\text{ ms}$
//!   when $\Gamma_{\text{tunnel}} \ge 15.0\text{ s}^{-1}$.

use serde::{Deserialize, Serialize};

/// Parameters for the SUSY QM double-well order book superpotential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SusyPotentialParams {
    /// Linear barrier restoring coefficient $a > 0$.
    pub a_linear: f64,
    /// Cubic non-linear confinement coefficient $b > 0$.
    pub b_cubic: f64,
    /// Microstructural noise diffusion intensity $D > 0$.
    pub diffusion_d: f64,
    /// Characteristic attempt frequency $\omega_0$ in Hz (default 100.0).
    pub attempt_freq_hz: f64,
    /// Critical tunneling rate threshold in $\text{s}^{-1}$ for maker evacuation (e.g. 15.0).
    pub critical_rate_hz: f64,
}

impl Default for SusyPotentialParams {
    fn default() -> Self {
        Self {
            a_linear: 2.5,
            b_cubic: 0.8,
            diffusion_d: 0.25,
            attempt_freq_hz: 100.0,
            critical_rate_hz: 15.0,
        }
    }
}

/// Instantaneous tunneling analysis verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SusyTunnelingVerdict {
    /// Instanton Kramers tunneling transition rate $\Gamma_{\text{tunnel}}$ in $\text{s}^{-1}$.
    pub tunneling_rate_hz: f64,
    /// Dimensionless tunneling barrier height $\Delta V / D = \frac{a^2}{4 b D}$.
    pub barrier_height: f64,
    /// Expected metastable lifetime $\tau_{\text{meta}} = 1 / \Gamma_{\text{tunnel}}$ in milliseconds.
    pub metastable_lifetime_ms: f64,
    /// Whether maker quote evacuation is triggered ($\Gamma_{\text{tunnel}} \ge \Gamma_{\text{crit}}$).
    pub should_evacuate_quote: bool,
    /// Recommended quote shift or defensive cancellation offset in basis points.
    pub recommended_shift_bps: f64,
}

/// SUSY QM Instanton Engine for real-time limit order book metastability detection.
pub struct SusyInstantonEngine;

impl SusyInstantonEngine {
    /// Evaluates Witten's superpotential $W(x) = \frac{-a x + b x^3}{2 \sqrt{D}}$ at queue imbalance $x \in [-1, 1]$.
    #[inline]
    pub fn evaluate_superpotential(x: f64, params: &SusyPotentialParams) -> f64 {
        let sqrt_d = params.diffusion_d.max(1e-6).sqrt();
        (-params.a_linear * x + params.b_cubic * x.powi(3)) / (2.0 * sqrt_d)
    }

    /// Evaluates the ground-state wave function $\psi_0(x) \propto \exp(-\int_0^x W(y) dy)$.
    #[inline]
    pub fn evaluate_ground_state_density(x: f64, params: &SusyPotentialParams) -> f64 {
        let sqrt_d = params.diffusion_d.max(1e-6).sqrt();
        let integral_w = (-0.5 * params.a_linear * x.powi(2) + 0.25 * params.b_cubic * x.powi(4))
            / (2.0 * sqrt_d);
        (-integral_w).clamp(-50.0, 50.0).exp()
    }

    /// Computes the closed-form instanton Kramers tunneling transition rate:
    /// $$\Gamma_{\text{tunnel}} = \omega_0 \frac{a \sqrt{2}}{\pi} \exp\left( -\frac{a^2}{4 b D} \right)$$
    #[inline]
    pub fn compute_tunneling_rate(params: &SusyPotentialParams) -> f64 {
        let a = params.a_linear.max(1e-4);
        let b = params.b_cubic.max(1e-4);
        let d = params.diffusion_d.max(1e-6);
        let omega = params.attempt_freq_hz.max(1.0);

        let prefactor = omega * (a * std::f64::consts::SQRT_2) / std::f64::consts::PI;
        let barrier = (a * a) / (4.0 * b * d);
        let exp_term = (-barrier).clamp(-60.0, 0.0).exp();

        prefactor * exp_term
    }

    /// Evaluates order book metastability under dynamic microstructural noise and OFI flow.
    pub fn evaluate_metastability(
        order_flow_imbalance: f64, // OFI in [-1, 1]
        depth_volatility: f64,     // local realized depth volatility sigma_D
        base_params: &SusyPotentialParams,
    ) -> SusyTunnelingVerdict {
        // Asymmetric potential tilt driven by directional OFI
        let mut dynamic_params = base_params.clone();
        dynamic_params.diffusion_d =
            (base_params.diffusion_d * (1.0 + depth_volatility.max(0.0))).max(1e-4);

        // Positive adverse OFI compresses the linear barrier restoring coefficient
        dynamic_params.a_linear =
            (base_params.a_linear * (1.0 - 0.40 * order_flow_imbalance.abs())).max(0.1);

        let gamma = Self::compute_tunneling_rate(&dynamic_params);
        let barrier = (dynamic_params.a_linear.powi(2))
            / (4.0 * dynamic_params.b_cubic * dynamic_params.diffusion_d);

        let lifetime_ms = if gamma > 1e-6 {
            1000.0 / gamma
        } else {
            1_000_000.0
        };

        let should_evacuate = gamma >= dynamic_params.critical_rate_hz;
        let shift_bps = if should_evacuate {
            // Scale quote shading by tunneling excess: 2 bps base + rate proportional
            2.0 + (gamma - dynamic_params.critical_rate_hz).min(50.0) * 0.25
        } else {
            0.0
        };

        SusyTunnelingVerdict {
            tunneling_rate_hz: gamma,
            barrier_height: barrier,
            metastable_lifetime_ms: lifetime_ms,
            should_evacuate_quote: should_evacuate,
            recommended_shift_bps: shift_bps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_susy_instanton_tunneling_rate_and_evacuation() {
        let params = SusyPotentialParams {
            a_linear: 2.0,
            b_cubic: 0.5,
            diffusion_d: 0.20,
            attempt_freq_hz: 100.0,
            critical_rate_hz: 15.0,
        };

        // 1. Baseline calm market: high barrier, low tunneling rate
        let rate_calm = SusyInstantonEngine::compute_tunneling_rate(&params);
        assert!(rate_calm < 5.0, "Calm rate should be low: {}", rate_calm);

        let verdict_calm = SusyInstantonEngine::evaluate_metastability(0.05, 0.02, &params);
        assert!(!verdict_calm.should_evacuate_quote);
        assert_eq!(verdict_calm.recommended_shift_bps, 0.0);
        assert!(verdict_calm.metastable_lifetime_ms > 200.0);

        // 2. High toxicity shock: extreme OFI + volatility burst -> barrier collapses
        let verdict_shock = SusyInstantonEngine::evaluate_metastability(0.95, 2.50, &params);
        assert!(
            verdict_shock.should_evacuate_quote,
            "Rate should breach threshold: {}",
            verdict_shock.tunneling_rate_hz
        );
        assert!(verdict_shock.tunneling_rate_hz >= 15.0);
        assert!(verdict_shock.recommended_shift_bps >= 2.0);
        assert!(verdict_shock.metastable_lifetime_ms < 65.0);
    }

    #[test]
    fn test_superpotential_and_ground_state_symmetry() {
        let params = SusyPotentialParams::default();
        let w_pos = SusyInstantonEngine::evaluate_superpotential(0.5, &params);
        let w_neg = SusyInstantonEngine::evaluate_superpotential(-0.5, &params);
        assert!(
            (w_pos + w_neg).abs() < 1e-10,
            "W(x) should be anti-symmetric"
        );

        let psi_pos = SusyInstantonEngine::evaluate_ground_state_density(0.5, &params);
        let psi_neg = SusyInstantonEngine::evaluate_ground_state_density(-0.5, &params);
        assert!(
            (psi_pos - psi_neg).abs() < 1e-10,
            "Ground state density should be symmetric"
        );
    }
}
