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

/// Standard normal CDF via the shared Abramowitz–Stegun approximation.
fn phi(x: f64) -> f64 {
    crate::autotune::norm_cdf(x).unwrap_or(if x < 0.0 { 0.0 } else { 1.0 })
}

/// First and second moments of the OU first-passage time from `mu - d`
/// back to `mu`, in SECONDS — the exact quadrature solution of the Dynkin
/// ODE `(σ²/2)T'' − θx·T' = −1` on (−∞,0], `T(0)=0`:
///
///   θ·E[T]   = √π ∫_{-a}^{0} e^{u²}(1+erf u) du          , a = d·√θ/σ
///   θ²·E[T²] = 4  ∫_{-a}^{0} e^{u²} ∫_{-∞}^{u} φ(v) e^{-v²} dv du
///   φ(v)     = √π ∫_{v}^{0} e^{w²}(1+erf w) dw = θ·E[T] from level v
///
/// Returns `(E[T], std(T))`. Replaces the former 2-term asymptotic
/// `πd²/2θ + d⁴/12θ`, which overstated passage time by up to ~20× at 4σ —
/// Monte-Carlo references: 0.90/1.43/2.05 θ⁻¹ at 1σ/2σ/4σ distances.
fn ou_passage_moments(theta: f64, stationary_std: f64, d: f64) -> Option<(f64, f64)> {
    if !theta.is_finite()
        || theta <= 0.0
        || !stationary_std.is_finite()
        || stationary_std <= 0.0
        || !d.is_finite()
        || d <= 0.0
    {
        return None;
    }
    // Integration bound a = |x|·√θ/σ = z/√2 for z = d/σ_stationary.
    let a = d / (stationary_std * std::f64::consts::SQRT_2);
    // Grid over [-lo, 0]: lo must cover the e^{-v²} tail of the inner
    // integral; beyond -10 the kernel is < 1e-43.
    let lo = -(a + 8.0).max(10.0);
    const M: usize = 2048;
    let step = -lo / M as f64;

    // h(w) = e^{w²}(1+erf w) = 2·e^{w²}·Φ(w√2)
    let h = |w: f64| (w * w).exp() * 2.0 * phi(w * std::f64::consts::SQRT_2);
    let node = |j: usize| lo + step * j as f64;

    // φ(v_j) = √π·∫_{v_j}^{0} h — backward cumulative trapezoid.
    let mut phi_v = vec![0.0; M + 1];
    for j in (0..M).rev() {
        let (wj, wj1) = (node(j), node(j + 1));
        phi_v[j] = phi_v[j + 1] + 0.5 * (h(wj) + h(wj1)) * step;
    }
    for v in phi_v.iter_mut() {
        *v *= std::f64::consts::PI.sqrt();
    }
    // Index of the lower bound -a on the grid.
    let ia = ((-a - lo) / step).round() as usize;

    // J(u_j) = ∫_{lo}^{u_j} φ(v)e^{-v²} dv — forward cumulative trapezoid.
    let mut cum_j = vec![0.0; M + 1];
    for j in 0..M {
        let (vj, vj1) = (node(j), node(j + 1));
        let f_j = phi_v[j] * (-vj * vj).exp();
        let f_j1 = phi_v[j + 1] * (-vj1 * vj1).exp();
        cum_j[j + 1] = cum_j[j] + 0.5 * (f_j + f_j1) * step;
    }

    // θ·E[T] = φ(-a) — the cumulative integral already evaluated at -a.
    let theta_et = phi_v[ia];
    // θ²·E[T²] = 4·∫_{-a}^{0} e^{u²}·J(u) du — trapezoid over [ia, M].
    let mut i2 = 0.0;
    for j in ia..M {
        let (uj, uj1) = (node(j), node(j + 1));
        let g_j = (uj * uj).exp() * cum_j[j];
        let g_j1 = (uj1 * uj1).exp() * cum_j[j + 1];
        i2 += 0.5 * (g_j + g_j1) * step;
    }

    let e_t = theta_et / theta;
    let e_t2 = 4.0 * i2 / (theta * theta);
    let var = (e_t2 - e_t * e_t).max(0.0);
    if !e_t.is_finite() || e_t <= 0.0 {
        return None;
    }
    Some((e_t, var.sqrt()))
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

    const YEAR_SECS: f64 = 365.25 * 86400.0;
    let mut best_d = min_d + step_size;
    let mut best_metric = -1e9;
    let mut best_moments = (0.0, 0.0);

    for i in 1..=steps {
        let d = min_d + (i as f64) * step_size;
        let net_return = d - transaction_cost_c;
        if net_return <= 0.0 {
            continue;
        }
        let Some((e_t, std_t)) = ou_passage_moments(ou.theta, std, d) else {
            continue;
        };
        // Bertram's objective: expected return per unit time — the renewal
        // rate of profitable cycles, not a squared-time pseudo-Sharpe.
        let metric = net_return / e_t;
        if metric > best_metric {
            best_metric = metric;
            best_d = d;
            best_moments = (e_t, std_t);
        }
    }
    if best_metric <= 0.0 {
        return None;
    }

    let optimal_distance = best_d;
    let net_return = optimal_distance - transaction_cost_c;
    let (e_t, std_t) = best_moments;
    let trades_per_year = (YEAR_SECS / e_t).min(10000.0);
    // Annualized Sharpe under the renewal-reward model: mean return rate
    // r/E[T] over its asymptotic std r·√(Var(T)/E[T]³), which reduces to
    // √N/CV(T) — √(cycles/year) divided by the passage-time coefficient
    // of variation. Backed by the same passage-time quadrature, not a
    // fabricated fit statistic.
    let cv_t = if e_t > 0.0 {
        std_t / e_t
    } else {
        f64::INFINITY
    };
    let sharpe = if cv_t.is_finite() && cv_t > 1e-9 {
        trades_per_year.sqrt() / cv_t
    } else {
        0.0
    };

    Some(OptimalThresholdResult {
        entry_long: ou.mu - optimal_distance,
        exit_long: ou.mu,
        entry_short: ou.mu + optimal_distance,
        exit_short: ou.mu,
        optimal_distance,
        expected_return_per_cycle: net_return,
        estimated_trades_per_year: trades_per_year,
        estimated_sharpe: sharpe.max(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (#46): the exact OU first-passage quadrature must match
    /// Monte-Carlo references — 0.90 / 1.43 / 2.05 θ⁻¹ at 1σ/2σ/4σ
    /// stationary-standard-deviation distances (the replaced 2-term
    /// asymptotic gave 1.65/7.62/46.5 — up to ~20× off at 4σ).
    #[test]
    fn passage_time_matches_monte_carlo_references() {
        for (z, want) in [(1.0_f64, 0.90), (2.0, 1.43), (4.0, 2.05)] {
            // theta = 1, stationary_std = 1 → d = z makes θ·E[T] = E[T].
            let (e_t, std_t) = ou_passage_moments(1.0, 1.0, z).expect("moments");
            assert!(
                (e_t - want).abs() / want < 0.10,
                "z={z}: E[T]·θ got {e_t:.3}, MC reference {want}"
            );
            assert!(std_t > 0.0 && std_t.is_finite());
        }
    }

    /// The optimizer must pick a distance above cost and report a finite,
    /// backed Sharpe — never a fabricated constant.
    #[test]
    fn optimizer_returns_backed_estimates() {
        let ou = OuModelParams {
            theta: 0.5,
            mu: 100.0,
            sigma: 0.4,
            half_life: 1.386,
            stationary_std: 0.4,
            r_squared: None,
        };
        let res = compute_bertram_thresholds(&ou, 0.01).expect("thresholds");
        assert!(res.optimal_distance > 0.01);
        assert!(res.estimated_trades_per_year > 0.0 && res.estimated_trades_per_year.is_finite());
        assert!(res.estimated_sharpe.is_finite() && res.estimated_sharpe >= 0.0);
        assert!((res.entry_long - (100.0 - res.optimal_distance)).abs() < 1e-9);
        assert!((res.entry_short - (100.0 + res.optimal_distance)).abs() < 1e-9);
    }
}
