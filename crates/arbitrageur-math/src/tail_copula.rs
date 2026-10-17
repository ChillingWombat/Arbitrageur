//! Multi-Asset Cross-Margin Portfolio Liquidation Distance & Extreme Value Stress Testing.
//!
//! Pairs Peaks-Over-Threshold (POT) Generalized Pareto Distribution (GPD) univariate tail risk metrics
//! ($\text{VaR}_\alpha, \text{ES}_\alpha$) with Archimedean Clayton and Gumbel joint tail copulas
//! to formulate closed-form dynamic margin buffer reservation boundaries $M^*_{\text{buffer}}$.

use serde::{Deserialize, Serialize};

/// Result of evaluating segregated account liquidation safety under joint tail stress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountTailSafetyReport {
    pub is_safe: bool,
    pub current_liquidation_distance: f64,
    pub required_margin_buffer: f64,
    pub expected_shortfall_999: f64,
    pub lower_tail_dependence: f64,
    pub joint_tail_penalty: f64,
    pub warning_message: Option<String>,
}

/// Extreme Value Theory (EVT) and Tail Copula Cross-Margin Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMarginTailEvtCopula {
    /// Venue maintenance margin ratio (e.g. 0.05 for 5%)
    pub maintenance_margin_ratio: f64,
    /// High quantile threshold $u$ for POT exceedance estimation
    pub threshold_u: f64,
    /// Tail shape parameter $\xi \in (0.0, 1.0)$
    pub tail_shape_xi: f64,
    /// Tail scale parameter $\sigma > 0$
    pub tail_scale_sigma: f64,
    /// Fraction of sample observations exceeding threshold: $N_u / N$
    pub sample_ratio_nu_n: f64,
    /// Clayton copula parameter $\theta > 0$ governing lower-tail crash dependence
    pub clayton_theta: f64,
    /// Gumbel copula parameter $\theta \ge 1.0$ governing upper-tail volatility spike dependence
    pub gumbel_theta: f64,
}

impl Default for CrossMarginTailEvtCopula {
    fn default() -> Self {
        Self {
            maintenance_margin_ratio: 0.05,
            threshold_u: 0.025,
            tail_shape_xi: 0.36,
            tail_scale_sigma: 0.015,
            sample_ratio_nu_n: 0.05,
            clayton_theta: 1.5,
            gumbel_theta: 1.8,
        }
    }
}

impl CrossMarginTailEvtCopula {
    pub fn new(
        mmr: f64,
        threshold_u: f64,
        tail_shape_xi: f64,
        tail_scale_sigma: f64,
        sample_ratio: f64,
        clayton_theta: f64,
        gumbel_theta: f64,
    ) -> Self {
        Self {
            maintenance_margin_ratio: mmr.clamp(0.01, 0.25),
            threshold_u: threshold_u.max(1e-4),
            tail_shape_xi: tail_shape_xi.clamp(0.05, 0.85),
            tail_scale_sigma: tail_scale_sigma.max(1e-4),
            sample_ratio_nu_n: sample_ratio.clamp(0.005, 0.25),
            clayton_theta: clayton_theta.max(0.1),
            gumbel_theta: gumbel_theta.max(1.0),
        }
    }

    /// Value-at-Risk under Generalized Pareto Distribution (GPD):
    ///
    /// $$\text{VaR}_\alpha(X) = u + \frac{\sigma}{\xi} \left[ \left( \frac{N}{N_u} (1 - \alpha) \right)^{-\xi} - 1 \right]$$
    pub fn var_gpd(&self, confidence_alpha: f64) -> f64 {
        let alpha = confidence_alpha.clamp(0.90, 0.9999);
        let prob_ratio = (1.0 - alpha) / self.sample_ratio_nu_n;
        self.threshold_u
            + (self.tail_scale_sigma / self.tail_shape_xi)
                * (prob_ratio.powf(-self.tail_shape_xi) - 1.0)
    }

    /// Expected Shortfall under GPD:
    ///
    /// $$\text{ES}_\alpha(X) = \frac{\text{VaR}_\alpha(X)}{1 - \xi} + \frac{\sigma - \xi u}{1 - \xi}$$
    pub fn expected_shortfall_gpd(&self, confidence_alpha: f64) -> f64 {
        let var = self.var_gpd(confidence_alpha);
        let xi = self.tail_shape_xi;
        (var / (1.0 - xi)) + (self.tail_scale_sigma - xi * self.threshold_u) / (1.0 - xi)
    }

    /// Clayton copula lower tail dependence coefficient:
    ///
    /// $$\lambda_L = \lim_{u \to 0^+} \frac{C(u, u)}{u} = 2^{-1/\theta} > 0$$
    pub fn clayton_lower_tail_dependence(&self) -> f64 {
        2.0f64.powf(-1.0 / self.clayton_theta)
    }

    /// Gumbel copula upper tail dependence coefficient:
    ///
    /// $$\lambda_U = 2 - 2^{1/\theta}$$
    pub fn gumbel_upper_tail_dependence(&self) -> f64 {
        2.0 - 2.0f64.powf(1.0 / self.gumbel_theta)
    }

    /// Dynamic Margin Buffer Reservation:
    ///
    /// $$M^*_{\text{buffer}} = \text{MMR} + \text{ES}_{0.999} \cdot \sqrt{\tau_{\text{unwind}}} + \lambda_L \cdot \sigma \cdot \sqrt{\tau_{\text{unwind}}}$$
    ///
    /// where unwind latency is converted into fractional hour units.
    pub fn compute_dynamic_margin_buffer(
        &self,
        confidence_alpha: f64,
        unwind_latency_sec: f64,
    ) -> f64 {
        let tau_hours = (unwind_latency_sec / 3600.0).max(1e-4);
        let tau_sqrt = tau_hours.sqrt();
        let es = self.expected_shortfall_gpd(confidence_alpha);
        let lambda_l = self.clayton_lower_tail_dependence();
        let tail_penalty = lambda_l * self.tail_scale_sigma * tau_sqrt;

        self.maintenance_margin_ratio + es * tau_sqrt + tail_penalty
    }

    /// Evaluates whether a segregated account maintains sufficient liquidation distance
    /// to safely survive worst-case market excursions during capital transfer latency $\tau_{\text{unwind}}$.
    pub fn evaluate_account_safety(
        &self,
        equity_usd: f64,
        notional_usd: f64,
        unwind_latency_sec: f64,
    ) -> AccountTailSafetyReport {
        if notional_usd <= 0.0 {
            return AccountTailSafetyReport {
                is_safe: true,
                current_liquidation_distance: 1.0,
                required_margin_buffer: self.maintenance_margin_ratio,
                expected_shortfall_999: 0.0,
                lower_tail_dependence: self.clayton_lower_tail_dependence(),
                joint_tail_penalty: 0.0,
                warning_message: None,
            };
        }

        let current_distance =
            (equity_usd - self.maintenance_margin_ratio * notional_usd) / notional_usd;
        let es_999 = self.expected_shortfall_gpd(0.999);
        let lambda_l = self.clayton_lower_tail_dependence();
        let tau_sqrt = (unwind_latency_sec / 3600.0).max(1e-4).sqrt();
        let tail_penalty = lambda_l * self.tail_scale_sigma * tau_sqrt;
        let required_buffer = self.maintenance_margin_ratio + es_999 * tau_sqrt + tail_penalty;

        let is_safe = current_distance >= required_buffer;
        let warning = if !is_safe {
            Some(format!(
                "Margin distance {:.2}% below required EVT copula buffer {:.2}% (ES_0.999={:.2}%, lambda_L={:.2}). Trigger defensive rebalancing or trim.",
                current_distance * 100.0,
                required_buffer * 100.0,
                es_999 * 100.0,
                lambda_l
            ))
        } else {
            None
        };

        AccountTailSafetyReport {
            is_safe,
            current_liquidation_distance: current_distance,
            required_margin_buffer: required_buffer,
            expected_shortfall_999: es_999,
            lower_tail_dependence: lambda_l,
            joint_tail_penalty: tail_penalty,
            warning_message: warning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tail_copula_var_and_es_gpd() {
        let engine = CrossMarginTailEvtCopula::default();

        let var_95 = engine.var_gpd(0.95);
        let var_99 = engine.var_gpd(0.99);
        let var_999 = engine.var_gpd(0.999);

        assert!(var_99 > var_95, "VaR must increase with confidence");
        assert!(var_999 > var_99, "VaR_0.999 must strictly exceed VaR_0.99");

        let es_999 = engine.expected_shortfall_gpd(0.999);
        assert!(
            es_999 > var_999,
            "Expected Shortfall must strictly exceed VaR"
        );
    }

    #[test]
    fn test_clayton_and_gumbel_tail_dependence() {
        let engine = CrossMarginTailEvtCopula::new(0.05, 0.025, 0.36, 0.015, 0.05, 2.0, 2.0);

        // Clayton theta = 2.0 -> lambda_L = 2^(-1/2) = 1/sqrt(2) ~ 0.7071
        let lambda_l = engine.clayton_lower_tail_dependence();
        assert!((lambda_l - (0.5f64).sqrt()).abs() < 1e-4);

        // Gumbel theta = 2.0 -> lambda_U = 2 - 2^(1/2) ~ 0.5858
        let lambda_u = engine.gumbel_upper_tail_dependence();
        assert!((lambda_u - (2.0 - 2.0f64.sqrt())).abs() < 1e-4);
    }

    #[test]
    fn test_account_tail_safety_evaluation() {
        let engine = CrossMarginTailEvtCopula::default();

        // 1. Ample equity: $2,000 on $10,000 notional (20% equity vs 5% MMR -> 15% distance)
        let safe_report = engine.evaluate_account_safety(2_000.0, 10_000.0, 30.0);
        assert!(safe_report.is_safe);
        assert!(safe_report.warning_message.is_none());

        // 2. Dangerous equity: $600 on $10,000 notional (6% equity vs 5% MMR -> 1% distance)
        // With 15-minute transfer latency (900s), required buffer will exceed 1%
        let unsafe_report = engine.evaluate_account_safety(600.0, 10_000.0, 900.0);
        assert!(!unsafe_report.is_safe);
        assert!(unsafe_report.warning_message.is_some());
    }
}
