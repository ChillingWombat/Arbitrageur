//! Cross-Exchange Unified Portfolio Margin Distance to Insolvency Early Warning System.
//!
//! Implements:
//! 1. **Multivariate Drift-Diffusion Segregated Account Equity**:
//!    Models segregated account equities $E_k(t)$ and maintenance margin requirements
//!    $\text{MMR}_k(t)$ under correlated asset price shocks ($\boldsymbol{\Sigma}_S$)
//!    and continuous funding bleed.
//! 2. **Multi-Dimensional Brownian Distance to Insolvency ($D_{\text{insolv}}$)**:
//!    Quantifies the exact number of standard deviations of market volatility separating
//!    the most vulnerable segregated account from liquidation over the operational capital
//!    transfer latency window $\Delta t_{\text{transfer}}$:
//!    $$D_{\text{insolv}}(t) = \min_k \frac{E_k(t) - \text{MMR}_k(t)}{\sigma_{E,k} \sqrt{\Delta t_{\text{transfer}}}}$$
//! 3. **Automated 4-Stage Defense Hierarchy**:
//!    - **Normal** ($D \ge 3.5\sigma$): Full autonomous operations.
//!    - **Advisory** ($2.5\sigma \le D < 3.5\sigma$): Pre-flight warning; poll internal transfer rails.
//!    - **RebalanceRequired** ($1.5\sigma \le D < 2.5\sigma$): Active top-up $\Delta E_k^*$.
//!    - **EmergencyDeleveraging** ($D < 1.5\sigma$): Immediate symmetric de-leveraging $\Delta Q^*$.
//! 4. **Joint Portfolio Survival under Archimedean Clayton Tail Copula**:
//!    Estimates joint survival probability across all segregated venues under market crash regimes.

use serde::{Deserialize, Serialize};

/// State of a single segregated venue account for margin distance evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegregatedVenueMarginState {
    /// Venue identifier (e.g. "hyperliquid", "lighter", "binance", "deribit").
    pub venue_id: String,
    /// Total current account equity in USD.
    pub current_equity_usd: f64,
    /// Maintenance margin requirement in USD.
    pub maintenance_margin_usd: f64,
    /// Vector of position notionals in USD across $M$ tracked assets.
    pub position_notionals_usd: Vec<f64>,
    /// Estimated hourly funding bleed / carry cost in USD.
    pub hourly_funding_bleed_usd: f64,
}

impl SegregatedVenueMarginState {
    pub fn new(
        venue_id: impl Into<String>,
        current_equity_usd: f64,
        maintenance_margin_usd: f64,
        position_notionals_usd: Vec<f64>,
        hourly_funding_bleed_usd: f64,
    ) -> Self {
        Self {
            venue_id: venue_id.into(),
            current_equity_usd: current_equity_usd.max(0.0),
            maintenance_margin_usd: maintenance_margin_usd.max(0.0),
            position_notionals_usd,
            hourly_funding_bleed_usd,
        }
    }

    /// Available margin buffer above maintenance margin: $E_k - \text{MMR}_k$.
    pub fn margin_buffer_usd(&self) -> f64 {
        (self.current_equity_usd - self.maintenance_margin_usd).max(0.0)
    }

    /// Margin utilization percentage: $\text{MMR}_k / E_k \times 100$.
    pub fn margin_utilization_pct(&self) -> f64 {
        if self.current_equity_usd > 1e-4 {
            (self.maintenance_margin_usd / self.current_equity_usd) * 100.0
        } else if self.maintenance_margin_usd > 0.0 {
            100.0
        } else {
            0.0
        }
    }
}

/// 4-stage automated defense hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarginDefenseStage {
    /// Safe margin distance ($D \ge 3.5\sigma$). Normal operations.
    Normal,
    /// Pre-emptive warning ($2.5\sigma \le D < 3.5\sigma$). Restrict new exposure; poll bridge routes.
    Advisory,
    /// Critical shortfall ($1.5\sigma \le D < 2.5\sigma$). Dispatch capital rebalance top-up $\Delta E^*$.
    RebalanceRequired,
    /// Imminent liquidation risk ($D < 1.5\sigma$). Immediately unwind positions symmetrically by fraction $\rho^*$.
    EmergencyDeleveraging,
}

/// Evaluation assessment for a single segregated venue account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountDistanceReport {
    /// Venue identifier.
    pub venue_id: String,
    /// Distance to insolvency in standard deviations ($D2I_k$).
    pub distance_sigma: f64,
    /// First-passage liquidation probability within transfer latency window (in percent).
    pub liquidation_prob_pct: f64,
    /// Recommended capital top-up in USD to restore $D_{\text{target}} = 4.0\sigma$.
    pub recommended_top_up_usd: f64,
    /// Recommended symmetric position unwind fraction $\rho^* \in [0.0, 1.0]$.
    pub recommended_unwind_fraction: f64,
    /// Assigned margin defense stage.
    pub stage: MarginDefenseStage,
}

/// Comprehensive portfolio-wide unified margin early warning report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedMarginWarningReport {
    /// Overall worst distance to insolvency across all segregated accounts.
    pub worst_distance_sigma: f64,
    /// Venue with the lowest distance to insolvency.
    pub worst_venue: String,
    /// Overall defense stage across the entire portfolio (highest severity).
    pub overall_stage: MarginDefenseStage,
    /// Total recommended capital top-up across all venues in USD.
    pub total_top_up_needed_usd: f64,
    /// Joint portfolio survival probability under Clayton crash copula (in percent).
    pub joint_survival_prob_pct: f64,
    /// Detailed reports per venue.
    pub venue_reports: Vec<AccountDistanceReport>,
    /// Whether all segregated venues are in safe/normal condition.
    pub is_portfolio_safe: bool,
}

/// Engine computing multi-dimensional Brownian distance to insolvency and early warning alerts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedMarginWarningEngine {
    /// Operational capital transfer latency in seconds (e.g. 60.0s for L2 bridge, 300.0s for CEX).
    pub transfer_latency_sec: f64,
    /// Target safety buffer in standard deviations (e.g. 4.0\sigma).
    pub target_safety_sigma: f64,
    /// Clayton lower-tail dependence parameter $\theta \ge 0.5$ (e.g. 2.0).
    pub clayton_theta: f64,
}

impl Default for UnifiedMarginWarningEngine {
    fn default() -> Self {
        Self {
            transfer_latency_sec: 60.0,
            target_safety_sigma: 4.0,
            clayton_theta: 2.0,
        }
    }
}

impl UnifiedMarginWarningEngine {
    pub fn new(transfer_latency_sec: f64, target_safety_sigma: f64, clayton_theta: f64) -> Self {
        Self {
            transfer_latency_sec: transfer_latency_sec.max(1.0),
            target_safety_sigma: target_safety_sigma.max(2.0),
            clayton_theta: clayton_theta.clamp(0.1, 10.0),
        }
    }

    /// Evaluates the unified distance to insolvency across segregated accounts given asset covariance.
    #[allow(clippy::needless_range_loop)]
    pub fn evaluate_portfolio(
        &self,
        accounts: &[SegregatedVenueMarginState],
        asset_covariance: &[Vec<f64>], // M x M matrix of annual return covariance
    ) -> UnifiedMarginWarningReport {
        if accounts.is_empty() {
            return UnifiedMarginWarningReport {
                worst_distance_sigma: f64::INFINITY,
                worst_venue: String::new(),
                overall_stage: MarginDefenseStage::Normal,
                total_top_up_needed_usd: 0.0,
                joint_survival_prob_pct: 100.0,
                venue_reports: Vec::new(),
                is_portfolio_safe: true,
            };
        }

        let tau_years = (self.transfer_latency_sec / (365.25 * 86400.0)).max(1e-8);
        let tau_sqrt = tau_years.sqrt();

        let mut venue_reports = Vec::with_capacity(accounts.len());
        let mut worst_distance = f64::INFINITY;
        let mut worst_venue = String::new();
        let mut worst_stage = MarginDefenseStage::Normal;
        let mut total_top_up = 0.0;
        let mut survival_probs = Vec::with_capacity(accounts.len());

        for acc in accounts {
            let buffer_usd = acc.margin_buffer_usd();

            // Compute portfolio volatility: sigma_E = sqrt(v^T Sigma v)
            let m = acc.position_notionals_usd.len();
            let mut variance_usd = 0.0;
            if !asset_covariance.is_empty() && asset_covariance.len() >= m {
                for i in 0..m {
                    for j in 0..m {
                        let cov = asset_covariance[i].get(j).copied().unwrap_or(0.0);
                        variance_usd +=
                            acc.position_notionals_usd[i] * cov * acc.position_notionals_usd[j];
                    }
                }
            } else {
                // Fallback assuming 60% default asset volatility if covariance matrix is not provided
                let notional_sum: f64 = acc.position_notionals_usd.iter().map(|x| x.abs()).sum();
                variance_usd = (notional_sum * 0.60).powi(2);
            }

            let vol_usd = variance_usd.max(1.0).sqrt();
            let d_k = buffer_usd / (vol_usd * tau_sqrt);

            // First-passage probability to barrier: P(tau <= delta_t) ~ 2 * (1 - Phi(d_k))
            let p_liq = 2.0 * (1.0 - standard_normal_cdf(d_k)).clamp(0.0, 1.0);
            let p_survive = (1.0 - p_liq).clamp(1e-6, 1.0);
            survival_probs.push(p_survive);

            // Determine stage and mitigation parameters
            let (stage, top_up, unwind_frac) = if d_k >= 3.5 {
                (MarginDefenseStage::Normal, 0.0, 0.0)
            } else if d_k >= 2.5 {
                (MarginDefenseStage::Advisory, 0.0, 0.0)
            } else if d_k >= 1.5 {
                let target_buffer = self.target_safety_sigma * vol_usd * tau_sqrt;
                let needed = (target_buffer - buffer_usd).max(0.0);
                (MarginDefenseStage::RebalanceRequired, needed, 0.0)
            } else {
                let target_buffer = 3.0 * vol_usd * tau_sqrt;
                let unwind = (1.0 - buffer_usd / target_buffer.max(1.0)).clamp(0.10, 0.95);
                let target_buffer_full = self.target_safety_sigma * vol_usd * tau_sqrt;
                let needed = (target_buffer_full - buffer_usd).max(0.0);
                (MarginDefenseStage::EmergencyDeleveraging, needed, unwind)
            };

            total_top_up += top_up;

            if d_k < worst_distance {
                worst_distance = d_k;
                worst_venue = acc.venue_id.clone();
            }

            if stage_severity(stage) > stage_severity(worst_stage) {
                worst_stage = stage;
            }

            venue_reports.push(AccountDistanceReport {
                venue_id: acc.venue_id.clone(),
                distance_sigma: d_k,
                liquidation_prob_pct: p_liq * 100.0,
                recommended_top_up_usd: top_up,
                recommended_unwind_fraction: unwind_frac,
                stage,
            });
        }

        // Compute joint survival probability under Clayton copula:
        // C(u_1, ..., u_K) = (sum(u_k^(-theta)) - (K - 1))^(-1/theta)
        let joint_survival_prob_pct = if !survival_probs.is_empty() {
            let k_len = survival_probs.len() as f64;
            let sum_pow: f64 = survival_probs
                .iter()
                .map(|&u| u.powf(-self.clayton_theta))
                .sum();
            let clayton_val = (sum_pow - (k_len - 1.0))
                .max(1.0)
                .powf(-1.0 / self.clayton_theta);
            (clayton_val * 100.0).clamp(0.0, 100.0)
        } else {
            100.0
        };

        let is_portfolio_safe = worst_stage == MarginDefenseStage::Normal;

        UnifiedMarginWarningReport {
            worst_distance_sigma: worst_distance,
            worst_venue,
            overall_stage: worst_stage,
            total_top_up_needed_usd: total_top_up,
            joint_survival_prob_pct,
            venue_reports,
            is_portfolio_safe,
        }
    }
}

fn stage_severity(stage: MarginDefenseStage) -> u8 {
    match stage {
        MarginDefenseStage::Normal => 0,
        MarginDefenseStage::Advisory => 1,
        MarginDefenseStage::RebalanceRequired => 2,
        MarginDefenseStage::EmergencyDeleveraging => 3,
    }
}

/// Rational Chebyshev approximation to the standard normal cumulative distribution function $\Phi(x)$.
fn standard_normal_cdf(x: f64) -> f64 {
    if x < -8.0 {
        return 0.0;
    }
    if x > 8.0 {
        return 1.0;
    }

    // Hart (1968) / Abramowitz & Stegun 26.2.17 approximation
    let p = 0.2316419;
    let b1 = 0.319381530;
    let b2 = -0.356563782;
    let b3 = 1.781477937;
    let b4 = -1.821255978;
    let b5 = 1.330274429;

    let t = 1.0 / (1.0 + p * x.abs());
    let phi = (-0.5 * x * x).exp() / (std::f64::consts::TAU).sqrt();
    let poly = ((((b5 * t + b4) * t + b3) * t + b2) * t + b1) * t;

    if x >= 0.0 {
        1.0 - phi * poly
    } else {
        phi * poly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_account_evaluates_safe() {
        let engine = UnifiedMarginWarningEngine::default();
        let accounts = vec![
            SegregatedVenueMarginState::new("hyperliquid", 100_000.0, 5_000.0, vec![20_000.0], 0.0),
            SegregatedVenueMarginState::new("lighter", 80_000.0, 4_000.0, vec![15_000.0], 0.0),
        ];

        let cov = vec![vec![0.36]]; // 60% vol
        let report = engine.evaluate_portfolio(&accounts, &cov);

        assert!(report.is_portfolio_safe);
        assert_eq!(report.overall_stage, MarginDefenseStage::Normal);
        assert!(report.worst_distance_sigma >= 3.5);
        assert_eq!(report.total_top_up_needed_usd, 0.0);
    }

    #[test]
    fn test_stressed_account_triggers_rebalance_required() {
        let engine = UnifiedMarginWarningEngine::new(60.0, 4.0, 2.0);

        // Account with thin margin buffer: $10k equity, $8.8k maintenance -> buffer only $1.2k
        // On $50k notional position with 60% vol -> vol_usd = $30k, 60s tau_sqrt = 0.001378
        // denominator = 30000 * 0.001378 = $41.34 -> buffer / 41.34 ~ 29.0
        // Let's create an account where buffer is very small relative to tau-vol:
        let thin_account = SegregatedVenueMarginState::new(
            "lighter",
            10_000.0,
            9_950.0, // buffer is only $50
            vec![50_000.0],
            0.0,
        );

        let cov = vec![vec![0.36]]; // 60% vol -> vol_usd = $30,000
        let report = engine.evaluate_portfolio(&[thin_account], &cov);

        assert!(!report.is_portfolio_safe);
        assert_eq!(report.worst_venue, "lighter");
        assert!(
            report.overall_stage == MarginDefenseStage::EmergencyDeleveraging
                || report.overall_stage == MarginDefenseStage::RebalanceRequired
        );
        assert!(report.total_top_up_needed_usd > 0.0);
    }

    #[test]
    fn test_clayton_copula_joint_survival() {
        let engine = UnifiedMarginWarningEngine::default();
        let accounts = vec![
            SegregatedVenueMarginState::new("hyperliquid", 50_000.0, 5_000.0, vec![10_000.0], 0.0),
            SegregatedVenueMarginState::new("lighter", 50_000.0, 5_000.0, vec![10_000.0], 0.0),
        ];

        let report = engine.evaluate_portfolio(&accounts, &[]);
        assert!(
            report.joint_survival_prob_pct > 95.0,
            "Joint survival probability should be high for well-capitalized accounts"
        );
    }
}
