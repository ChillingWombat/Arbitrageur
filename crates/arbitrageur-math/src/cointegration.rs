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
    // Numerically stable two-pass de-meaning (avoids catastrophic cancellation on large scales)
    let fn_count = n as f64;
    let mean_x = x.iter().sum::<f64>() / fn_count;
    let mean_y = y.iter().sum::<f64>() / fn_count;
    let mut var_x = 0.0;
    let mut cov_xy = 0.0;
    for i in 0..n {
        let dx = x[i] - mean_x;
        let dy = y[i] - mean_y;
        var_x += dx * dx;
        cov_xy += dx * dy;
    }

    if var_x.abs() < 1e-15 {
        return None;
    }

    let beta = cov_xy / var_x;
    let alpha = mean_y - beta * mean_x;

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

/// Result of Johansen Cointegration Test.
#[derive(Debug, Clone, PartialEq)]
pub struct JohansenResult {
    /// Number of statistically significant cointegrating vectors (rank r)
    pub cointegrating_rank: usize,
    /// Eigenvalues sorted descending
    pub eigenvalues: Vec<f64>,
    /// Trace test statistics: lambda_trace(r) = -T * sum_{i=r+1}^K ln(1 - lambda_i)
    pub trace_statistics: Vec<f64>,
    /// 95% critical values for trace test
    pub trace_critical_values: Vec<f64>,
    /// Normalized cointegrating vector beta (where spread = y - beta_hedge * x)
    pub beta: Vec<f64>,
    /// Speed of adjustment vector alpha
    pub alpha: Vec<f64>,
    /// Whether the system has at least one cointegrating vector at 95% confidence
    pub is_cointegrated: bool,
}

/// Compute bivariate (K=2) Johansen cointegration test between two series `y` and `x`.
///
/// Solves the generalized eigenvalue problem:
/// det(lambda * S_11 - S_10 * S_00^-1 * S_01) = 0
/// for trace statistics, cointegrating rank r, cointegrating vector beta, and speed of adjustment alpha.
pub fn johansen_cointegration_test_2x2(y: &[f64], x: &[f64]) -> Option<JohansenResult> {
    let n = y.len();
    if n < 35 || n != x.len() {
        return None;
    }

    // Step 1: Form differences Delta Y_t and lagged levels Y_{t-1}
    let t = n - 1;
    let mut dy = Vec::with_capacity(t);
    let mut dx = Vec::with_capacity(t);
    let mut y_lag = Vec::with_capacity(t);
    let mut x_lag = Vec::with_capacity(t);

    for i in 0..t {
        dy.push(y[i + 1] - y[i]);
        dx.push(x[i + 1] - x[i]);
        y_lag.push(y[i]);
        x_lag.push(x[i]);
    }

    // Centered two-pass de-meaning
    let t_f = t as f64;
    let mean_dy = dy.iter().sum::<f64>() / t_f;
    let mean_dx = dx.iter().sum::<f64>() / t_f;
    let mean_yl = y_lag.iter().sum::<f64>() / t_f;
    let mean_xl = x_lag.iter().sum::<f64>() / t_f;

    // Form 2x2 moment matrices S_00, S_11, S_01
    let mut s00_00 = 0.0;
    let mut s00_01 = 0.0;
    let mut s00_11 = 0.0;

    let mut s11_00 = 0.0;
    let mut s11_01 = 0.0;
    let mut s11_11 = 0.0;

    let mut s01_00 = 0.0;
    let mut s01_01 = 0.0;
    let mut s01_10 = 0.0;
    let mut s01_11 = 0.0;

    for i in 0..t {
        let u0 = dy[i] - mean_dy;
        let u1 = dx[i] - mean_dx;
        let v0 = y_lag[i] - mean_yl;
        let v1 = x_lag[i] - mean_xl;

        s00_00 += u0 * u0;
        s00_01 += u0 * u1;
        s00_11 += u1 * u1;

        s11_00 += v0 * v0;
        s11_01 += v0 * v1;
        s11_11 += v1 * v1;

        s01_00 += u0 * v0;
        s01_01 += u0 * v1;
        s01_10 += u1 * v0;
        s01_11 += u1 * v1;
    }

    s00_00 /= t_f;
    s00_01 /= t_f;
    s00_11 /= t_f;

    s11_00 /= t_f;
    s11_01 /= t_f;
    s11_11 /= t_f;

    s01_00 /= t_f;
    s01_01 /= t_f;
    s01_10 /= t_f;
    s01_11 /= t_f;

    // Invert S_00
    let det_s00 = s00_00 * s00_11 - s00_01 * s00_01;
    if det_s00.abs() < 1e-15 {
        return None;
    }
    let inv_s00_00 = s00_11 / det_s00;
    let inv_s00_01 = -s00_01 / det_s00;
    let inv_s00_11 = s00_00 / det_s00;

    // Invert S_11
    let det_s11 = s11_00 * s11_11 - s11_01 * s11_01;
    if det_s11.abs() < 1e-15 {
        return None;
    }
    let inv_s11_00 = s11_11 / det_s11;
    let inv_s11_01 = -s11_01 / det_s11;
    let inv_s11_11 = s11_00 / det_s11;

    // Compute P = S_10 * S_00^-1:
    let p_00 = s01_00 * inv_s00_00 + s01_10 * inv_s00_01;
    let p_01 = s01_00 * inv_s00_01 + s01_10 * inv_s00_11;
    let p_10 = s01_01 * inv_s00_00 + s01_11 * inv_s00_01;
    let p_11 = s01_01 * inv_s00_01 + s01_11 * inv_s00_11;

    // Compute Q = P * S_01:
    let q_00 = p_00 * s01_00 + p_01 * s01_10;
    let q_01 = p_00 * s01_01 + p_01 * s01_11;
    let q_10 = p_10 * s01_00 + p_11 * s01_10;
    let q_11 = p_10 * s01_01 + p_11 * s01_11;

    // Companion matrix M = S_11^-1 * Q:
    let m_00 = inv_s11_00 * q_00 + inv_s11_01 * q_10;
    let m_01 = inv_s11_00 * q_01 + inv_s11_01 * q_11;
    let m_10 = inv_s11_01 * q_00 + inv_s11_11 * q_10;
    let m_11 = inv_s11_01 * q_01 + inv_s11_11 * q_11;

    // Eigenvalues of 2x2 companion matrix:
    // lambda^2 - tr(M)*lambda + det(M) = 0
    let tr_m = m_00 + m_11;
    let det_m = m_00 * m_11 - m_01 * m_10;
    let disc = (tr_m * tr_m - 4.0 * det_m).max(0.0);
    let sqrt_disc = disc.sqrt();

    let l1 = ((tr_m + sqrt_disc) / 2.0).clamp(0.0, 0.9999);
    let l2 = ((tr_m - sqrt_disc) / 2.0).clamp(0.0, 0.9999);

    let (lambda_1, lambda_2) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };

    // Johansen Trace Statistics:
    // trace(0) = -T * (ln(1 - lambda_1) + ln(1 - lambda_2))
    // trace(1) = -T * ln(1 - lambda_2)
    let trace_0 = -t_f * ((1.0 - lambda_1).ln() + (1.0 - lambda_2).ln());
    let trace_1 = -t_f * (1.0 - lambda_2).ln();

    // 95% critical values for bivariate Johansen test with constant:
    let crit_0 = 15.49;
    let crit_1 = 3.84;

    let rank = if trace_0 > crit_0 {
        if trace_1 > crit_1 {
            2
        } else {
            1
        }
    } else {
        0
    };

    let beta_x = if m_01.abs() > 1e-12 {
        -(lambda_1 - m_00) / m_01
    } else if (m_11 - lambda_1).abs() > 1e-12 {
        -m_10 / (lambda_1 - m_11)
    } else {
        1.0
    };

    let b0 = 1.0;
    let b1 = beta_x;
    let s11_quad = b0 * (s11_00 * b0 + s11_01 * b1) + b1 * (s11_01 * b0 + s11_11 * b1);
    let denom = s11_quad.max(1e-12);

    let alpha_0 = (s01_00 * b0 + s01_01 * b1) / denom;
    let alpha_1 = (s01_10 * b0 + s01_11 * b1) / denom;

    Some(JohansenResult {
        cointegrating_rank: rank,
        eigenvalues: vec![lambda_1, lambda_2],
        trace_statistics: vec![trace_0, trace_1],
        trace_critical_values: vec![crit_0, crit_1],
        beta: vec![1.0, beta_x],
        alpha: vec![alpha_0, alpha_1],
        is_cointegrated: rank >= 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cointegration_large_magnitude_numerical_stability() {
        // Test with raw BTC scale prices (100,000 USD)
        let n = 100;
        let mut x = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let true_beta = 1.85;
        let true_alpha = 500.0;

        for i in 0..n {
            let px = 100_000.0 + (i as f64) * 25.0;
            let noise = ((i % 5) as f64 - 2.0) * 2.0;
            let py = true_alpha + true_beta * px + noise;
            x.push(px);
            y.push(py);
        }

        let res =
            engle_granger_test(&y, &x).expect("engle_granger_test must succeed on large inputs");
        assert!(
            (res.beta - true_beta).abs() < 1e-3,
            "Beta should be close to true beta, got {}",
            res.beta
        );
        assert!(
            (res.alpha - true_alpha).abs() < 50.0,
            "Alpha should be close to true alpha, got {}",
            res.alpha
        );
        assert!(res.is_cointegrated_5pct);
    }

    #[test]
    fn test_johansen_cointegration_bivariate_stationary() {
        let n = 120;
        let mut x = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut cur_x = 100.0;
        let true_beta = 1.5;

        for i in 0..n {
            // Random walk for x
            cur_x += ((i % 7) as f64 - 3.0) * 0.5;
            // Stationary noise for y
            let noise = ((i % 5) as f64 - 2.0) * 0.2;
            let cur_y = true_beta * cur_x + noise;
            x.push(cur_x);
            y.push(cur_y);
        }

        let res = johansen_cointegration_test_2x2(&y, &x)
            .expect("johansen_cointegration_test_2x2 should succeed");
        assert!(res.is_cointegrated);
        assert!(res.cointegrating_rank >= 1);
        assert!(res.trace_statistics[0] > res.trace_critical_values[0]);
        assert_eq!(res.beta.len(), 2);
    }

    #[test]
    fn test_johansen_cointegration_insufficient_samples() {
        let x = vec![1.0; 20];
        let y = vec![1.0; 20];
        assert!(johansen_cointegration_test_2x2(&y, &x).is_none());
    }
}
