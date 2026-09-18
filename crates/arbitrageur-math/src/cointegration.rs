/// Result of Engle-Granger Cointegration Test.
#[derive(Debug, Clone, PartialEq)]
pub struct CointegrationResult {
    /// Hedge ratio \beta: Spread = y - \beta * x
    pub beta: f64,
    /// Intercept \alpha
    pub alpha: f64,
    /// ADF test t-statistic on residuals
    pub adf_statistic: f64,
    /// Critical values at 1%, 5%, 10% significance
    pub critical_values: (f64, f64, f64),
    /// Whether the pair is cointegrated at 5% significance level
    pub is_cointegrated_5pct: bool,
    /// Half-life of residual mean-reversion
    pub half_life: f64,
}

/// Compute Engle-Granger cointegration test between two series `y` and `x`.
pub fn engle_granger_test(y: &[f64], x: &[f64]) -> Option<CointegrationResult> {
    let n = y.len();
    if n < 30 || n != x.len() {
        return None;
    }

    // Step 1: OLS regression y = alpha + beta * x
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xy = 0.0;
    let fn_count = n as f64;

    for i in 0..n {
        sum_x += x[i];
        sum_y += y[i];
        sum_xx += x[i] * x[i];
        sum_xy += x[i] * y[i];
    }

    let denom = fn_count * sum_xx - sum_x * sum_x;
    if denom.abs() < 1e-15 {
        return None;
    }

    let beta = (fn_count * sum_xy - sum_x * sum_y) / denom;
    let alpha = (sum_y - beta * sum_x) / fn_count;

    // Step 2: Extract residuals
    let mut residuals = Vec::with_capacity(n);
    for i in 0..n {
        residuals.push(y[i] - (alpha + beta * x[i]));
    }

    // Step 3: Run ADF test on residuals:
    // \Delta e_t = \gamma e_{t-1} + \sum \phi_i \Delta e_{t-i} + u_t
    // Simplified 1-lag ADF test:
    let m = n - 1;
    let mut delta_e = Vec::with_capacity(m);
    let mut lag_e = Vec::with_capacity(m);

    for i in 0..m {
        lag_e.push(residuals[i]);
        delta_e.push(residuals[i + 1] - residuals[i]);
    }

    // Regress delta_e on lag_e (no intercept in cointegration residual ADF)
    let mut sum_lag_sq = 0.0;
    let mut sum_lag_delta = 0.0;
    for i in 0..m {
        sum_lag_sq += lag_e[i] * lag_e[i];
        sum_lag_delta += lag_e[i] * delta_e[i];
    }

    if sum_lag_sq.abs() < 1e-15 {
        return None;
    }

    let gamma = sum_lag_delta / sum_lag_sq;

    // Residual variance
    let mut sse = 0.0;
    for i in 0..m {
        let err = delta_e[i] - gamma * lag_e[i];
        sse += err * err;
    }
    let s2 = sse / ((m - 1) as f64).max(1.0);
    let se_gamma = (s2 / sum_lag_sq).sqrt();

    let adf_statistic = if se_gamma > 1e-15 {
        gamma / se_gamma
    } else {
        0.0
    };

    // Engle-Granger MacKinnon critical values for 2 variables:
    // 1%: -3.90, 5%: -3.34, 10%: -3.04
    let crit_1pct = -3.90;
    let crit_5pct = -3.34;
    let crit_10pct = -3.04;

    let is_cointegrated_5pct = adf_statistic < crit_5pct;

    // Half-life: gamma = exp(-theta) - 1 => theta = -ln(1 + gamma)
    let half_life = if gamma < 0.0 && (1.0 + gamma) > 0.0 {
        -std::f64::consts::LN_2 / (1.0 + gamma).ln()
    } else {
        f64::INFINITY
    };

    Some(CointegrationResult {
        beta,
        alpha,
        adf_statistic,
        critical_values: (crit_1pct, crit_5pct, crit_10pct),
        is_cointegrated_5pct,
        half_life,
    })
}
