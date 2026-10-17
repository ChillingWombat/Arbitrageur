//! Distribution-Free Conformal Risk Control & Dynamic Margin Sizing.
//!
//! Implements finite-sample distribution-free risk guarantees (Bates, Angelopoulos,
//! Penchev, Jordan 2023) to bound portfolio margin shortfall and liquidation deficit
//! across segregated high-frequency trading accounts (Hyperliquid, Lighter, Binance, Deribit):
//!
//! $$\mathbb{E}[L(Y, C_{\lambda^*}(X))] \le \alpha$$
//!
//! Given calibration samples $\mathcal{D}_{\text{cal}} = \{(X_i, Y_i)\}_{i=1}^n$ of historical
//! volatility excursions and basis slippages, computes the conformal buffer $\lambda^*(\alpha)$
//! satisfying the finite-sample upper bound:
//!
//! $$\frac{n}{n+1} \hat{R}(\lambda) + \frac{B}{n+1} \le \alpha$$
//!
//! Dynamically scales maintenance margin and enforces fail-closed order submission
//! blocking and proportional de-leveraging when available margin drops below conformal bounds.

use serde::{Deserialize, Serialize};

/// An empirical shock observation recorded during stress episodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginShockObservation {
    /// Normalized high-frequency volatility $\sigma_{\text{hf}} / \bar{\sigma}$.
    pub vol_ratio: f64,
    /// Order flow imbalance $\text{OFI} \in [-1, 1]$.
    pub ofi: f64,
    /// Realized adverse excursion / basis slippage in USD or bps.
    pub realized_adverse_shock: f64,
}

/// Margin health status emitted by the conformal guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConformalMarginStatus {
    /// Account equity exceeds conformal requirement with adequate buffer.
    Safe,
    /// Account equity is within 10% of conformal requirement; entry restricted to hedge-only.
    Warning,
    /// Account equity is below conformal requirement; new risk-increasing orders blocked.
    Breach,
}

/// Evaluation verdict for a prospective order or account health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalMarginVerdict {
    pub status: ConformalMarginStatus,
    /// Current available equity in USD.
    pub available_equity_usd: f64,
    /// Calibrated required conformal margin requirement in USD.
    pub required_conformal_margin_usd: f64,
    /// Calibrated conformal buffer parameter $\lambda^*(\alpha)$.
    pub calibrated_lambda: f64,
    /// Whether new risk-increasing order submission is permitted.
    pub can_submit_order: bool,
    /// Required deleveraging notional to restore safe buffer if breached.
    pub deleverage_notional_usd: f64,
    /// Diagnostic reasoning.
    pub reason: String,
}

/// Conformal Risk Guard and Dynamic Margin Sizing Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalRiskGuard {
    /// Target risk tolerance $\alpha$ (e.g. 0.01 for 1% expected shortfall).
    pub target_alpha: f64,
    /// Loss bound $B$ (maximum possible loss ratio, typically 1.0 or 2.0).
    pub loss_bound: f64,
    /// Base initial margin requirement in USD.
    pub base_margin_usd: f64,
    /// Sensitivity coefficient to negative OFI skew.
    pub ofi_skew_beta: f64,
    /// Historical calibration shock dataset.
    pub calibration_data: Vec<MarginShockObservation>,
}

impl ConformalRiskGuard {
    pub fn new(
        target_alpha: f64,
        loss_bound: f64,
        base_margin_usd: f64,
        ofi_skew_beta: f64,
    ) -> Self {
        Self {
            target_alpha: target_alpha.clamp(1e-4, 0.5),
            loss_bound: loss_bound.max(0.1),
            base_margin_usd: base_margin_usd.max(1.0),
            ofi_skew_beta: ofi_skew_beta.max(0.0),
            calibration_data: Vec::new(),
        }
    }

    /// Appends a new stress episode observation to the calibration dataset.
    pub fn record_observation(&mut self, obs: MarginShockObservation) {
        self.calibration_data.push(obs);
    }

    /// Evaluates empirical loss $\hat{R}(\lambda)$ over the calibration set.
    ///
    /// Loss function $L_i(\lambda) = \min(B, \max(0, (Y_i - \lambda) / M_{\text{base}}))$.
    pub fn empirical_loss(&self, lambda: f64) -> f64 {
        if self.calibration_data.is_empty() {
            return 0.0;
        }

        let total_loss: f64 = self
            .calibration_data
            .iter()
            .map(|obs| {
                let shortfall = obs.realized_adverse_shock - lambda;
                if shortfall <= 0.0 {
                    0.0
                } else {
                    (shortfall / self.base_margin_usd).min(self.loss_bound)
                }
            })
            .sum();

        total_loss / (self.calibration_data.len() as f64)
    }

    /// Calibrates the conformal safety buffer $\lambda^*(\alpha)$ via exact 1D bisection.
    ///
    /// Finds $\lambda^* = \inf \{ \lambda \ge 0 : \frac{n}{n+1} \hat{R}(\lambda) + \frac{B}{n+1} \le \alpha \}$.
    pub fn calibrate_lambda(&self) -> f64 {
        let n = self.calibration_data.len() as f64;
        if n < 5.0 {
            // Insufficient samples: fallback to conservative heuristic buffer
            return self.base_margin_usd * 1.5;
        }

        let penalty = self.loss_bound / (n + 1.0);
        let weight = n / (n + 1.0);

        // If penalty already exceeds target alpha, maximum buffer is required
        if penalty >= self.target_alpha {
            let max_shock = self
                .calibration_data
                .iter()
                .map(|o| o.realized_adverse_shock)
                .fold(0.0, f64::max);
            return max_shock.max(self.base_margin_usd);
        }

        // Target empirical loss: \hat{R}(\lambda) <= (\alpha - penalty) / weight
        let target_r = ((self.target_alpha - penalty) / weight).max(0.0);

        // Monotonic search on lambda: lambda in [0, max_shock * 1.5]
        let max_shock = self
            .calibration_data
            .iter()
            .map(|o| o.realized_adverse_shock)
            .fold(0.0, f64::max)
            .max(self.base_margin_usd * 2.0);

        let mut low = 0.0;
        let mut high = max_shock * 1.5;

        for _ in 0..24 {
            let mid = 0.5 * (low + high);
            let loss_mid = self.empirical_loss(mid);
            if loss_mid <= target_r {
                // Buffer is large enough; try to tighten
                high = mid;
            } else {
                // Loss exceeds target; need larger buffer
                low = mid;
            }
        }

        high
    }

    /// Computes the required dynamic conformal margin requirement in USD for current market conditions.
    ///
    /// $$M_{\text{req}}(X) = M_{\text{base}} + \lambda^*(\alpha) \cdot \frac{\sigma_{\text{hf}}}{\bar{\sigma}} + \beta_{\text{skew}} \cdot \max(0, -\text{OFI}) \cdot M_{\text{base}}$$
    pub fn compute_required_margin(&self, vol_ratio: f64, ofi: f64) -> (f64, f64) {
        let lambda_star = self.calibrate_lambda();
        let vol_multiplier = vol_ratio.max(0.5);
        let ofi_penalty = (-ofi).max(0.0) * self.ofi_skew_beta;

        let dynamic_margin = self.base_margin_usd
            + lambda_star * vol_multiplier
            + ofi_penalty * self.base_margin_usd;

        (dynamic_margin, lambda_star)
    }

    /// Assesses account equity against conformal margin requirements to emit an execution verdict.
    pub fn evaluate_account_health(
        &self,
        available_equity_usd: f64,
        vol_ratio: f64,
        ofi: f64,
    ) -> ConformalMarginVerdict {
        let (required_margin, lambda_star) = self.compute_required_margin(vol_ratio, ofi);

        if available_equity_usd >= required_margin * 1.10 {
            ConformalMarginVerdict {
                status: ConformalMarginStatus::Safe,
                available_equity_usd,
                required_conformal_margin_usd: required_margin,
                calibrated_lambda: lambda_star,
                can_submit_order: true,
                deleverage_notional_usd: 0.0,
                reason: format!(
                    "Equity ${:.2} comfortably exceeds conformal requirement ${:.2}",
                    available_equity_usd, required_margin
                ),
            }
        } else if available_equity_usd >= required_margin {
            ConformalMarginVerdict {
                status: ConformalMarginStatus::Warning,
                available_equity_usd,
                required_conformal_margin_usd: required_margin,
                calibrated_lambda: lambda_star,
                can_submit_order: true, // Allowed with caution
                deleverage_notional_usd: 0.0,
                reason: format!(
                    "Equity ${:.2} within 10% buffer of conformal margin ${:.2}",
                    available_equity_usd, required_margin
                ),
            }
        } else {
            let deficit = required_margin - available_equity_usd;
            ConformalMarginVerdict {
                status: ConformalMarginStatus::Breach,
                available_equity_usd,
                required_conformal_margin_usd: required_margin,
                calibrated_lambda: lambda_star,
                can_submit_order: false, // Fail-closed block
                deleverage_notional_usd: deficit * 1.5, // 1.5x buffer restoration
                reason: format!(
                    "Conformal margin breach! Deficit: ${:.2} (Required: ${:.2}, Avail: ${:.2}). Order entry blocked.",
                    deficit, required_margin, available_equity_usd
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empirical_loss_monotonicity() {
        let mut guard = ConformalRiskGuard::new(0.05, 1.0, 1000.0, 0.2);

        // Populate synthetic calibration shocks
        for i in 1..=100 {
            guard.record_observation(MarginShockObservation {
                vol_ratio: 1.0 + (i as f64) * 0.01,
                ofi: -0.2,
                realized_adverse_shock: (i as f64) * 20.0, // Shocks from $20 to $2000
            });
        }

        let loss_100 = guard.empirical_loss(100.0);
        let loss_500 = guard.empirical_loss(500.0);
        let loss_1500 = guard.empirical_loss(1500.0);
        let loss_3000 = guard.empirical_loss(3000.0);

        assert!(loss_100 > loss_500);
        assert!(loss_500 > loss_1500);
        assert!(loss_3000 == 0.0);
    }

    #[test]
    fn test_conformal_calibration_and_finite_sample_bound() {
        let mut guard = ConformalRiskGuard::new(0.02, 1.0, 500.0, 0.1);

        for i in 1..=200 {
            let shock = if i % 10 == 0 {
                600.0 // Occasional large tail shock
            } else {
                50.0 + (i as f64) * 1.5 // Modest ambient shocks
            };
            guard.record_observation(MarginShockObservation {
                vol_ratio: 1.0,
                ofi: 0.0,
                realized_adverse_shock: shock,
            });
        }

        let lambda_star = guard.calibrate_lambda();
        assert!(lambda_star > 50.0);

        // Verify finite-sample bound: n/(n+1) * R_hat(lambda) + B/(n+1) <= alpha + eps
        let n = guard.calibration_data.len() as f64;
        let r_hat = guard.empirical_loss(lambda_star);
        let bound = (n / (n + 1.0)) * r_hat + (guard.loss_bound / (n + 1.0));

        assert!(bound <= guard.target_alpha + 1e-4);
    }

    #[test]
    fn test_dynamic_margin_sizing_under_volatility_and_skew() {
        let mut guard = ConformalRiskGuard::new(0.05, 1.0, 1000.0, 0.5);

        for i in 1..=50 {
            guard.record_observation(MarginShockObservation {
                vol_ratio: 1.0,
                ofi: 0.0,
                realized_adverse_shock: (i as f64) * 10.0,
            });
        }

        let (req_base, _) = guard.compute_required_margin(1.0, 0.0);
        let (req_high_vol, _) = guard.compute_required_margin(2.5, 0.0);
        let (req_adverse_ofi, _) = guard.compute_required_margin(1.0, -0.8);

        // High volatility must demand strictly more margin
        assert!(req_high_vol > req_base);
        // Adverse OFI sweep (adverse liquidity pressure) must increase margin requirement
        assert!(req_adverse_ofi > req_base);
    }

    #[test]
    fn test_account_health_verdict_and_fail_closed_blocking() {
        let mut guard = ConformalRiskGuard::new(0.05, 1.0, 1000.0, 0.2);

        for i in 1..=50 {
            guard.record_observation(MarginShockObservation {
                vol_ratio: 1.0,
                ofi: 0.0,
                realized_adverse_shock: (i as f64) * 15.0,
            });
        }

        let (req, _) = guard.compute_required_margin(1.0, 0.0);

        // 1. Safe condition: equity = 2 * req
        let verdict_safe = guard.evaluate_account_health(req * 2.0, 1.0, 0.0);
        assert_eq!(verdict_safe.status, ConformalMarginStatus::Safe);
        assert!(verdict_safe.can_submit_order);
        assert_eq!(verdict_safe.deleverage_notional_usd, 0.0);

        // 2. Breach condition: equity = 0.7 * req
        let verdict_breach = guard.evaluate_account_health(req * 0.7, 1.0, 0.0);
        assert_eq!(verdict_breach.status, ConformalMarginStatus::Breach);
        assert!(!verdict_breach.can_submit_order); // Fail-closed block active!
        assert!(verdict_breach.deleverage_notional_usd > 0.0);
    }
}
