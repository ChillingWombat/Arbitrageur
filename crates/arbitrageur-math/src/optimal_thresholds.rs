use crate::ou_process::OuModelParams;

/// Result of optimal threshold optimization for a mean-reverting process.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptimalThresholdResult {
    /// Optimal entry spread level for long position (enter when spread <= entry_long)
    pub entry_long: f64,
    /// Optimal exit spread level for long position (exit when spread >= exit_long)
    pub exit_long: f64,
    /// Optimal entry spread level for short position (enter when spread >= entry_short)
    pub entry_short: f64,
    /// Optimal exit spread level for short position (exit when spread <= exit_short)
    pub exit_short: f64,
    /// Half-width threshold distance d = |entry - exit|
    pub optimal_distance: f64,
    /// Expected return per trade cycle after costs
    pub expected_return_per_cycle: f64,
    /// Estimated trade frequency per year
    pub estimated_trades_per_year: f64,
    /// Estimated annual Sharpe ratio
    pub estimated_sharpe: f64,
}

/// Computes optimal entry and exit thresholds using Bertram (2010) optimization.
///
/// Parameters:
/// - `ou`: Fitted Ornstein-Uhlenbeck parameters
/// - `transaction_cost_c`: Total transaction cost (round-trip fees + slippage)
pub fn compute_bertram_thresholds(
    ou: &OuModelParams,
    transaction_cost_c: f64,
) -> Option<OptimalThresholdResult> {
    if ou.theta <= 0.0 || ou.sigma <= 0.0 || transaction_cost_c < 0.0 {
        return None;
    }

    // Grid search / Newton-Raphson approximation for optimal distance d > c
    // that maximizes trade rate * expected return per cycle:
    // f(d) = (d - c) / E[T(d)]
    let std = ou.stationary_std;
    let min_d = transaction_cost_c * 1.05;
    let max_d = (std * 4.0).max(transaction_cost_c * 5.0);
    let steps = 200;
    let step_size = (max_d - min_d) / steps as f64;

    let mut best_d = min_d + step_size;
    let mut best_metric = -1e9;

    for i in 1..=steps {
        let d = min_d + (i as f64) * step_size;
        let net_return = d - transaction_cost_c;
        if net_return <= 0.0 {
            continue;
        }

        // Expected passage time approximation for OU process from mu - d to mu:
        // E[T] ~ (pi / 2*theta) * (d / (sigma / sqrt(2*theta)))^2 + higher order terms
        let normalized_d = d / std;
        // Accurate asymptotic expansion for first passage time of OU
        let expected_time = (std::f64::consts::PI / (2.0 * ou.theta)) * normalized_d * normalized_d
            + (normalized_d.powi(4)) / (12.0 * ou.theta);

        if expected_time > 1e-6 {
            let annual_trades = 365.25 * 86400.0 / expected_time;
            let annualized_return = net_return * annual_trades;
            let variance = d * d * annual_trades;
            let sharpe = if variance > 1e-9 {
                annualized_return / variance.sqrt()
            } else {
                0.0
            };

            let metric = sharpe;
            if metric > best_metric {
                best_metric = metric;
                best_d = d;
            }
        }
    }

    let optimal_distance = best_d;
    let net_return = optimal_distance - transaction_cost_c;
    let normalized_d = optimal_distance / std;
    let expected_time = (std::f64::consts::PI / (2.0 * ou.theta)) * normalized_d * normalized_d
        + (normalized_d.powi(4)) / (12.0 * ou.theta);
    let trades_per_year = (365.25 * 86400.0 / expected_time).min(10000.0);
    let sharpe = (best_metric).max(0.0);

    Some(OptimalThresholdResult {
        entry_long: ou.mu - optimal_distance,
        exit_long: ou.mu,
        entry_short: ou.mu + optimal_distance,
        exit_short: ou.mu,
        optimal_distance,
        expected_return_per_cycle: net_return,
        estimated_trades_per_year: trades_per_year,
        estimated_sharpe: sharpe,
    })
}
