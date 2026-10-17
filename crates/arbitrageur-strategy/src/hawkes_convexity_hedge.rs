//! Hawkes Liquidation Cascade & Synthetic Option Convexity Hedging.
//!
//! Formulates mutually exciting Hawkes liquidation cascades (criticality rho >= 0.90),
//! Carr-Madan deep OTM options synthetic convexity hedge ratio h_cvx*,
//! and dynamic segregated margin haircut multipliers protecting against -300% APY funding shocks.

use serde::{Deserialize, Serialize};

/// Recommendation for synthetic convexity hedging under Hawkes liquidation cascades.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvexityHedgeRecommendation {
    pub is_critical_cascade: bool,
    pub hawkes_spectral_radius: f64,
    pub recommended_put_qty: f64,
    pub put_strike_usd: f64,
    pub dynamic_margin_haircut_mult: f64,
}

/// Evaluator for Hawkes-Excited Liquidation Cascades and Synthetic Convexity Hedging.
pub struct HawkesConvexityEngine;

impl HawkesConvexityEngine {
    /// Evaluates Carr-Madan synthetic convexity hedge ratio and dynamic margin multiplier.
    pub fn evaluate_convexity_hedge(
        spot_price: f64,
        perp_position_qty: f64,
        hawkes_spectral_radius: f64,
        lambda_liq: f64,
        jump_mean: f64,
        basis_diffusion_sigma: f64,
        current_funding_rate_bps: f64,
    ) -> ConvexityHedgeRecommendation {
        let is_critical = hawkes_spectral_radius >= 0.90;
        let s0 = spot_price.max(1e-4);
        let q_perp = perp_position_qty.abs();

        // Carr-Madan synthetic convexity hedge ratio
        let jump_var = lambda_liq * jump_mean.powi(2);
        let diff_var = basis_diffusion_sigma.powi(2).max(1e-6);
        let h_cvx = (q_perp / s0) * (1.0 + jump_var / diff_var);

        let put_strike = s0 * 0.85; // Deep OTM 15% put wing

        // Dynamic margin haircut multiplier
        let fund_factor = (current_funding_rate_bps.abs() / 5.0).max(1.0);
        let liq_factor = if is_critical { 1.50 } else { 1.0 };
        let haircut_mult = fund_factor * liq_factor;

        ConvexityHedgeRecommendation {
            is_critical_cascade: is_critical,
            hawkes_spectral_radius,
            recommended_put_qty: h_cvx,
            put_strike_usd: put_strike,
            dynamic_margin_haircut_mult: haircut_mult,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hawkes_convexity_hedge_and_margin_haircut() {
        let rec = HawkesConvexityEngine::evaluate_convexity_hedge(
            3000.0, 10.0, 0.94, 2.5, -0.05, 0.02, -50.0,
        );
        assert!(rec.is_critical_cascade);
        assert!(rec.recommended_put_qty > 0.0);
        assert_eq!(rec.put_strike_usd, 2550.0); // 0.85 * 3000
        assert!(rec.dynamic_margin_haircut_mult > 1.0);
    }
}
