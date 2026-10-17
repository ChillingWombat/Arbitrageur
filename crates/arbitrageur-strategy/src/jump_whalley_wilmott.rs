//! High-Frequency Non-Linear Option Greeks Hedging under Jump-Diffusion & Quanto Smile Dislocations.
//!
//! Models dynamic delta-gamma rehedging of multi-leg option packages (4-leg box spreads, 8-leg jelly rolls,
//! reverse iron condors) under compound Poisson jump-diffusion. Formulates jump-augmented Whalley-Wilmott
//! rehedging bandwidth:
//! Delta S* = [(3 * c_perp * S * Gamma_pkg^2) / (2 * gamma * (sigma^2 + lambda_J * E[J^2]))]^(1/3)
//! which contracts the inaction band under jump hazard to preempt catastrophic unhedged gap risk.
//! Also provides the Quanto-adjusted Implied Volatility dislocation hurdle between CeFi (Deribit coin-margined)
//! and DeFi (Lyra/Aevo USDC-margined) option surfaces:
//! Delta IV*(K, T) = IV_DeFi - IV_CeFi - rho_{S,sigma} * sigma_S * sigma_vol * sqrt(T).

use serde::{Deserialize, Serialize};

/// Configuration parameters for jump-augmented Whalley-Wilmott delta rehedging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JumpWhalleyWilmottConfig {
    /// Absolute risk aversion coefficient gamma (default: 0.05).
    pub gamma_risk_aversion: f64,
    /// Proportional perpetual hedging transaction cost c_perp (e.g. 0.0003 for 3.0 bps).
    pub c_perp_fee_rate: f64,
    /// Poisson jump arrival intensity lambda_J (jumps per year, default: 12.0).
    pub lambda_jump_intensity: f64,
    /// Mean jump magnitude mu_J (default: -0.05, representing negative crash bias).
    pub jump_mean: f64,
    /// Jump magnitude standard deviation sigma_J (default: 0.15).
    pub jump_std: f64,
    /// Correlation between underlying spot price and volatility rho_{S,sigma} (default: -0.70).
    pub spot_vol_correlation: f64,
    /// Volatility of volatility parameter sigma_vol (default: 1.20).
    pub vol_of_vol: f64,
}

impl Default for JumpWhalleyWilmottConfig {
    fn default() -> Self {
        Self {
            gamma_risk_aversion: 0.05,
            c_perp_fee_rate: 0.0003,
            lambda_jump_intensity: 12.0,
            jump_mean: -0.05,
            jump_std: 0.15,
            spot_vol_correlation: -0.70,
            vol_of_vol: 1.20,
        }
    }
}

/// Evaluation decision for dynamic options delta rehedging.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JumpRehedgeDecision {
    /// True if the current delta imbalance breaches the jump-contracted inaction band.
    pub should_rehedge: bool,
    /// Half-width of the spot price rehedge trigger band Delta S*.
    pub spot_bandwidth: f64,
    /// Half-width of the delta inaction band Delta_star = 0.5 * |Gamma| * Delta S*.
    pub delta_inaction_band: f64,
    /// Current net portfolio delta imbalance across the options package and perpetual hedges.
    pub current_delta_imbalance: f64,
    /// Target trade quantity in perpetual contracts to restore delta neutrality.
    pub target_rehedge_qty: f64,
    /// Jump variance component lambda_J * E[J^2].
    pub jump_variance_drag: f64,
}

/// Quanto smile dislocation hurdle report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantoSmileHurdleReport {
    /// Observed raw IV spread (IV_DeFi - IV_CeFi).
    pub raw_iv_spread: f64,
    /// Structural quanto adjustment drift: rho_{S,sigma} * sigma_S * sigma_vol * sqrt(T).
    pub quanto_drift_adjustment: f64,
    /// Net arbitrage edge after correcting for coin-margined quanto drift and execution frictions.
    pub net_edge_vol_points: f64,
    /// True if net edge exceeds hurdle and warrants cross-venue options arbitrage.
    pub is_arbitrageable: bool,
}

/// Engine for jump-augmented Whalley-Wilmott rehedging and quanto smile hurdles.
#[derive(Debug, Clone)]
pub struct JumpWhalleyWilmottEngine {
    config: JumpWhalleyWilmottConfig,
}

impl JumpWhalleyWilmottEngine {
    pub fn new(config: JumpWhalleyWilmottConfig) -> Self {
        Self { config }
    }

    /// Computes the jump variance component: lambda_J * E[J^2] = lambda_J * (mu_J^2 + sigma_J^2).
    pub fn compute_jump_variance_drag(&self) -> f64 {
        let e_j2 = self.config.jump_mean * self.config.jump_mean
            + self.config.jump_std * self.config.jump_std;
        self.config.lambda_jump_intensity * e_j2
    }

    /// Computes the jump-augmented Whalley-Wilmott spot bandwidth Delta S*:
    /// Delta S* = [(3 * c_perp * S * Gamma_pkg^2) / (2 * gamma * (sigma^2 + lambda_J * E[J^2]))]^(1/3)
    pub fn compute_rehedge_bandwidth(
        &self,
        spot_price: f64,
        diffusion_vol: f64,
        package_gamma: f64,
    ) -> f64 {
        let abs_gamma = package_gamma.abs().max(1e-6);
        let gamma_sq = abs_gamma * abs_gamma;
        let sigma_sq = diffusion_vol * diffusion_vol;
        let jump_var = self.compute_jump_variance_drag();
        let total_variance = (sigma_sq + jump_var).max(1e-4);

        let numerator = 3.0 * self.config.c_perp_fee_rate * spot_price * gamma_sq;
        let denominator = 2.0 * self.config.gamma_risk_aversion * total_variance;

        (numerator / denominator).cbrt()
    }

    /// Evaluates whether a multi-leg option package requires immediate perpetual rehedging.
    pub fn evaluate_rehedge(
        &self,
        spot_price: f64,
        diffusion_vol: f64,
        package_gamma: f64,
        current_delta_imbalance: f64,
    ) -> JumpRehedgeDecision {
        let abs_gamma = package_gamma.abs().max(1e-6);
        let spot_bandwidth =
            self.compute_rehedge_bandwidth(spot_price, diffusion_vol, package_gamma);
        let delta_inaction_band = 0.5 * abs_gamma * spot_bandwidth;

        let should_rehedge = current_delta_imbalance.abs() > delta_inaction_band;
        let target_rehedge_qty = if should_rehedge {
            -current_delta_imbalance
        } else {
            0.0
        };

        JumpRehedgeDecision {
            should_rehedge,
            spot_bandwidth,
            delta_inaction_band,
            current_delta_imbalance,
            target_rehedge_qty,
            jump_variance_drag: self.compute_jump_variance_drag(),
        }
    }

    /// Computes the Quanto-adjusted IV dislocation hurdle between CeFi and DeFi surfaces:
    /// Delta IV*(K, T) = IV_DeFi - IV_CeFi - rho_{S,sigma} * sigma_S * sigma_vol * sqrt(T)
    pub fn evaluate_quanto_smile_hurdle(
        &self,
        iv_defi: f64,
        iv_cefi: f64,
        spot_vol: f64,
        time_to_expiry_years: f64,
        min_hurdle_vol_points: f64,
    ) -> QuantoSmileHurdleReport {
        let raw_iv_spread = iv_defi - iv_cefi;
        let sqrt_t = time_to_expiry_years.max(1e-4).sqrt();
        // Structural quanto drift adjustment
        let quanto_drift_adjustment =
            self.config.spot_vol_correlation * spot_vol * self.config.vol_of_vol * sqrt_t;

        let net_edge = raw_iv_spread - quanto_drift_adjustment;
        let is_arbitrageable = net_edge.abs() >= min_hurdle_vol_points;

        QuantoSmileHurdleReport {
            raw_iv_spread,
            quanto_drift_adjustment,
            net_edge_vol_points: net_edge,
            is_arbitrageable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jump_variance_drag_contracts_bandwidth() {
        let cfg_no_jump = JumpWhalleyWilmottConfig {
            lambda_jump_intensity: 0.0,
            ..Default::default()
        };
        let engine_no_jump = JumpWhalleyWilmottEngine::new(cfg_no_jump);

        let cfg_with_jump = JumpWhalleyWilmottConfig {
            lambda_jump_intensity: 24.0, // Active jump regime
            ..Default::default()
        };
        let engine_with_jump = JumpWhalleyWilmottEngine::new(cfg_with_jump);

        let spot = 65_000.0;
        let vol = 0.50;
        let gamma = 0.002;

        let bw_no_jump = engine_no_jump.compute_rehedge_bandwidth(spot, vol, gamma);
        let bw_with_jump = engine_with_jump.compute_rehedge_bandwidth(spot, vol, gamma);

        // Jump risk inflates total variance in denominator -> bandwidth contracts!
        assert!(bw_with_jump < bw_no_jump);
    }

    #[test]
    fn test_rehedge_trigger_on_band_breach() {
        let engine = JumpWhalleyWilmottEngine::new(JumpWhalleyWilmottConfig::default());
        let spot = 3_000.0;
        let vol = 0.60;
        let gamma = 0.005;

        // Bandwidth ~ a few dollars; inaction delta ~ 0.5 * gamma * bw
        let decision_quiet = engine.evaluate_rehedge(spot, vol, gamma, 0.0001);
        assert!(!decision_quiet.should_rehedge);
        assert_eq!(decision_quiet.target_rehedge_qty, 0.0);

        // Large delta excursion breaches band
        let decision_breach = engine.evaluate_rehedge(spot, vol, gamma, 0.15);
        assert!(decision_breach.should_rehedge);
        assert_eq!(decision_breach.target_rehedge_qty, -0.15);
    }

    #[test]
    fn test_quanto_smile_dislocation_hurdle() {
        let engine = JumpWhalleyWilmottEngine::new(JumpWhalleyWilmottConfig::default());
        // DeFi IV = 62.0%, CeFi IV = 58.0% -> raw spread = +4.0%
        let report = engine.evaluate_quanto_smile_hurdle(0.62, 0.58, 0.60, 30.0 / 365.0, 0.02);

        // Negative correlation rho = -0.70 makes quanto adjustment negative -> net edge is larger
        assert!(report.raw_iv_spread > 0.0);
        assert!(report.quanto_drift_adjustment < 0.0);
        assert!(report.net_edge_vol_points > report.raw_iv_spread);
        assert!(report.is_arbitrageable);
    }
}
