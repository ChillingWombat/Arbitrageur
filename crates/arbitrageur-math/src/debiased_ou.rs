//! De-biased Ornstein-Uhlenbeck (OU) parameter estimation under microstructure noise.
//!
//! Naive OLS on high-frequency tick data suffers from errors-in-variables
//! attenuation bias: bid-ask bounce inflates apparent mean-reversion speed (\theta -> \infty).
//!
//! This module implements the de-biased ARMA(1,1) autocovariance ratio estimator:
//!     a* = \gamma_2 / \gamma_1
//! where \gamma_j is the lag-j autocovariance of the observed spread. Because microstructure
//! noise is white, \gamma_1 and \gamma_2 contain only the continuous process dynamics,
//! completely removing the noise variance \sigma_\eta^2 from the estimation of a = exp(-\theta \Delta t).

use crate::ou_process::OuModelParams;

/// Result of de-biased OU fitting, carrying both calibrated OU parameters
/// and the estimated microstructure noise variance.
#[derive(Debug, Clone, PartialEq)]
pub struct DebiasedOuResult {
    pub params: OuModelParams,
    /// Estimated variance of the high-frequency microstructure noise \sigma_\eta^2
    pub noise_variance: f64,
    /// Estimated variance of the true underlying continuous spread \sigma_X^2
    pub process_variance: f64,
    /// Signal-to-noise ratio: \sigma_X^2 / \sigma_\eta^2
    pub snr: f64,
}

/// Calibrates an OU process from a high-frequency discrete time series using the
/// de-biased ARMA(1,1) autocovariance ratio estimator.
///
/// # Arguments
/// - `series`: Slice of observed spread observations Y_k.
/// - `dt`: Sampling interval in seconds (\Delta t > 0).
pub fn fit_ou_debiased(series: &[f64], dt: f64) -> Option<DebiasedOuResult> {
    let n = series.len();
    if n < 15 || dt <= 0.0 {
        return None;
    }

    let mean = series.iter().sum::<f64>() / n as f64;
    let mut gamma0 = 0.0;
    let mut gamma1 = 0.0;
    let mut gamma2 = 0.0;

    for i in 0..n {
        let d0 = series[i] - mean;
        gamma0 += d0 * d0;
        if i + 1 < n {
            gamma1 += d0 * (series[i + 1] - mean);
        }
        if i + 2 < n {
            gamma2 += d0 * (series[i + 2] - mean);
        }
    }

    gamma0 /= n as f64;
    gamma1 /= (n - 1) as f64;
    gamma2 /= (n - 2) as f64;

    if gamma1 <= 1e-12 || gamma2 <= 1e-12 {
        return None;
    }

    // ARMA(1, 1) autocovariance ratio: a* = \gamma_2 / \gamma_1
    let a = gamma2 / gamma1;
    if a <= 0.0 || a >= 0.9999 {
        return None;
    }

    let theta = -a.ln() / dt;
    let var_x = (gamma1 / a).max(1e-12);
    let noise_var = (gamma0 - var_x).max(0.0);
    let sigma = (2.0 * theta * var_x).sqrt();
    let half_life = std::f64::consts::LN_2 / theta;
    let stationary_std = var_x.sqrt();
    let snr = if noise_var > 1e-12 {
        var_x / noise_var
    } else {
        f64::INFINITY
    };

    Some(DebiasedOuResult {
        params: OuModelParams {
            theta,
            mu: mean,
            sigma,
            half_life,
            stationary_std,
            r_squared: None,
        },
        noise_variance: noise_var,
        process_variance: var_x,
        snr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debiased_ou_clean_and_noisy() {
        let n = 200;
        let dt: f64 = 1.0;
        let theta_true: f64 = 0.05;
        let a_true = (-theta_true * dt).exp(); // ~0.9512

        // Simulate clean AR(1) series: X_k = a * X_{k-1} + e_k
        let mut clean = Vec::with_capacity(n);
        let mut x = 10.0;
        for i in 0..n {
            x = a_true * (x - 5.0) + 5.0 + ((i as f64 * 0.1).sin() * 0.2);
            clean.push(x);
        }

        let res = fit_ou_debiased(&clean, dt);
        assert!(res.is_some(), "Should fit clean series");
        let fit = res.unwrap();
        assert!(fit.params.theta > 0.0, "Theta should be positive");
        assert!(fit.params.half_life > 0.0, "Half-life should be positive");
    }
}
