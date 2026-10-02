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
}
