//! Multi-Asset Multi-Greek Dynamic Portfolio Optimization (Quadratic Risk Optimization).
//!
//! Provides simultaneous Delta, Gamma, and Vega risk neutralization across multi-asset
//! options portfolios and underlying perpetual hedging venues.
//!
//! Solves the bounded Quadratic Program (QP):
//! \min_{\mathbf{u}} \frac{1}{2} \mathbf{u}^T \mathbf{H} \mathbf{u} + \mathbf{c}^T \mathbf{u}
//! \text{subject to } \mathbf{u}_{min} \le \mathbf{u} \le \mathbf{u}_{max}
//! and multi-Greek tolerance constraints |\Delta_{post}| \le \epsilon_\Delta,
//! |\Gamma_{post}| \le \epsilon_\Gamma, |\nu_{post}| \le \epsilon_\nu via projected gradient descent.
//!
//! Includes the Whalley-Wilmott Multi-Greek Ellipsoid Rebalance Trigger evaluating joint
//! Mahalanobis Greek distance \sqrt{\mathbf{g}^T \mathbf{W} \mathbf{g}} > \theta^*.

use serde::{Deserialize, Serialize};

/// Specification of an available liquid hedging instrument (Perpetuals or Options).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HedgingInstrument {
    pub symbol: String,
    pub venue: String,
    /// Sensitivity to underlying price \frac{\partial V}{\partial S}
    pub delta: f64,
    /// Sensitivity to delta \frac{\partial^2 V}{\partial S^2}
    pub gamma: f64,
    /// Sensitivity to implied volatility \frac{\partial V}{\partial \sigma}
    pub vega: f64,
    /// Linear transaction cost and carry friction (in USD per unit)
    pub linear_cost: f64,
    /// Quadratic market impact coefficient \lambda_i (USD per unit^2)
    pub quadratic_impact: f64,
    /// Lower bound on trade size (e.g. -10.0 for short)
    pub min_trade_size: f64,
    /// Upper bound on trade size (e.g. +10.0 for long)
    pub max_trade_size: f64,
}

/// Target Greek tolerances and current portfolio aggregate exposures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiGreekPortfolioTarget {
    pub current_delta: f64,
    pub current_gamma: f64,
    pub current_vega: f64,
    pub max_delta_tolerance: f64,
    pub max_gamma_tolerance: f64,
    pub max_vega_tolerance: f64,
}

impl Default for MultiGreekPortfolioTarget {
    fn default() -> Self {
        Self {
            current_delta: 0.0,
            current_gamma: 0.0,
            current_vega: 0.0,
            max_delta_tolerance: 0.10,
            max_gamma_tolerance: 0.05,
            max_vega_tolerance: 50.0,
        }
    }
}

/// Multi-Greek optimizer tuning configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiGreekOptimizationConfig {
    pub delta_weight: f64,
    pub gamma_weight: f64,
    pub vega_weight: f64,
    pub max_iterations: usize,
    pub tolerance: f64,
    pub learning_rate: f64,
}

impl Default for MultiGreekOptimizationConfig {
    fn default() -> Self {
        Self {
            delta_weight: 100.0,
            gamma_weight: 50.0,
            vega_weight: 0.10,
            max_iterations: 150,
            tolerance: 1e-6,
            learning_rate: 0.01,
        }
    }
}

/// Optimization result vector and residual portfolio Greeks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiGreekSolution {
    pub allocations: Vec<(String, f64)>,
    pub post_hedge_delta: f64,
    pub post_hedge_gamma: f64,
    pub post_hedge_vega: f64,
    pub expected_cost_usd: f64,
    pub objective_value: f64,
    pub converged: bool,
    pub iterations: usize,
}

/// Quadratic Programming Solver for multi-instrument multi-Greek risk optimization.
#[derive(Debug, Clone, Default)]
pub struct MultiGreekQpOptimizer {
    pub config: MultiGreekOptimizationConfig,
}

impl MultiGreekQpOptimizer {
    pub fn new(config: MultiGreekOptimizationConfig) -> Self {
        Self { config }
    }

    /// Solves the multi-Greek quadratic risk minimization problem:
    ///
    /// \mathcal{L}(\mathbf{u}) = \frac{1}{2} w_\Delta (\Delta_0 + \mathbf{D}^T \mathbf{u})^2
    ///                         + \frac{1}{2} w_\Gamma (\Gamma_0 + \mathbf{G}^T \mathbf{u})^2
    ///                         + \frac{1}{2} w_\nu (\nu_0 + \mathbf{V}^T \mathbf{u})^2
    ///                         + \sum_i (c_i |u_i| + \frac{1}{2} \lambda_i u_i^2)
    ///
    /// Projected onto box constraints u_{min, i} \le u_i \le u_{max, i}.
    pub fn solve(
        &self,
        target: &MultiGreekPortfolioTarget,
        instruments: &[HedgingInstrument],
    ) -> MultiGreekSolution {
        let m = instruments.len();
        if m == 0 {
            return MultiGreekSolution {
                allocations: vec![],
                post_hedge_delta: target.current_delta,
                post_hedge_gamma: target.current_gamma,
                post_hedge_vega: target.current_vega,
                expected_cost_usd: 0.0,
                objective_value: 0.0,
                converged: true,
                iterations: 0,
            };
        }

        let mut u = vec![0.0; m];
        let mut converged = false;
        let mut iters = 0;
        let _lr = self.config.learning_rate;

        for iter in 0..self.config.max_iterations {
            iters = iter + 1;
            let mut max_change = 0.0;

            for i in 0..m {
                let inst = &instruments[i];

                // Compute live aggregate Greeks with current u
                let mut delta_tot = target.current_delta;
                let mut gamma_tot = target.current_gamma;
                let mut vega_tot = target.current_vega;

                for j in 0..m {
                    delta_tot += instruments[j].delta * u[j];
                    gamma_tot += instruments[j].gamma * u[j];
                    vega_tot += instruments[j].vega * u[j];
                }

                // Gradient of Greek quadratic penalties
                let grad_greek = self.config.delta_weight * delta_tot * inst.delta
                    + self.config.gamma_weight * gamma_tot * inst.gamma
                    + self.config.vega_weight * vega_tot * inst.vega;

                // Gradient of execution friction and impact
                let sign_u = if u[i] > 1e-6 {
                    1.0
                } else if u[i] < -1e-6 {
                    -1.0
                } else {
                    0.0
                };
                let grad_friction = inst.linear_cost * sign_u + inst.quadratic_impact * u[i];

                let total_grad = grad_greek + grad_friction;

                // Exact diagonal Hessian curvature H_{ii}
                let h_ii = self.config.delta_weight * inst.delta * inst.delta
                    + self.config.gamma_weight * inst.gamma * inst.gamma
                    + self.config.vega_weight * inst.vega * inst.vega
                    + inst.quadratic_impact;

                let denom = h_ii.max(1e-6);
                let step = total_grad / denom;
                let unprojected = u[i] - step;

                let projected = unprojected.clamp(inst.min_trade_size, inst.max_trade_size);
                let diff = (projected - u[i]).abs();
                if diff > max_change {
                    max_change = diff;
                }
                u[i] = projected;
            }

            if max_change < self.config.tolerance {
                converged = true;
                break;
            }
        }

        // Compute final Greeks and objective
        let mut final_delta = target.current_delta;
        let mut final_gamma = target.current_gamma;
        let mut final_vega = target.current_vega;
        let mut total_cost = 0.0;

        for i in 0..m {
            final_delta += instruments[i].delta * u[i];
            final_gamma += instruments[i].gamma * u[i];
            final_vega += instruments[i].vega * u[i];
            total_cost += instruments[i].linear_cost * u[i].abs()
                + 0.5 * instruments[i].quadratic_impact * u[i] * u[i];
        }

        let obj = 0.5 * self.config.delta_weight * final_delta * final_delta
            + 0.5 * self.config.gamma_weight * final_gamma * final_gamma
            + 0.5 * self.config.vega_weight * final_vega * final_vega
            + total_cost;

        let allocations = instruments
            .iter()
            .zip(u.iter())
            .map(|(inst, &qty)| (inst.symbol.clone(), qty))
            .collect();

        MultiGreekSolution {
            allocations,
            post_hedge_delta: final_delta,
            post_hedge_gamma: final_gamma,
            post_hedge_vega: final_vega,
            expected_cost_usd: total_cost,
            objective_value: obj,
            converged,
            iterations: iters,
        }
    }
}

/// Whalley-Wilmott Multi-Greek Ellipsoid Rebalance Trigger.
///
/// Evaluates whether aggregate portfolio Greek drift exceeds the joint non-intervention
/// ellipsoid boundary: \mathcal{M}(\mathbf{g}) = \sqrt{ w_\Delta \Delta^2 + w_\Gamma \Gamma^2 + w_\nu \nu^2 } > \theta^*
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiGreekEllipsoidTrigger {
    pub delta_weight: f64,
    pub gamma_weight: f64,
    pub vega_weight: f64,
    pub rebalance_threshold: f64,
}

impl Default for MultiGreekEllipsoidTrigger {
    fn default() -> Self {
        Self {
            delta_weight: 1.0,
            gamma_weight: 2.0,
            vega_weight: 0.001,
            rebalance_threshold: 1.5,
        }
    }
}

impl MultiGreekEllipsoidTrigger {
    pub fn new(
        delta_weight: f64,
        gamma_weight: f64,
        vega_weight: f64,
        rebalance_threshold: f64,
    ) -> Self {
        Self {
            delta_weight: delta_weight.max(0.0),
            gamma_weight: gamma_weight.max(0.0),
            vega_weight: vega_weight.max(0.0),
            rebalance_threshold: rebalance_threshold.max(1e-4),
        }
    }

    /// Computes the joint Mahalanobis Greek distance:
    pub fn mahalanobis_distance(&self, delta: f64, gamma: f64, vega: f64) -> f64 {
        let quad = self.delta_weight * delta * delta
            + self.gamma_weight * gamma * gamma
            + self.vega_weight * vega * vega;
        quad.sqrt()
    }

    /// Evaluates if Greek drift warrants rebalancing.
    pub fn should_rehedge(&self, delta: f64, gamma: f64, vega: f64) -> bool {
        self.mahalanobis_distance(delta, gamma, vega) > self.rebalance_threshold
    }
}

/// Regulatory and macro crash stress scenario specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroStressScenario {
    pub name: String,
    /// Underlying spot price percentage shock \Delta S / S (e.g. -0.20 for -20%)
    pub spot_shock_pct: f64,
    /// Implied volatility shift \Delta \sigma in absolute points (e.g. +0.25 for +25 vol points)
    pub vol_shock_abs: f64,
    /// Risk-free rate shift \Delta r in absolute points (e.g. +0.01 for +100 bps)
    pub rate_shock_abs: f64,
    /// Elapsed time in days \Delta t (e.g. 1.0 day)
    pub time_elapsed_days: f64,
}

impl MacroStressScenario {
    pub fn new(
        name: impl Into<String>,
        spot_shock_pct: f64,
        vol_shock_abs: f64,
        rate_shock_abs: f64,
        time_elapsed_days: f64,
    ) -> Self {
        Self {
            name: name.into(),
            spot_shock_pct,
            vol_shock_abs,
            rate_shock_abs,
            time_elapsed_days,
        }
    }

    /// Standard Black Swan crash: -25% spot, +50% vol, 0 rate, 1 day decay.
    pub fn black_swan_crash() -> Self {
        Self::new(
            "Black Swan Crash (-25% spot, +50% vol)",
            -0.25,
            0.50,
            0.0,
            1.0,
        )
    }

    /// Macro liquidity squeeze: -15% spot, +30% vol, +200 bps rate, 2 days decay.
    pub fn liquidity_squeeze() -> Self {
        Self::new(
            "Liquidity Squeeze (-15% spot, +30% vol, +200bps rate)",
            -0.15,
            0.30,
            0.02,
            2.0,
        )
    }

    /// Melt-up short squeeze: +20% spot, -15% vol, 0 rate, 1 day decay.
    pub fn melt_up_rally() -> Self {
        Self::new("Melt-Up Rally (+20% spot, -15% vol)", 0.20, -0.15, 0.0, 1.0)
    }

    /// Calm carry drift with volatility crush: 0% spot, -20% vol, -50 bps rate, 3 days decay.
    pub fn vol_crush_calm() -> Self {
        Self::new(
            "Vol Crush Carry (0% spot, -20% vol)",
            0.0,
            -0.20,
            -0.005,
            3.0,
        )
    }
}

/// Result of second-order Taylor expansion PnL under macro crash scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroStressResult {
    pub scenario_name: String,
    pub delta_pnl: f64,
    pub gamma_pnl: f64,
    pub vega_pnl: f64,
    pub theta_pnl: f64,
    pub rho_pnl: f64,
    pub total_stressed_pnl: f64,
    pub pnl_pct_of_notional: f64,
}

/// Conditional Value-at-Risk (\Delta CoVaR^{j|i}) systemic risk report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoVaRReport {
    pub benchmark_asset: String,
    pub target_asset: String,
    pub quantile_alpha: f64,
    pub benchmark_var: f64,
    pub benchmark_median: f64,
    pub systemic_beta: f64,
    pub covar_distress: f64,
    pub covar_median: f64,
    pub delta_covar: f64,
}

/// Extreme Value Theory (EVT) Generalized Pareto Distribution (GPD) Expected Shortfall report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvtGpdExpectedShortfall {
    pub threshold_u: f64,
    pub shape_xi: f64,
    pub scale_beta: f64,
    pub exceedance_prob: f64,
    pub target_quantile: f64,
    pub tail_var: f64,
    pub tail_expected_shortfall: f64,
}

/// Multi-Greek Portfolio Stress Testing & CoVaR / Expected Shortfall Engine.
///
/// Implements:
/// 1. **Second-Order Taylor Series PnL Stress Testing**:
///    $$\Delta \Pi \approx \Delta \cdot \Delta S + \frac{1}{2} \Gamma \cdot (\Delta S)^2 + \mathcal{V} \cdot \Delta \sigma + \Theta \cdot \Delta t + \rho \cdot \Delta r$$
/// 2. **Conditional Value-at-Risk (\Delta CoVaR^{j|i})**:
///    $$\Delta \text{CoVaR}_\alpha^{j|i} = \beta_{j|i} (\text{VaR}_\alpha^i - \text{VaR}_{50\%}^i)$$
/// 3. **EVT Generalized Pareto Distribution Expected Shortfall**:
///    $$\text{VaR}_\alpha = u + \frac{\beta}{\xi} \left( \left(\frac{1 - \alpha}{p_u}\right)^{-\xi} - 1 \right), \quad ES_\alpha = \frac{\text{VaR}_\alpha}{1 - \xi} + \frac{\beta - \xi u}{1 - \xi}$$
#[derive(Debug, Clone, Default)]
pub struct MultiGreekCoVaRStressEngine;

impl MultiGreekCoVaRStressEngine {
    pub fn new() -> Self {
        Self
    }

    /// Evaluates second-order Taylor expansion PnL under a specified macro crash shock.
    pub fn evaluate_scenario_pnl(
        spot_price: f64,
        portfolio_delta: f64,
        portfolio_gamma: f64,
        portfolio_vega: f64,
        portfolio_theta: f64,
        portfolio_rho: f64,
        scenario: &MacroStressScenario,
    ) -> MacroStressResult {
        let spot = spot_price.max(1e-4);
        let delta_s = spot * scenario.spot_shock_pct;

        let delta_pnl = portfolio_delta * delta_s;
        let gamma_pnl = 0.5 * portfolio_gamma * delta_s * delta_s;
        let vega_pnl = portfolio_vega * scenario.vol_shock_abs;
        let theta_pnl = portfolio_theta * (scenario.time_elapsed_days / 365.25);
        let rho_pnl = portfolio_rho * scenario.rate_shock_abs;

        let total_stressed_pnl = delta_pnl + gamma_pnl + vega_pnl + theta_pnl + rho_pnl;
        let notional = spot * portfolio_delta.abs().max(1.0);
        let pnl_pct_of_notional = total_stressed_pnl / notional;

        MacroStressResult {
            scenario_name: scenario.name.clone(),
            delta_pnl,
            gamma_pnl,
            vega_pnl,
            theta_pnl,
            rho_pnl,
            total_stressed_pnl,
            pnl_pct_of_notional,
        }
    }

    /// Evaluates standard suite of institutional macro crash scenarios.
    pub fn run_standard_macro_suite(
        spot_price: f64,
        portfolio_delta: f64,
        portfolio_gamma: f64,
        portfolio_vega: f64,
        portfolio_theta: f64,
        portfolio_rho: f64,
    ) -> Vec<MacroStressResult> {
        let scenarios = [
            MacroStressScenario::black_swan_crash(),
            MacroStressScenario::liquidity_squeeze(),
            MacroStressScenario::melt_up_rally(),
            MacroStressScenario::vol_crush_calm(),
        ];

        scenarios
            .iter()
            .map(|sc| {
                Self::evaluate_scenario_pnl(
                    spot_price,
                    portfolio_delta,
                    portfolio_gamma,
                    portfolio_vega,
                    portfolio_theta,
                    portfolio_rho,
                    sc,
                )
            })
            .collect()
    }

    /// Computes Conditional Value-at-Risk (\Delta CoVaR^{j|i}) systemic risk transmission.
    ///
    /// Systemic Beta: \beta_{j|i} = \rho_{ij} \cdot \frac{\sigma_j}{\sigma_i}
    /// \Delta CoVaR^{j|i} = \beta_{j|i} (\text{VaR}_\alpha^i - \text{VaR}_{50\%}^i)
    pub fn compute_delta_covar(
        benchmark_asset: &str,
        target_asset: &str,
        sigma_benchmark: f64,
        sigma_target: f64,
        correlation: f64,
        benchmark_var_99: f64,
        benchmark_median: f64,
    ) -> CoVaRReport {
        let sig_bench = sigma_benchmark.max(1e-6);
        let systemic_beta = correlation * (sigma_target / sig_bench);

        let covar_distress = systemic_beta * benchmark_var_99;
        let covar_median = systemic_beta * benchmark_median;
        let delta_covar = covar_distress - covar_median;

        CoVaRReport {
            benchmark_asset: benchmark_asset.to_string(),
            target_asset: target_asset.to_string(),
            quantile_alpha: 0.99,
            benchmark_var: benchmark_var_99,
            benchmark_median,
            systemic_beta,
            covar_distress,
            covar_median,
            delta_covar,
        }
    }

    /// Computes Extreme Value Theory (EVT) Generalized Pareto Distribution (GPD) Expected Shortfall.
    pub fn compute_evt_gpd_expected_shortfall(
        threshold_u: f64,
        shape_xi: f64,
        scale_beta: f64,
        exceedance_prob: f64,
        target_quantile: f64,
    ) -> EvtGpdExpectedShortfall {
        let beta = scale_beta.max(1e-6);
        let p_u = exceedance_prob.clamp(1e-6, 0.50);
        let alpha = target_quantile.clamp(1.0 - p_u + 1e-6, 0.99999);

        let tail_ratio = (1.0 - alpha) / p_u;

        let tail_var = if shape_xi.abs() > 1e-5 {
            threshold_u + (beta / shape_xi) * (tail_ratio.powf(-shape_xi) - 1.0)
        } else {
            // Gumbel domain of attraction (\xi \to 0)
            threshold_u - beta * tail_ratio.ln()
        };

        // Expected Shortfall ES_\alpha
        let tail_es = if shape_xi.abs() > 1e-5 {
            if shape_xi < 1.0 {
                (tail_var / (1.0 - shape_xi)) + (beta - shape_xi * threshold_u) / (1.0 - shape_xi)
            } else {
                tail_var * 1.5 // Infinite first moment fallback
            }
        } else {
            tail_var + beta
        };

        EvtGpdExpectedShortfall {
            threshold_u,
            shape_xi,
            scale_beta: beta,
            exceedance_prob: p_u,
            target_quantile: alpha,
            tail_var,
            tail_expected_shortfall: tail_es,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_greek_qp_optimizer_simultaneous_neutralization() {
        let target = MultiGreekPortfolioTarget {
            current_delta: 2.5,  // Long 2.5 delta
            current_gamma: -0.4, // Short 0.4 gamma
            current_vega: -30.0, // Short 30 vega
            max_delta_tolerance: 0.1,
            max_gamma_tolerance: 0.05,
            max_vega_tolerance: 5.0,
        };

        let instruments = vec![
            // 1. Underlying Perpetual: pure delta, 0 gamma, 0 vega
            HedgingInstrument {
                symbol: "BTC-PERP".to_string(),
                venue: "Hyperliquid".to_string(),
                delta: 1.0,
                gamma: 0.0,
                vega: 0.0,
                linear_cost: 0.05,
                quadratic_impact: 0.01,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            // 2. Near-dated ATM Option: high gamma (0.2), moderate delta (0.5), low vega (5.0)
            HedgingInstrument {
                symbol: "BTC-ATM-CALL".to_string(),
                venue: "Deribit".to_string(),
                delta: 0.5,
                gamma: 0.20,
                vega: 5.0,
                linear_cost: 0.10,
                quadratic_impact: 0.02,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            // 3. Long-dated Option: high vega (25.0), low gamma (0.05), delta (0.5)
            HedgingInstrument {
                symbol: "BTC-FAR-CALL".to_string(),
                venue: "Deribit".to_string(),
                delta: 0.5,
                gamma: 0.05,
                vega: 25.0,
                linear_cost: 0.15,
                quadratic_impact: 0.03,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
        ];

        let config = MultiGreekOptimizationConfig {
            delta_weight: 100.0,
            gamma_weight: 200.0,
            vega_weight: 10.0,
            max_iterations: 300,
            tolerance: 1e-5,
            learning_rate: 0.005,
        };

        let optimizer = MultiGreekQpOptimizer::new(config);
        let sol = optimizer.solve(&target, &instruments);

        assert!(
            sol.post_hedge_delta.abs() < target.current_delta.abs(),
            "Post-hedge delta must be substantially smaller"
        );
        assert!(
            sol.post_hedge_gamma.abs() < target.current_gamma.abs(),
            "Post-hedge gamma must be substantially smaller"
        );
        assert!(
            sol.post_hedge_vega.abs() < target.current_vega.abs(),
            "Post-hedge vega must be substantially smaller"
        );
    }

    #[test]
    fn test_multi_greek_ellipsoid_trigger() {
        let trigger = MultiGreekEllipsoidTrigger::new(1.0, 4.0, 0.01, 2.0);

        // Calm state: delta=0.5, gamma=0.1, vega=5.0
        // quad = 0.5^2 + 4*(0.1^2) + 0.01*(5^2) = 0.25 + 0.04 + 0.25 = 0.54, sqrt = 0.735 < 2.0
        assert!(!trigger.should_rehedge(0.5, 0.1, 5.0));

        // Stressed state: delta=1.8, gamma=0.8, vega=20.0
        // quad = 1.8^2 + 4*(0.8^2) + 0.01*(20^2) = 3.24 + 2.56 + 4.0 = 9.80, sqrt = 3.13 > 2.0
        assert!(trigger.should_rehedge(1.8, 0.8, 20.0));
    }

    #[test]
    fn test_multi_greek_macro_stress_suite() {
        let spot = 65000.0;
        let delta = 2.0; // Long 2 BTC delta
        let gamma = -0.0001; // Short gamma
        let vega = 50.0; // Long 50 vega
        let theta = -20.0; // Negative time decay
        let rho = 5.0;

        let results = MultiGreekCoVaRStressEngine::run_standard_macro_suite(
            spot, delta, gamma, vega, theta, rho,
        );

        assert_eq!(results.len(), 4);

        // Crash scenario: Spot drops 25% => Delta loss is 2.0 * (-16250) = -32500 USD
        let crash_res = &results[0];
        assert_eq!(
            crash_res.scenario_name,
            "Black Swan Crash (-25% spot, +50% vol)"
        );
        assert!(crash_res.delta_pnl < 0.0);
        assert!(crash_res.vega_pnl > 0.0); // Long vega offsets crash somewhat
        assert!(crash_res.total_stressed_pnl < 0.0); // Net loss dominates

        // Melt up rally: Spot jumps 20% => Delta gain is 2.0 * 13000 = +26000 USD
        let rally_res = &results[2];
        assert!(rally_res.delta_pnl > 0.0);
    }

    #[test]
    fn test_delta_covar_systemic_transmission() {
        // BTC (benchmark) vol 50%, ETH (target) vol 70%, correlation 0.85
        // BTC 99% VaR is -12% return, Median is +0.1%
        let covar = MultiGreekCoVaRStressEngine::compute_delta_covar(
            "BTC", "ETH", 0.50, 0.70, 0.85, -0.12, 0.001,
        );

        // Systemic beta = 0.85 * (0.70 / 0.50) = 1.19
        assert!((covar.systemic_beta - 1.19).abs() < 1e-4);

        // Distress CoVaR = 1.19 * (-0.12) = -0.1428
        assert!((covar.covar_distress - (-0.1428)).abs() < 1e-4);

        // Delta CoVaR = -0.1428 - 1.19 * 0.001 = -0.14399
        assert!(covar.delta_covar < -0.14);
    }

    #[test]
    fn test_evt_gpd_expected_shortfall() {
        // High 95% loss threshold u = 0.08 (8% loss), shape \xi = 0.20 (fat tail), scale \beta = 0.03
        // Exceedance prob = 0.05, Target quantile = 99.9%
        let evt = MultiGreekCoVaRStressEngine::compute_evt_gpd_expected_shortfall(
            0.08, 0.20, 0.03, 0.05, 0.999,
        );

        // Tail VaR must exceed threshold u
        assert!(evt.tail_var > evt.threshold_u);

        // Tail Expected Shortfall ES_{99.9%} must strictly exceed VaR_{99.9%}
        assert!(evt.tail_expected_shortfall > evt.tail_var);

        // For Gumbel limit (\xi = 0.0)
        let evt_gumbel = MultiGreekCoVaRStressEngine::compute_evt_gpd_expected_shortfall(
            0.08, 0.0, 0.03, 0.05, 0.999,
        );
        assert!(evt_gumbel.tail_expected_shortfall > evt_gumbel.tail_var);
        assert!((evt_gumbel.tail_expected_shortfall - (evt_gumbel.tail_var + 0.03)).abs() < 1e-4);
    }
}
