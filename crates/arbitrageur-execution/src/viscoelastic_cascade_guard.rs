//! Viscoelastic LOB Hydrodynamics & Liquidation Cascade Circuit Breaker.
//!
//! Models limit order book depth deformation under concentrated forced liquidation
//! selling pressure $F_{\text{liq}}(x, t)$ as a continuum Maxwell-Kelvin-Voigt viscoelastic fluid:
//!
//! $$\sigma(x, t) + \tau_{\text{relax}} \frac{\partial \sigma(x, t)}{\partial t} = \eta \frac{\partial^2 u}{\partial x \partial t} + K_{\text{lob}} \frac{\partial u}{\partial x}$$
//!
//! Derives the exact Routh-Hurwitz critical runaway instability threshold from the cubic
//! dispersion relation:
//!
//! $$\beta_{\text{crit}} = \frac{K_{\text{lob}} (\rho_{\text{inertia}} + \gamma_{\text{friction}} \tau_{\text{relax}})}{\tau_{\text{relax}}}$$
//!
//! Computes the real-time Liquidation Cascade Index ($\text{LCI} = \beta_{\text{liq}} / \beta_{\text{crit}}$)
//! and critical slowing down indicators (autocorrelation time $\tau_{\text{corr}}$ and displacement
//! variance amplification), enforcing a three-tier fail-closed execution posture across
//! Hyperliquid, Lighter, Binance, and Deribit accounts.

use serde::{Deserialize, Serialize};

/// Three-tier operational posture under viscoelastic order book stress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CascadeAlertLevel {
    /// $\text{LCI} < 0.60$: Stable viscoelastic damping; normal passive/aggressive quoting.
    Green,
    /// $0.60 \le \text{LCI} < 0.85$: Elevated stress; widen maker spreads and retract deep bids.
    Amber,
    /// $\text{LCI} \ge 0.85$: Runaway liquidation cascade imminent; trip circuit breaker,
    /// evacuate all resting maker orders, and dispatch immediate synthetic perp delta lock.
    RedCircuitBreaker,
}

/// Evaluation verdict emitted by the Viscoelastic Cascade Guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViscoelasticCascadeVerdict {
    pub alert_level: CascadeAlertLevel,
    /// Critical liquidation feedback threshold $\beta_{\text{crit}}$.
    pub critical_beta_threshold: f64,
    /// Realized liquidation feedback intensity $\beta_{\text{liq}}$.
    pub realized_beta_liq: f64,
    /// Liquidation Cascade Index $\text{LCI} = \beta_{\text{liq}} / \beta_{\text{crit}}$.
    pub lci_ratio: f64,
    /// Critical slowing down autocorrelation relaxation time $\tau_{\text{corr}}$ in seconds.
    pub autocorrelation_time_sec: f64,
    /// Variance amplification factor relative to tranquil baseline.
    pub variance_amplification_ratio: f64,
    /// Recommended maker quote spread multiplier ($\ge 1.0$).
    pub spread_multiplier: f64,
    /// Whether resting maker orders must be immediately evacuated.
    pub evacuate_maker_quotes: bool,
    /// Required immediate synthetic perpetual delta lock quantity $Q_{\text{lock}}^* = -Q_{\text{exposed}}$.
    pub synthetic_perp_delta_lock_qty: f64,
}

/// Continuum Maxwell-Kelvin-Voigt Viscoelastic LOB Cascade Guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViscoelasticCascadeGuard {
    /// Maxwell stress relaxation timescale $\tau_{\text{relax}}$ in seconds (typically 0.15 - 0.60s).
    pub tau_relax_sec: f64,
    /// Dynamic shear viscosity $\eta$ (dissipation from cancel/replace latency).
    pub eta_viscosity: f64,
    /// Elastic bulk modulus $K_{\text{lob}}$ (order replenishment stiffness).
    pub k_lob_modulus: f64,
    /// Effective liquidity inertia $\rho_{\text{inertia}}$.
    pub rho_inertia: f64,
    /// Frictional damping coefficient $\gamma_{\text{friction}}$.
    pub gamma_friction: f64,
    /// Amber alert LCI threshold (default 0.60).
    pub amber_lci_threshold: f64,
    /// Red circuit breaker LCI threshold (default 0.85).
    pub red_lci_threshold: f64,
}

impl Default for ViscoelasticCascadeGuard {
    fn default() -> Self {
        Self {
            tau_relax_sec: 0.35,
            eta_viscosity: 1.20,
            k_lob_modulus: 50.0,
            rho_inertia: 1.0,
            gamma_friction: 2.5,
            amber_lci_threshold: 0.60,
            red_lci_threshold: 0.85,
        }
    }
}

impl ViscoelasticCascadeGuard {
    pub fn new(
        tau_relax_sec: f64,
        eta_viscosity: f64,
        k_lob_modulus: f64,
        rho_inertia: f64,
        gamma_friction: f64,
    ) -> Self {
        Self {
            tau_relax_sec: tau_relax_sec.max(0.01),
            eta_viscosity: eta_viscosity.max(0.01),
            k_lob_modulus: k_lob_modulus.max(0.1),
            rho_inertia: rho_inertia.max(0.01),
            gamma_friction: gamma_friction.max(0.01),
            amber_lci_threshold: 0.60,
            red_lci_threshold: 0.85,
        }
    }

    /// Computes the Routh-Hurwitz critical liquidation feedback threshold:
    ///
    /// $$\beta_{\text{crit}} = \frac{K_{\text{lob}} (\rho + \gamma \tau_{\text{relax}})}{\tau_{\text{relax}}}$$
    #[inline(always)]
    pub fn critical_feedback_threshold(&self) -> f64 {
        self.k_lob_modulus * (self.rho_inertia + self.gamma_friction * self.tau_relax_sec)
            / self.tau_relax_sec
    }

    /// Evaluates current forced liquidation velocity and unhedged inventory exposure.
    pub fn evaluate_cascade_risk(
        &self,
        realized_beta_liq: f64,
        unhedged_inventory_qty: f64,
    ) -> ViscoelasticCascadeVerdict {
        let beta_crit = self.critical_feedback_threshold();
        let beta_liq = realized_beta_liq.max(0.0);
        let lci = beta_liq / beta_crit.max(1e-6);

        // Critical slowing down: \tau_corr ~ 1 / (\beta_crit - \beta_liq)
        let stability_margin = (beta_crit - beta_liq).max(beta_crit * 0.02);
        let tau_corr = beta_crit / stability_margin; // normalized scale in seconds * tau_0
        let variance_amp = (beta_crit / stability_margin).clamp(1.0, 50.0);

        if lci >= self.red_lci_threshold {
            ViscoelasticCascadeVerdict {
                alert_level: CascadeAlertLevel::RedCircuitBreaker,
                critical_beta_threshold: beta_crit,
                realized_beta_liq: beta_liq,
                lci_ratio: lci,
                autocorrelation_time_sec: tau_corr * self.tau_relax_sec,
                variance_amplification_ratio: variance_amp,
                spread_multiplier: 5.0,
                evacuate_maker_quotes: true,
                synthetic_perp_delta_lock_qty: -unhedged_inventory_qty,
            }
        } else if lci >= self.amber_lci_threshold {
            ViscoelasticCascadeVerdict {
                alert_level: CascadeAlertLevel::Amber,
                critical_beta_threshold: beta_crit,
                realized_beta_liq: beta_liq,
                lci_ratio: lci,
                autocorrelation_time_sec: tau_corr * self.tau_relax_sec,
                variance_amplification_ratio: variance_amp,
                spread_multiplier: 1.0 + 2.0 * lci,
                evacuate_maker_quotes: false,
                synthetic_perp_delta_lock_qty: 0.0,
            }
        } else {
            ViscoelasticCascadeVerdict {
                alert_level: CascadeAlertLevel::Green,
                critical_beta_threshold: beta_crit,
                realized_beta_liq: beta_liq,
                lci_ratio: lci,
                autocorrelation_time_sec: tau_corr * self.tau_relax_sec,
                variance_amplification_ratio: variance_amp,
                spread_multiplier: 1.0,
                evacuate_maker_quotes: false,
                synthetic_perp_delta_lock_qty: 0.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routh_hurwitz_critical_threshold_calculation() {
        let guard = ViscoelasticCascadeGuard::new(0.50, 1.0, 40.0, 1.0, 2.0);
        // beta_crit = 40.0 * (1.0 + 2.0 * 0.50) / 0.50 = 40.0 * 2.0 / 0.50 = 160.0
        let beta_crit = guard.critical_feedback_threshold();
        assert!((beta_crit - 160.0).abs() < 1e-6);
    }

    #[test]
    fn test_three_tier_cascade_transitions_and_delta_lock() {
        let guard = ViscoelasticCascadeGuard::new(0.50, 1.0, 40.0, 1.0, 2.0); // beta_crit = 160.0

        // 1. Tranquil market (LCI = 40 / 160 = 0.25 < 0.60) -> Green
        let v_green = guard.evaluate_cascade_risk(40.0, 2.5);
        assert_eq!(v_green.alert_level, CascadeAlertLevel::Green);
        assert!(!v_green.evacuate_maker_quotes);
        assert_eq!(v_green.spread_multiplier, 1.0);
        assert_eq!(v_green.synthetic_perp_delta_lock_qty, 0.0);

        // 2. Stressed market (LCI = 112 / 160 = 0.70 in [0.60, 0.85)) -> Amber
        let v_amber = guard.evaluate_cascade_risk(112.0, 2.5);
        assert_eq!(v_amber.alert_level, CascadeAlertLevel::Amber);
        assert!(!v_amber.evacuate_maker_quotes);
        assert!((v_amber.spread_multiplier - 2.4).abs() < 1e-6);
        assert!(v_amber.variance_amplification_ratio > v_green.variance_amplification_ratio);

        // 3. Runaway liquidation cascade (LCI = 144 / 160 = 0.90 >= 0.85) -> RedCircuitBreaker
        let v_red = guard.evaluate_cascade_risk(144.0, 2.5);
        assert_eq!(v_red.alert_level, CascadeAlertLevel::RedCircuitBreaker);
        assert!(v_red.evacuate_maker_quotes);
        assert_eq!(v_red.spread_multiplier, 5.0);
        assert_eq!(v_red.synthetic_perp_delta_lock_qty, -2.5);
        assert!(v_red.autocorrelation_time_sec > v_amber.autocorrelation_time_sec);
    }
}
