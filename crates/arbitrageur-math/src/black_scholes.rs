//! High-performance Black-Scholes pricing, analytical Greeks, and implied volatility solvers.
//!
//! Features:
//! - Analytical European Call and Put pricing
//! - Analytical first and second-order Greeks (Delta, Gamma, Vega, Theta, Rho)
//! - Ultra-fast Implied Volatility solver utilizing Halley's method (cubic convergence)
//!   seeded by the Corrado-Miller / Brenner-Subrahmanyam analytical approximation,
//!   reaching <1e-10 precision in 2-3 iterations (vs 50+ iterations for bisection).

use std::f64::consts::PI;

/// Option contract exercise type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionType {
    Call,
    Put,
}

/// Black-Scholes Greeks and theoretical fair value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackScholesGreeks {
    /// Theoretical option price
    pub price: f64,
    /// First derivative w.r.t underlying spot price dV/dS
    pub delta: f64,
    /// Second derivative w.r.t underlying spot price d²V/dS²
    pub gamma: f64,
    /// Sensitivity w.r.t volatility dV/dσ (per 100% vol change)
    pub vega: f64,
    /// Time decay sensitivity -dV/dt (per year)
    pub theta: f64,
    /// Sensitivity w.r.t risk-free interest rate dV/dr
    pub rho: f64,
}

/// Standard normal probability density function (PDF): φ(x) = (1/√(2π)) * exp(-x²/2).
#[inline]
pub fn standard_normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * PI).sqrt()
}

/// High-precision standard normal cumulative distribution function (CDF): Φ(x).
/// Uses Hart's rational approximation / Cody's erf formulation with max absolute error < 1.5e-7.
#[inline]
pub fn standard_normal_cdf(x: f64) -> f64 {
    if !x.is_finite() {
        return if x < 0.0 { 0.0 } else { 1.0 };
    }
    let t = 1.0 / (1.0 + 0.2316419 * x.abs());
    let poly = t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let phi = standard_normal_pdf(x);
    if x >= 0.0 {
        (1.0 - phi * poly).clamp(0.0, 1.0)
    } else {
        (phi * poly).clamp(0.0, 1.0)
    }
}

/// Computes Black-Scholes d1 and d2 parameters:
/// d1 = [ln(S/K) + (r + σ²/2)T] / (σ√T)
/// d2 = d1 - σ√T
#[inline]
pub fn compute_d1_d2(s: f64, k: f64, t: f64, r: f64, sigma: f64) -> Option<(f64, f64)> {
    if s <= 0.0 || k <= 0.0 || t <= 0.0 || sigma <= 0.0 || !r.is_finite() {
        return None;
    }
    let sqrt_t = t.sqrt();
    let sigma_sqrt_t = sigma * sqrt_t;
    let d1 = ((s / k).ln() + (r + 0.5 * sigma * sigma) * t) / sigma_sqrt_t;
    let d2 = d1 - sigma_sqrt_t;
    if d1.is_finite() && d2.is_finite() {
        Some((d1, d2))
    } else {
        None
    }
}

/// Analytical Black-Scholes pricing for European options.
///
/// Parameters:
/// - `option_type`: Call or Put
/// - `s`: Underlying spot price (S > 0)
/// - `k`: Strike price (K > 0)
/// - `t`: Time to expiration in years (T > 0)
/// - `r`: Risk-free interest rate (annualized, continuous)
/// - `sigma`: Implied volatility (annualized, σ > 0)
pub fn black_scholes_price(
    option_type: OptionType,
    s: f64,
    k: f64,
    t: f64,
    r: f64,
    sigma: f64,
) -> Option<f64> {
    let (d1, d2) = compute_d1_d2(s, k, t, r, sigma)?;
    let discount = (-r * t).exp();

    match option_type {
        OptionType::Call => {
            let nd1 = standard_normal_cdf(d1);
            let nd2 = standard_normal_cdf(d2);
            Some((s * nd1 - k * discount * nd2).max(0.0))
        }
        OptionType::Put => {
            let n_neg_d1 = standard_normal_cdf(-d1);
            let n_neg_d2 = standard_normal_cdf(-d2);
            Some((k * discount * n_neg_d2 - s * n_neg_d1).max(0.0))
        }
    }
}

/// Computes Black-Scholes fair price and analytical Greeks.
pub fn black_scholes_greeks(
    option_type: OptionType,
    s: f64,
    k: f64,
    t: f64,
    r: f64,
    sigma: f64,
) -> Option<BlackScholesGreeks> {
    let (d1, d2) = compute_d1_d2(s, k, t, r, sigma)?;
    let sqrt_t = t.sqrt();
    let discount = (-r * t).exp();
    let pdf_d1 = standard_normal_pdf(d1);

    let gamma = pdf_d1 / (s * sigma * sqrt_t);
    let vega = s * sqrt_t * pdf_d1;

    match option_type {
        OptionType::Call => {
            let nd1 = standard_normal_cdf(d1);
            let nd2 = standard_normal_cdf(d2);
            let price = (s * nd1 - k * discount * nd2).max(0.0);
            let delta = nd1;
            let theta = -(s * pdf_d1 * sigma) / (2.0 * sqrt_t) - r * k * discount * nd2;
            let rho = k * t * discount * nd2;

            Some(BlackScholesGreeks {
                price,
                delta,
                gamma,
                vega,
                theta,
                rho,
            })
        }
        OptionType::Put => {
            let n_neg_d1 = standard_normal_cdf(-d1);
            let n_neg_d2 = standard_normal_cdf(-d2);
            let price = (k * discount * n_neg_d2 - s * n_neg_d1).max(0.0);
            let delta = nd1_to_put_delta(standard_normal_cdf(d1));
            let theta = -(s * pdf_d1 * sigma) / (2.0 * sqrt_t) + r * k * discount * n_neg_d2;
            let rho = -k * t * discount * n_neg_d2;

            Some(BlackScholesGreeks {
                price,
                delta,
                gamma,
                vega,
                theta,
                rho,
            })
        }
    }
}

#[inline]
fn nd1_to_put_delta(nd1: f64) -> f64 {
    nd1 - 1.0
}

/// Solves for Implied Volatility using Halley's method (cubic root-finding)
/// initialized via the Corrado-Miller analytical seed.
///
/// Halley's recurrence:
/// x_{n+1} = x_n - [f(x_n) / f'(x_n)] * [1 - (f(x_n) * f''(x_n)) / (2 * (f'(x_n))²)]⁻¹
/// Since f'(σ) = Vega and f''(σ) = Vega * d1 * d2 / σ, Halley's method evaluates
/// the curvature of the Black-Scholes pricing formula directly, giving cubic convergence.
pub fn implied_volatility(
    option_type: OptionType,
    market_price: f64,
    s: f64,
    k: f64,
    t: f64,
    r: f64,
) -> Option<f64> {
    if !market_price.is_finite() || market_price <= 0.0 || s <= 0.0 || k <= 0.0 || t <= 0.0 {
        return None;
    }

    let discount = (-r * t).exp();
    let discounted_k = k * discount;

    // Check no-arbitrage bounds
    let (call_price, intrinsic) = match option_type {
        OptionType::Call => {
            let intrinsic = (s - discounted_k).max(0.0);
            if market_price < intrinsic || market_price >= s {
                return None;
            }
            (market_price, intrinsic)
        }
        OptionType::Put => {
            let intrinsic = (discounted_k - s).max(0.0);
            if market_price < intrinsic || market_price >= discounted_k {
                return None;
            }
            // Put-call parity: C = P + S - K*e^(-rT)
            let synth_call = market_price + s - discounted_k;
            if synth_call <= 0.0 {
                return None;
            }
            (synth_call, (s - discounted_k).max(0.0))
        }
    };

    if (call_price - intrinsic).abs() < 1e-9 {
        return Some(1e-6); // Near zero volatility limit
    }

    // Corrado-Miller (1996) analytical seed approximation for Call:
    let sqrt_t = t.sqrt();
    let term1 = call_price - (s - discounted_k) * 0.5;
    let discr = term1 * term1 - (s - discounted_k) * (s - discounted_k) / PI;
    let seed = if discr >= 0.0 {
        let numer = (2.0 * PI).sqrt() * (term1 + discr.sqrt());
        let denom = (s + discounted_k) * sqrt_t;
        if denom > 0.0 {
            (numer / denom).clamp(0.01, 8.0)
        } else {
            0.3
        }
    } else {
        // Brenner-Subrahmanyam fallback: σ ≈ (C / S) * √(2π/T)
        ((call_price / s) * (2.0 * PI / t).sqrt()).clamp(0.01, 8.0)
    };

    // Halley's method iteration (maximum 8 iterations; typically converges in 2-3)
    let mut sigma = seed;
    for _ in 0..8 {
        let (d1, d2) = match compute_d1_d2(s, k, t, r, sigma) {
            Some(res) => res,
            None => break,
        };

        let nd1 = standard_normal_cdf(d1);
        let nd2 = standard_normal_cdf(d2);
        let current_price = s * nd1 - discounted_k * nd2;
        let diff = current_price - call_price;

        if diff.abs() < 1e-10 {
            return Some(sigma);
        }

        let pdf_d1 = standard_normal_pdf(d1);
        let vega = s * sqrt_t * pdf_d1;

        if vega < 1e-12 {
            break;
        }

        // Curvature ratio: f''(σ)/f'(σ) = d1 * d2 / σ
        let curvature_ratio = d1 * d2 / sigma;
        let denom = vega - 0.5 * diff * curvature_ratio;
        let step = if denom.abs() > 0.1 * vega {
            diff / denom
        } else {
            diff / vega
        };

        let next_sigma = sigma - step;
        if next_sigma <= 1e-6 {
            sigma *= 0.5;
        } else if next_sigma > 25.0 {
            sigma = (sigma + 25.0) * 0.5;
        } else {
            sigma = next_sigma;
        }

        if step.abs() < 1e-9 {
            return Some(sigma);
        }
    }

    // Robust Newton-Raphson fallback with dynamic damping
    let mut nr_sigma = sigma.clamp(1e-4, 25.0);
    for _ in 0..15 {
        let Some((d1, d2)) = compute_d1_d2(s, k, t, r, nr_sigma) else {
            break;
        };
        let nd1 = standard_normal_cdf(d1);
        let nd2 = standard_normal_cdf(d2);
        let current_price = s * nd1 - discounted_k * nd2;
        let diff = current_price - call_price;

        if diff.abs() < 1e-9 {
            return Some(nr_sigma);
        }

        let pdf_d1 = standard_normal_pdf(d1);
        let vega = s * sqrt_t * pdf_d1;
        if vega < 1e-15 {
            break;
        }

        let step = (diff / vega).clamp(-2.0, 2.0);
        nr_sigma = (nr_sigma - step).clamp(1e-6, 25.0);
        if step.abs() < 1e-9 {
            return Some(nr_sigma);
        }
    }

    // High-precision Bisection fallback if derivative-based solvers stalled in extreme tail
    let mut low = 1e-6;
    let mut high = 25.0;
    let mut best_est = nr_sigma;
    let mut best_err = f64::MAX;

    for _ in 0..40 {
        let mid = 0.5 * (low + high);
        let Some(p) = black_scholes_price(OptionType::Call, s, k, t, r, mid) else {
            break;
        };
        let diff = p - call_price;
        let err = diff.abs();
        if err < best_err {
            best_err = err;
            best_est = mid;
        }
        if err < 1e-8 || (high - low) < 1e-8 {
            return Some(mid);
        }
        if diff > 0.0 {
            high = mid;
        } else {
            low = mid;
        }
    }

    if best_err < 1e-4 {
        Some(best_est)
    } else {
        None
    }
}

/// Analytical SABR model implied volatility (Hagan et al. 2002 asymptotic expansion).
///
/// Parameters:
/// - `f`: Forward price (F > 0)
/// - `k`: Strike price (K > 0)
/// - `t`: Time to expiration in years (T > 0)
/// - `alpha`: Volatility level (alpha > 0)
/// - `beta`: Elasticity parameter (beta in [0, 1], typically 1.0 for lognormal crypto perps/futures)
/// - `rho`: Correlation between asset price and volatility (rho in (-1, 1))
/// - `nu`: Volatility of volatility (nu >= 0)
pub fn sabr_implied_volatility(
    f: f64,
    k: f64,
    t: f64,
    alpha: f64,
    beta: f64,
    rho: f64,
    nu: f64,
) -> Option<f64> {
    if f <= 0.0
        || k <= 0.0
        || t <= 0.0
        || alpha <= 0.0
        || nu < 0.0
        || rho.abs() >= 1.0
        || !(0.0..=1.0).contains(&beta)
    {
        return None;
    }
    let one_minus_beta = 1.0 - beta;
    if (f - k).abs() < 1e-8 {
        // ATM formula
        let f_b = f.powf(one_minus_beta);
        let term1 = alpha / f_b;
        let term2 = 1.0
            + ((one_minus_beta.powi(2) / 24.0) * (alpha * alpha / f.powf(2.0 * one_minus_beta))
                + 0.25 * rho * beta * nu * alpha / f_b
                + ((2.0 - 3.0 * rho * rho) / 24.0) * nu * nu)
                * t;
        return Some((term1 * term2).clamp(1e-4, 10.0));
    }

    let fk = f * k;
    let fk_b = fk.powf(one_minus_beta * 0.5);
    let log_fk = (f / k).ln();
    let z = (nu / alpha) * fk_b * log_fk;
    let rad = 1.0 - 2.0 * rho * z + z * z;
    if rad < 0.0 {
        return None;
    }
    let num_x = rad.sqrt() + z - rho;
    if num_x <= 0.0 {
        return None;
    }
    let x_z = (num_x / (1.0 - rho)).ln();
    let chi_z = if x_z.abs() < 1e-10 { 1.0 } else { z / x_z };

    let denom = fk_b
        * (1.0
            + (one_minus_beta.powi(2) / 24.0) * log_fk.powi(2)
            + (one_minus_beta.powi(4) / 1920.0) * log_fk.powi(4));
    if denom <= 0.0 {
        return None;
    }
    let factor = alpha / denom;
    let term = 1.0
        + ((one_minus_beta.powi(2) / 24.0) * (alpha * alpha / fk.powf(one_minus_beta))
            + 0.25 * rho * beta * nu * alpha / fk_b
            + ((2.0 - 3.0 * rho * rho) / 24.0) * nu * nu)
            * t;

    Some((factor * chi_z * term).clamp(1e-4, 10.0))
}

/// SVI (Stochastic Volatility Inspired) total implied variance w(k) = sigma^2(k, t) * t.
/// Gatheral (2004) formulation parameterized by log-moneyness k = ln(K / F).
///
/// w(k) = a + b * (rho * (k - m) + sqrt((k - m)^2 + sigma^2))
#[inline]
pub fn svi_total_variance(k: f64, a: f64, b: f64, rho: f64, m: f64, sigma: f64) -> f64 {
    let diff = k - m;
    let root = (diff * diff + sigma * sigma).sqrt();
    a + b * (rho * diff + root)
}

/// Verifies Roger Lee's (2004) asymptotic wing moment bounds:
/// Total implied variance slope as |k| -> infinity must satisfy:
/// limsup_{k -> +inf} w(k)/k = b * (1 + rho) <= 2.0
/// limsup_{k -> -inf} w(k)/|k| = b * (1 - rho) <= 2.0
/// Combined: b * (1 + |rho|) <= 2.0.
#[inline]
pub fn verify_svi_roger_lee_bounds(b: f64, rho: f64) -> bool {
    b >= 0.0 && rho.abs() < 1.0 && b * (1.0 + rho.abs()) <= 2.0
}

/// SVI implied volatility sigma(k, t) = sqrt(w(k) / t).
pub fn svi_implied_volatility(
    k: f64,
    t: f64,
    a: f64,
    b: f64,
    rho: f64,
    m: f64,
    sigma: f64,
) -> Option<f64> {
    if t <= 0.0 || !verify_svi_roger_lee_bounds(b, rho) {
        return None;
    }
    let w = svi_total_variance(k, a, b, rho, m, sigma);
    if w <= 0.0 {
        return None;
    }
    Some((w / t).sqrt().clamp(1e-4, 10.0))
}

/// Validates absence of butterfly arbitrage via Breeden-Litzenberger discrete density.
/// For sorted strike sequence K1 < K2 < ... < Kn and corresponding call prices C1, C2, ... Cn,
/// the second finite difference d²C/dK² must be >= 0 (non-negative risk-neutral density).
/// Returns Ok(()) if arbitrage-free, or Err((strike, d2c)) if butterfly arbitrage is present.
pub fn check_butterfly_arbitrage_density(
    strikes: &[f64],
    call_prices: &[f64],
) -> Result<(), (f64, f64)> {
    if strikes.len() < 3 || strikes.len() != call_prices.len() {
        return Ok(());
    }
    for i in 1..strikes.len() - 1 {
        let dk1 = strikes[i] - strikes[i - 1];
        let dk2 = strikes[i + 1] - strikes[i];
        if dk1 <= 0.0 || dk2 <= 0.0 {
            continue;
        }
        // Second finite difference approximation of d²C/dK²
        let slope_right = (call_prices[i + 1] - call_prices[i]) / dk2;
        let slope_left = (call_prices[i] - call_prices[i - 1]) / dk1;
        let d2c = (slope_right - slope_left) / (0.5 * (dk1 + dk2));
        if d2c < -1e-6 {
            return Err((strikes[i], d2c));
        }
    }
    Ok(())
}

/// Parameters resulting from Levenberg-Marquardt SABR smile calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct SabrCalibratedParams {
    pub alpha: f64,
    pub rho: f64,
    pub nu: f64,
    pub rmse: f64,
    pub iterations: usize,
    pub converged: bool,
}

/// Compact 3x3 symmetric positive-definite linear solver via Cholesky decomposition.
/// Solves A * x = b in closed form with zero heap allocation.
#[allow(clippy::needless_range_loop)]
fn solve_3x3_spd(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let mut l = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..=i {
            let mut sum = 0.0;
            for k in 0..j {
                sum += l[i][k] * l[j][k];
            }
            if i == j {
                let diag = a[i][i] - sum;
                if diag <= 1e-14 {
                    return None;
                }
                l[i][j] = diag.sqrt();
            } else {
                let diag = l[j][j];
                if diag <= 1e-14 {
                    return None;
                }
                l[i][j] = (a[i][j] - sum) / diag;
            }
        }
    }
    // Forward substitution L * y = b
    let mut y = [0.0; 3];
    for i in 0..3 {
        let mut sum = 0.0;
        for k in 0..i {
            sum += l[i][k] * y[k];
        }
        y[i] = (b[i] - sum) / l[i][i];
    }
    // Back substitution L^T * x = y
    let mut x = [0.0; 3];
    for i in (0..3).rev() {
        let mut sum = 0.0;
        for k in (i + 1)..3 {
            sum += l[k][i] * x[k];
        }
        x[i] = (y[i] - sum) / l[i][i];
    }
    Some(x)
}

/// Calibrate Hagan's SABR model (beta = 1.0) parameters (alpha, rho, nu) from live option strikes and implied volatilities
/// using the damped Levenberg-Marquardt algorithm.
///
/// Filters noisy/extreme quotes (moneyness K/F in [0.45, 2.20], IV in [0.05, 5.0]).
#[allow(clippy::needless_range_loop)]
pub fn calibrate_sabr_levenberg_marquardt(
    forward: f64,
    strikes: &[f64],
    market_ivs: &[f64],
    time_to_exp: f64,
) -> Option<SabrCalibratedParams> {
    let n = strikes.len();
    if n < 3 || n != market_ivs.len() || forward <= 0.0 || time_to_exp <= 0.0 {
        return None;
    }

    // 3-tier quote noise filtering:
    let mut valid_strikes = Vec::with_capacity(n);
    let mut valid_ivs = Vec::with_capacity(n);
    for i in 0..n {
        let k = strikes[i];
        let iv = market_ivs[i];
        let m = k / forward;
        if (0.45..=2.20).contains(&m) && (0.05..=5.0).contains(&iv) {
            valid_strikes.push(k);
            valid_ivs.push(iv);
        }
    }

    let m_len = valid_strikes.len();
    if m_len < 3 {
        return None;
    }

    let mid_idx = m_len / 2;
    let mut alpha = valid_ivs[mid_idx].clamp(0.10, 2.50);
    let mut rho = -0.25;
    let mut nu = 0.80;
    let mut lambda = 1e-3;

    let mut best_rmse = f64::MAX;
    let max_iter = 25;
    let tol = 1e-5;

    for iter in 0..max_iter {
        let mut jt_j = [[0.0; 3]; 3];
        let mut jt_r = [0.0; 3];
        let mut sum_sq_err = 0.0;

        for i in 0..m_len {
            let k = valid_strikes[i];
            let mkt_iv = valid_ivs[i];
            let model_iv = sabr_implied_volatility(forward, k, time_to_exp, alpha, 1.0, rho, nu)
                .unwrap_or(alpha);

            let r_i = model_iv - mkt_iv;
            sum_sq_err += r_i * r_i;

            let h = 1e-5;
            let iv_a = sabr_implied_volatility(forward, k, time_to_exp, alpha + h, 1.0, rho, nu)
                .unwrap_or(model_iv);
            let iv_r = sabr_implied_volatility(
                forward,
                k,
                time_to_exp,
                alpha,
                1.0,
                (rho + h).min(0.99),
                nu,
            )
            .unwrap_or(model_iv);
            let iv_n = sabr_implied_volatility(forward, k, time_to_exp, alpha, 1.0, rho, nu + h)
                .unwrap_or(model_iv);

            let j0 = (iv_a - model_iv) / h;
            let j1 = (iv_r - model_iv) / h;
            let j2 = (iv_n - model_iv) / h;
            let j_row = [j0, j1, j2];

            for r in 0..3 {
                for c in 0..3 {
                    jt_j[r][c] += j_row[r] * j_row[c];
                }
                jt_r[r] += j_row[r] * r_i;
            }
        }

        let rmse = (sum_sq_err / (m_len as f64)).sqrt();
        best_rmse = rmse;

        if rmse < tol {
            return Some(SabrCalibratedParams {
                alpha,
                rho,
                nu,
                rmse,
                iterations: iter,
                converged: true,
            });
        }

        let mut damped_a = jt_j;
        for d in 0..3 {
            damped_a[d][d] *= 1.0 + lambda;
            damped_a[d][d] += 1e-6;
        }

        let rhs = [-jt_r[0], -jt_r[1], -jt_r[2]];

        if let Some(delta) = solve_3x3_spd(damped_a, rhs) {
            let next_alpha = (alpha + delta[0]).clamp(0.01, 5.0);
            let next_rho = (rho + delta[1]).clamp(-0.99, 0.99);
            let next_nu = (nu + delta[2]).clamp(0.01, 5.0);

            let mut next_sq_err = 0.0;
            for i in 0..m_len {
                let k = valid_strikes[i];
                let mkt_iv = valid_ivs[i];
                let m_iv = sabr_implied_volatility(
                    forward,
                    k,
                    time_to_exp,
                    next_alpha,
                    1.0,
                    next_rho,
                    next_nu,
                )
                .unwrap_or(next_alpha);
                let diff = m_iv - mkt_iv;
                next_sq_err += diff * diff;
            }
            let next_rmse = (next_sq_err / (m_len as f64)).sqrt();

            if next_rmse < rmse {
                alpha = next_alpha;
                rho = next_rho;
                nu = next_nu;
                lambda = (lambda / 3.0).max(1e-7);
            } else {
                lambda = (lambda * 5.0).min(1e5);
            }
        } else {
            lambda = (lambda * 5.0).min(1e5);
        }
    }

    Some(SabrCalibratedParams {
        alpha,
        rho,
        nu,
        rmse: best_rmse,
        iterations: max_iter,
        converged: false,
    })
}

/// Computes continuous risk-neutral probability density q(K) = e^{rT} d²C/dK²
/// from the SABR implied volatility smile using numerical second derivatives.
pub fn compute_breeden_litzenberger_density(
    forward: f64,
    strike: f64,
    time_to_exp: f64,
    alpha: f64,
    rho: f64,
    nu: f64,
) -> Option<f64> {
    if forward <= 0.0 || strike <= 0.0 || time_to_exp <= 0.0 {
        return None;
    }
    let sigma = sabr_implied_volatility(forward, strike, time_to_exp, alpha, 1.0, rho, nu)?;
    let sqrt_t = time_to_exp.sqrt();
    if sigma <= 0.0 || sqrt_t <= 0.0 {
        return None;
    }
    let d1 = ((forward / strike).ln() + 0.5 * sigma * sigma * time_to_exp) / (sigma * sqrt_t);

    let h = (strike * 1e-4).max(1e-4);
    let sig_up = sabr_implied_volatility(forward, strike + h, time_to_exp, alpha, 1.0, rho, nu)?;
    let sig_dn = sabr_implied_volatility(forward, strike - h, time_to_exp, alpha, 1.0, rho, nu)?;
    let d_sig = (sig_up - sig_dn) / (2.0 * h);
    let d2_sig = (sig_up - 2.0 * sigma + sig_dn) / (h * h);

    let term1 = 1.0 + strike * d1 * sqrt_t * d_sig;
    let poly = term1 * term1 - (strike * strike * time_to_exp * d_sig * d_sig)
        + (strike * strike * sigma * sqrt_t * d2_sig);

    let phi_d1 = (-0.5 * d1 * d1).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let density = (phi_d1 / (strike * sigma * sqrt_t)) * poly;

    Some(density)
}

/// Verifies that risk-neutral density is strictly non-negative across candidate strikes.
pub fn check_sabr_butterfly_arbitrage_free(
    forward: f64,
    strikes: &[f64],
    time_to_exp: f64,
    alpha: f64,
    rho: f64,
    nu: f64,
) -> bool {
    for &k in strikes {
        if let Some(density) =
            compute_breeden_litzenberger_density(forward, k, time_to_exp, alpha, rho, nu)
        {
            if density < -1e-6 {
                return false;
            }
        }
    }
    true
}

/// Leland (1985) transaction-cost adjusted option replication engine.
///
/// Under discrete delta-hedging at frequency dt paying proportional transaction cost k:
/// - Selling an option / short gamma (buying high, selling low):
///   $$\hat{\sigma}_{\text{short}}^2 = \sigma^2 \left(1 + \sqrt{\frac{2}{\pi}} \frac{k}{\sigma \sqrt{\Delta t}}\right)$$
/// - Buying an option / long gamma (buying low, selling high):
///   $$\hat{\sigma}_{\text{long}}^2 = \sigma^2 \left(1 - \sqrt{\frac{2}{\pi}} \frac{k}{\sigma \sqrt{\Delta t}}\right)$$
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LelandOptionReplicator {
    pub taker_fee_rate: f64,
    pub rebalance_dt_sec: f64,
}

impl Default for LelandOptionReplicator {
    fn default() -> Self {
        Self {
            taker_fee_rate: 0.0005, // 5 bps standard taker fee
            rebalance_dt_sec: 60.0, // 1-minute discrete hedging interval
        }
    }
}

impl LelandOptionReplicator {
    pub fn new(taker_fee_rate: f64, rebalance_dt_sec: f64) -> Self {
        Self {
            taker_fee_rate: taker_fee_rate.max(1e-6),
            rebalance_dt_sec: rebalance_dt_sec.max(1.0),
        }
    }

    /// Computes Leland transaction-cost adjusted volatility.
    pub fn compute_leland_volatility(&self, base_annual_vol: f64, is_long_gamma: bool) -> f64 {
        let seconds_per_year = 365.25 * 86_400.0;
        let dt_years = self.rebalance_dt_sec / seconds_per_year;
        let sigma = base_annual_vol.max(1e-4);

        let factor =
            (2.0 / std::f64::consts::PI).sqrt() * (self.taker_fee_rate / (sigma * dt_years.sqrt()));

        let leland_var = if is_long_gamma {
            (sigma * sigma * (1.0 - factor)).max(1e-6)
        } else {
            sigma * sigma * (1.0 + factor)
        };

        leland_var.sqrt()
    }

    /// Computes fair synthetic option price with Leland transaction cost adjustment.
    #[allow(clippy::too_many_arguments)]
    pub fn synthetic_price(
        &self,
        option_type: OptionType,
        spot: f64,
        strike: f64,
        time_to_exp: f64,
        r: f64,
        base_annual_vol: f64,
        is_long_gamma: bool,
    ) -> Option<f64> {
        let adj_vol = self.compute_leland_volatility(base_annual_vol, is_long_gamma);
        black_scholes_price(option_type, spot, strike, time_to_exp, r, adj_vol)
    }

    /// Computes synthetic option Delta under Leland volatility.
    #[allow(clippy::too_many_arguments)]
    pub fn synthetic_delta(
        &self,
        option_type: OptionType,
        spot: f64,
        strike: f64,
        time_to_exp: f64,
        r: f64,
        base_annual_vol: f64,
        is_long_gamma: bool,
    ) -> Option<f64> {
        let adj_vol = self.compute_leland_volatility(base_annual_vol, is_long_gamma);
        let greeks = black_scholes_greeks(option_type, spot, strike, time_to_exp, r, adj_vol)?;
        Some(greeks.delta)
    }

    /// Computes target perpetual futures replication contracts for an option quantity Q:
    /// Q_perp = Q_opt * Delta
    #[allow(clippy::too_many_arguments)]
    pub fn target_perp_replication_qty(
        &self,
        option_type: OptionType,
        contracts: f64,
        spot: f64,
        strike: f64,
        time_to_exp: f64,
        r: f64,
        base_annual_vol: f64,
    ) -> Option<f64> {
        let delta = self.synthetic_delta(
            option_type,
            spot,
            strike,
            time_to_exp,
            r,
            base_annual_vol,
            true,
        )?;
        Some(contracts * delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_black_scholes_call_put_parity() {
        let s = 100.0;
        let k = 105.0;
        let t = 0.5;
        let r = 0.05;
        let sigma = 0.25;

        let call = black_scholes_price(OptionType::Call, s, k, t, r, sigma).expect("call");
        let put = black_scholes_price(OptionType::Put, s, k, t, r, sigma).expect("put");

        // C - P = S - K * exp(-r*T)
        let left = call - put;
        let right = s - k * (-r * t).exp();
        assert!(
            (left - right).abs() < 1e-6,
            "Parity check: {left} vs {right}"
        );
    }

    #[test]
    fn test_black_scholes_greeks_finite_difference() {
        let s = 100.0;
        let k = 100.0;
        let t = 1.0;
        let r = 0.03;
        let sigma = 0.20;

        let greeks = black_scholes_greeks(OptionType::Call, s, k, t, r, sigma).expect("greeks");

        // Delta check: (P(S+h) - P(S-h)) / (2*h)
        let h_s = 1e-4;
        let p_up = black_scholes_price(OptionType::Call, s + h_s, k, t, r, sigma).unwrap();
        let p_down = black_scholes_price(OptionType::Call, s - h_s, k, t, r, sigma).unwrap();
        let num_delta = (p_up - p_down) / (2.0 * h_s);
        assert!((greeks.delta - num_delta).abs() < 1e-4, "Delta match");

        // Gamma check: (P(S+h) - 2*P(S) + P(S-h)) / h²
        let h_g = 0.05;
        let p_g_up = black_scholes_price(OptionType::Call, s + h_g, k, t, r, sigma).unwrap();
        let p_g_down = black_scholes_price(OptionType::Call, s - h_g, k, t, r, sigma).unwrap();
        let num_gamma = (p_g_up - 2.0 * greeks.price + p_g_down) / (h_g * h_g);
        assert!((greeks.gamma - num_gamma).abs() < 1e-3, "Gamma match");

        // Vega check: (P(sigma+h) - P(sigma-h)) / (2*h)
        let h_v = 1e-4;
        let p_vup = black_scholes_price(OptionType::Call, s, k, t, r, sigma + h_v).unwrap();
        let p_vdown = black_scholes_price(OptionType::Call, s, k, t, r, sigma - h_v).unwrap();
        let num_vega = (p_vup - p_vdown) / (2.0 * h_v);
        assert!((greeks.vega - num_vega).abs() < 1e-4, "Vega match");
    }

    #[test]
    fn test_implied_volatility_roundtrip() {
        let cases = [
            (OptionType::Call, 100.0, 100.0, 1.0, 0.05, 0.20),
            (OptionType::Call, 100.0, 120.0, 0.5, 0.02, 0.35),
            (OptionType::Call, 100.0, 80.0, 0.25, 0.01, 0.15),
            (OptionType::Put, 100.0, 100.0, 1.0, 0.05, 0.25),
            (OptionType::Put, 50000.0, 48000.0, 0.08, 0.04, 0.65), // Crypto BTC style
            (OptionType::Call, 3000.0, 3200.0, 0.15, 0.03, 0.75),  // Crypto ETH style
        ];

        for (opt_type, s, k, t, r, true_sigma) in cases {
            let mkt_price = black_scholes_price(opt_type, s, k, t, r, true_sigma).expect("price");
            let solved_iv = implied_volatility(opt_type, mkt_price, s, k, t, r)
                .expect("implied volatility solver");
            assert!(
                (solved_iv - true_sigma).abs() < 1e-4,
                "IV roundtrip failed: got {solved_iv} expected {true_sigma} for {:?}",
                (opt_type, s, k, t, r)
            );
        }
    }

    #[test]
    fn test_invalid_arbitrage_bounds_fail_closed() {
        // Price less than intrinsic
        assert!(implied_volatility(OptionType::Call, 5.0, 100.0, 90.0, 1.0, 0.0).is_none());
        // Price above spot
        assert!(implied_volatility(OptionType::Call, 105.0, 100.0, 90.0, 1.0, 0.0).is_none());
        // Negative inputs
        assert!(implied_volatility(OptionType::Call, -1.0, 100.0, 90.0, 1.0, 0.0).is_none());
    }

    #[test]
    fn test_extreme_volatilities_roundtrip() {
        let extreme_cases = [
            (OptionType::Call, 100.0, 100.0, 1.0, 0.0, 0.02), // Very low vol (2%)
            (OptionType::Call, 100.0, 100.0, 0.5, 0.05, 3.5), // High vol (350%)
            (OptionType::Call, 100.0, 100.0, 0.25, 0.0, 10.0), // Extreme crypto crash vol (1000%)
            (OptionType::Put, 100.0, 100.0, 0.5, 0.02, 0.03), // Low vol Put
            (OptionType::Put, 100.0, 100.0, 0.1, 0.01, 5.0),  // High vol Put
        ];

        for (opt_type, s, k, t, r, true_sigma) in extreme_cases {
            let mkt_price = black_scholes_price(opt_type, s, k, t, r, true_sigma).expect("price");
            let solved_iv = implied_volatility(opt_type, mkt_price, s, k, t, r)
                .expect("extreme implied volatility solver");
            assert!(
                (solved_iv - true_sigma).abs() < 1e-3,
                "Extreme IV roundtrip failed: got {solved_iv} expected {true_sigma}"
            );
        }
    }

    #[test]
    fn test_standard_normal_cdf_tail_precision() {
        let cdf_neg7 = standard_normal_cdf(-7.0);
        assert!(cdf_neg7 > 0.0 && cdf_neg7 < 1e-11);
        let cdf_pos7 = standard_normal_cdf(7.0);
        assert!((cdf_pos7 - (1.0 - cdf_neg7)).abs() < 1e-14);
    }

    #[test]
    fn test_sabr_implied_volatility_atm_and_skew() {
        let f = 100.0;
        let t = 0.5;
        let alpha = 0.30;
        let beta = 1.0; // lognormal crypto style
        let rho = -0.25;
        let nu = 0.50;

        // ATM case
        let atm_iv = sabr_implied_volatility(f, f, t, alpha, beta, rho, nu).expect("ATM SABR");
        assert!(
            (atm_iv - 0.30).abs() < 0.05,
            "ATM SABR should be close to alpha: {atm_iv}"
        );

        // OTM Put (K < F) with negative rho should exhibit higher IV (skew)
        let otm_put_k = 85.0;
        let otm_put_iv =
            sabr_implied_volatility(f, otm_put_k, t, alpha, beta, rho, nu).expect("OTM Put SABR");
        assert!(
            otm_put_iv > atm_iv,
            "Negative rho implies higher OTM put vol: {otm_put_iv} > {atm_iv}"
        );

        // OTM Call (K > F)
        let otm_call_k = 115.0;
        let otm_call_iv =
            sabr_implied_volatility(f, otm_call_k, t, alpha, beta, rho, nu).expect("OTM Call SABR");
        assert!(otm_call_iv.is_finite() && otm_call_iv > 0.0);
    }

    #[test]
    fn test_svi_total_variance_and_roger_lee_bounds() {
        // Valid SVI parameters
        let a = 0.04;
        let b = 0.15;
        let rho = -0.30;
        let m = 0.0;
        let sigma = 0.10;

        assert!(verify_svi_roger_lee_bounds(b, rho));
        // Violating Roger Lee bounds: b * (1 + |rho|) > 2.0
        assert!(!verify_svi_roger_lee_bounds(1.8, 0.5)); // 1.8 * 1.5 = 2.7 > 2.0

        let k = 0.1; // 10% OTM call log-moneyness
        let t = 0.25; // 3 months
        let w = svi_total_variance(k, a, b, rho, m, sigma);
        assert!(w > 0.0);

        let svi_iv = svi_implied_volatility(k, t, a, b, rho, m, sigma).expect("SVI IV");
        assert!(svi_iv > 0.0 && svi_iv.is_finite());
        assert!((svi_iv - (w / t).sqrt()).abs() < 1e-10);
    }

    #[test]
    fn test_check_butterfly_arbitrage_density() {
        let strikes = [80.0, 90.0, 100.0, 110.0, 120.0];

        // Arbitrage-free convex call prices (e.g. from BS)
        let s = 100.0;
        let t = 0.5;
        let r = 0.03;
        let sigma = 0.30;
        let valid_prices: Vec<f64> = strikes
            .iter()
            .map(|&k| black_scholes_price(OptionType::Call, s, k, t, r, sigma).unwrap())
            .collect();

        assert!(
            check_butterfly_arbitrage_density(&strikes, &valid_prices).is_ok(),
            "Standard Black-Scholes call prices must be arbitrage-free"
        );

        // Fabricate non-convex prices (middle strike overpriced -> negative density)
        let mut toxic_prices = valid_prices.clone();
        toxic_prices[2] += 5.0; // Artificial bump in 100 strike call price
        let err = check_butterfly_arbitrage_density(&strikes, &toxic_prices);
        assert!(
            err.is_err(),
            "Artificially elevated middle strike must fail butterfly arbitrage density check"
        );
        let (bad_k, d2c) = err.unwrap_err();
        assert_eq!(bad_k, 100.0);
        assert!(d2c < 0.0);
    }

    #[test]
    fn test_calibrate_sabr_levenberg_marquardt() {
        let forward = 100.0;
        let time_to_exp = 0.25; // 3 months
        let true_alpha = 0.50;
        let true_rho = -0.25;
        let true_nu = 0.75;

        let strikes = [75.0, 85.0, 95.0, 100.0, 105.0, 115.0, 125.0];
        let market_ivs: Vec<f64> = strikes
            .iter()
            .map(|&k| {
                sabr_implied_volatility(forward, k, time_to_exp, true_alpha, 1.0, true_rho, true_nu)
                    .unwrap()
            })
            .collect();

        let calib = calibrate_sabr_levenberg_marquardt(forward, &strikes, &market_ivs, time_to_exp)
            .expect("SABR calibration should succeed");

        assert!(
            calib.rmse < 0.01,
            "Calibrated RMSE should be < 0.01, got {}",
            calib.rmse
        );
        assert!(
            (calib.alpha - true_alpha).abs() < 0.05,
            "Alpha should be close to true alpha: {} vs {}",
            calib.alpha,
            true_alpha
        );
        assert!(
            (calib.rho - true_rho).abs() < 0.15,
            "Rho should be close to true rho: {} vs {}",
            calib.rho,
            true_rho
        );
    }

    #[test]
    fn test_compute_breeden_litzenberger_density() {
        let forward = 100.0;
        let time_to_exp = 0.5;
        let alpha = 0.40;
        let rho = -0.15;
        let nu = 0.50;

        let strikes = [80.0, 90.0, 100.0, 110.0, 120.0];
        for &k in &strikes {
            let density =
                compute_breeden_litzenberger_density(forward, k, time_to_exp, alpha, rho, nu)
                    .expect("Density should exist");
            assert!(
                density > 0.0,
                "Density at strike {} must be positive, got {}",
                k,
                density
            );
        }

        assert!(check_sabr_butterfly_arbitrage_free(
            forward,
            &strikes,
            time_to_exp,
            alpha,
            rho,
            nu
        ));
    }

    #[test]
    fn test_leland_synthetic_options_replication() {
        let replicator = LelandOptionReplicator::new(0.0005, 60.0); // 5 bps, 60s
        let base_vol = 0.60;

        // Short gamma: buying high and selling low raises effective volatility
        let vol_short = replicator.compute_leland_volatility(base_vol, false);
        assert!(vol_short > base_vol);

        // Long gamma: buying low and selling high lowers effective volatility (friction deducted from edge)
        let vol_long = replicator.compute_leland_volatility(base_vol, true);
        assert!(vol_long < base_vol);

        let spot = 2500.0;
        let strike = 2500.0;
        let t = 30.0 / 365.25;
        let r = 0.05;

        let call_price = replicator
            .synthetic_price(OptionType::Call, spot, strike, t, r, base_vol, false)
            .unwrap();
        assert!(call_price > 0.0);

        let delta = replicator
            .synthetic_delta(OptionType::Call, spot, strike, t, r, base_vol, true)
            .unwrap();
        assert!(delta > 0.40 && delta < 0.60); // ATM delta ~ 0.50

        let hedge_qty = replicator
            .target_perp_replication_qty(OptionType::Call, 10.0, spot, strike, t, r, base_vol)
            .unwrap();
        assert!((hedge_qty - (10.0 * delta)).abs() < 1e-6);
    }
}
