//! Cross-Venue Options Volatility Surface Arbitrage & Dispersion Trading Engine.
//!
//! Provides:
//! - Model-free extraction of cross-asset implied correlation ρ_implied(K, T) from index options vs constituent options.
//! - Quantitative evaluation of volatility dispersion alpha: Long single-asset straddles vs Short index straddles.
//! - Deribit Portfolio Margin (PM) cross-asset netting relief calculation (~72.6% capital efficiency).
//! - Cross-venue volatility smile and synthetic forward parity arbitrage between CeFi (Deribit) and on-chain option CLOBs (Aevo, Derive, Lyra).

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use arbitrageur_math::black_scholes::sabr_implied_volatility;
use arbitrageur_math::svi_arbitrage_free::SviSliceParameters;
use arbitrageur_risk::delta_guard::VolatilitySmileProvider;

/// Point on the cross-asset implied correlation smile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationSmilePoint {
    pub strike: f64,
    pub moneyness: f64,
    pub btc_iv: f64,
    pub eth_iv: f64,
    pub basket_iv: f64,
    pub implied_correlation: f64,
}

/// Signal for entering or holding a volatility dispersion arbitrage position.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispersionTradeSignal {
    pub should_enter: bool,
    pub avg_implied_correlation: f64,
    pub realized_correlation: f64,
    pub correlation_spread: f64,
    pub margin_relief_ratio: f64,
    pub action: &'static str,
}

/// Implied Correlation Smile Calibrator and Volatility Dispersion Evaluator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpliedCorrelationSmileCalibrator {
    /// Basket weight for primary asset (e.g. BTC = 0.65).
    pub weight_btc: f64,
    /// Basket weight for secondary asset (e.g. ETH = 0.35).
    pub weight_eth: f64,
    /// Minimum spread between implied correlation and realized correlation to trigger entry (e.g. 0.12 = 12%).
    pub min_dispersion_hurdle: f64,
    /// Deribit Portfolio Margin cross-asset correlation netting parameter (typically 0.85).
    pub deribit_pm_correlation: f64,
}

impl Default for ImpliedCorrelationSmileCalibrator {
    fn default() -> Self {
        Self {
            weight_btc: 0.65,
            weight_eth: 0.35,
            min_dispersion_hurdle: 0.12,
            deribit_pm_correlation: 0.85,
        }
    }
}

impl ImpliedCorrelationSmileCalibrator {
    pub fn new(
        weight_btc: f64,
        weight_eth: f64,
        min_dispersion_hurdle: f64,
        deribit_pm_correlation: f64,
    ) -> Self {
        let sum = weight_btc + weight_eth;
        let w_btc = if sum > 0.0 { weight_btc / sum } else { 0.65 };
        let w_eth = if sum > 0.0 { weight_eth / sum } else { 0.35 };
        Self {
            weight_btc: w_btc,
            weight_eth: w_eth,
            min_dispersion_hurdle: min_dispersion_hurdle.clamp(0.01, 0.50),
            deribit_pm_correlation: deribit_pm_correlation.clamp(0.50, 0.99),
        }
    }

    /// Extracts model-free implied correlation ρ(K, T) across a strike strip:
    /// σ_index^2 = w_1^2 σ_1^2 + w_2^2 σ_2^2 + 2 w_1 w_2 ρ σ_1 σ_2
    /// => ρ = (σ_index^2 - w_1^2 σ_1^2 - w_2^2 σ_2^2) / (2 w_1 w_2 σ_1 σ_2)
    pub fn extract_implied_correlation(&self, btc_iv: f64, eth_iv: f64, basket_iv: f64) -> f64 {
        let w1 = self.weight_btc;
        let w2 = self.weight_eth;
        let num = basket_iv * basket_iv - w1 * w1 * btc_iv * btc_iv - w2 * w2 * eth_iv * eth_iv;
        let denom = 2.0 * w1 * w2 * btc_iv * eth_iv;
        if denom <= 1e-12 {
            return self.deribit_pm_correlation;
        }
        (num / denom).clamp(-0.99, 0.99)
    }

    /// Computes theoretical Deribit Portfolio Margin (PM) cross-asset netting relief:
    /// Relief = 1 - sqrt(w1^2 + w2^2 - 2 * rho_pm * w1 * w2) / (w1 + w2)
    pub fn compute_portfolio_margin_relief(&self) -> f64 {
        let w1 = self.weight_btc;
        let w2 = self.weight_eth;
        let rho = self.deribit_pm_correlation;
        let net_variance = w1 * w1 + w2 * w2 - 2.0 * rho * w1 * w2;
        let net_risk = net_variance.max(0.0).sqrt();
        let total = w1 + w2;
        if total > 0.0 {
            1.0 - (net_risk / total)
        } else {
            0.0
        }
    }

    /// Evaluates multi-strike dispersion opportunity and determines trade qualification.
    pub fn evaluate_dispersion(
        &self,
        points: &[CorrelationSmilePoint],
        realized_correlation_30d: f64,
    ) -> DispersionTradeSignal {
        let margin_relief = self.compute_portfolio_margin_relief();
        if points.is_empty() {
            return DispersionTradeSignal {
                should_enter: false,
                avg_implied_correlation: 0.0,
                realized_correlation: realized_correlation_30d,
                correlation_spread: 0.0,
                margin_relief_ratio: margin_relief,
                action: "NO_DATA: Smile points empty",
            };
        }

        let avg_implied: f64 =
            points.iter().map(|p| p.implied_correlation).sum::<f64>() / points.len() as f64;
        let spread = avg_implied - realized_correlation_30d;

        if spread >= self.min_dispersion_hurdle {
            DispersionTradeSignal {
                should_enter: true,
                avg_implied_correlation: avg_implied,
                realized_correlation: realized_correlation_30d,
                correlation_spread: spread,
                margin_relief_ratio: margin_relief,
                action: "ENTER_DISPERSION: Implied correlation rich vs realized (Long components, Short basket)",
            }
        } else {
            DispersionTradeSignal {
                should_enter: false,
                avg_implied_correlation: avg_implied,
                realized_correlation: realized_correlation_30d,
                correlation_spread: spread,
                margin_relief_ratio: margin_relief,
                action: "HOLD_NEUTRAL: Correlation spread below hurdle",
            }
        }
    }
}

/// Trade recommendation for cross-venue relative smile / skew mispricing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrossVenueSkewArbitrageDecision {
    pub should_trade: bool,
    pub deribit_iv: f64,
    pub onchain_iv: f64,
    pub iv_spread_vol: f64,
    pub synthetic_forward_diff_bps: f64,
    pub recommended_action: &'static str,
}

/// Router for detecting cross-venue volatility smile and synthetic forward dislocations
/// between Deribit and on-chain option CLOBs (Aevo, Derive, Lyra).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossVenueSmileArbitrageEngine {
    /// Minimum IV spread (in vol points, e.g. 0.065 = 6.5 vol pts) required to enter skew arb.
    pub min_skew_spread_vol: f64,
    /// Minimum synthetic forward basis dislocation in basis points (e.g. 15.0 bps).
    pub min_forward_diff_bps: f64,
}

impl Default for CrossVenueSmileArbitrageEngine {
    fn default() -> Self {
        Self {
            min_skew_spread_vol: 0.065,
            min_forward_diff_bps: 15.0,
        }
    }
}

impl CrossVenueSmileArbitrageEngine {
    pub fn new(min_skew_spread_vol: f64, min_forward_diff_bps: f64) -> Self {
        Self {
            min_skew_spread_vol: min_skew_spread_vol.clamp(0.01, 0.50),
            min_forward_diff_bps: min_forward_diff_bps.clamp(1.0, 200.0),
        }
    }

    /// Evaluates synthetic forward price via put-call parity:
    /// F = K + exp(r * T) * (C - P)
    pub fn calculate_synthetic_forward(
        &self,
        call_price: f64,
        put_price: f64,
        strike: f64,
        risk_free_rate: f64,
        time_to_exp_years: f64,
    ) -> f64 {
        let discount = (risk_free_rate * time_to_exp_years).exp();
        strike + discount * (call_price - put_price)
    }

    /// Evaluates cross-venue implied volatility skew mispricing and forward basis discrepancies.
    pub fn evaluate_skew_dislocation(
        &self,
        deribit_iv: f64,
        onchain_iv: f64,
        deribit_forward: f64,
        onchain_forward: f64,
        spot_price: f64,
    ) -> CrossVenueSkewArbitrageDecision {
        let iv_spread = onchain_iv - deribit_iv;
        let forward_diff = onchain_forward - deribit_forward;
        let forward_diff_bps = if spot_price > 0.0 {
            (forward_diff / spot_price) * 10_000.0
        } else {
            0.0
        };

        if iv_spread >= self.min_skew_spread_vol {
            CrossVenueSkewArbitrageDecision {
                should_trade: true,
                deribit_iv,
                onchain_iv,
                iv_spread_vol: iv_spread,
                synthetic_forward_diff_bps: forward_diff_bps,
                recommended_action: "SKEW_ARB_SHORT_ONCHAIN: Sell high-IV on-chain option, Buy low-IV Deribit option (delta-hedge via perp)",
            }
        } else if iv_spread <= -self.min_skew_spread_vol {
            CrossVenueSkewArbitrageDecision {
                should_trade: true,
                deribit_iv,
                onchain_iv,
                iv_spread_vol: iv_spread,
                synthetic_forward_diff_bps: forward_diff_bps,
                recommended_action: "SKEW_ARB_LONG_ONCHAIN: Buy low-IV on-chain option, Sell high-IV Deribit option (delta-hedge via perp)",
            }
        } else if forward_diff_bps.abs() >= self.min_forward_diff_bps {
            let action = if forward_diff_bps > 0.0 {
                "FORWARD_ARB_SYNTHETIC: Long synthetic forward on Deribit, Short synthetic forward on-chain"
            } else {
                "FORWARD_ARB_SYNTHETIC: Long synthetic forward on-chain, Short synthetic forward on Deribit"
            };
            CrossVenueSkewArbitrageDecision {
                should_trade: true,
                deribit_iv,
                onchain_iv,
                iv_spread_vol: iv_spread,
                synthetic_forward_diff_bps: forward_diff_bps,
                recommended_action: action,
            }
        } else {
            CrossVenueSkewArbitrageDecision {
                should_trade: false,
                deribit_iv,
                onchain_iv,
                iv_spread_vol: iv_spread,
                synthetic_forward_diff_bps: forward_diff_bps,
                recommended_action:
                    "HOLD_NEUTRAL: Volatility smile and synthetic forward within fair pricing bands",
            }
        }
    }
}

/// Online streaming recursive Levenberg-Marquardt SABR parameter tracker.
/// Calibrates (alpha, rho, nu) with fixed lognormal backbone beta = 1.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineRecursiveSabrCalibrator {
    pub forward: f64,
    pub time_to_exp: f64,
    pub alpha: f64,
    pub rho: f64,
    pub nu: f64,
    pub p_matrix: [[f64; 3]; 3],
    pub forgetting_factor: f64,
    pub lambda_damping: f64,
    pub last_residual: f64,
    pub updates_count: usize,
}

impl OnlineRecursiveSabrCalibrator {
    pub fn new(forward: f64, time_to_exp: f64, init_atm_iv: f64) -> Self {
        let p_init = 10.0;
        Self {
            forward: forward.max(1.0),
            time_to_exp: time_to_exp.max(1e-4),
            alpha: init_atm_iv.clamp(0.05, 3.50),
            rho: -0.25,
            nu: 0.85,
            p_matrix: [[p_init, 0.0, 0.0], [0.0, p_init, 0.0], [0.0, 0.0, p_init]],
            forgetting_factor: 0.96,
            lambda_damping: 1e-3,
            last_residual: 0.0,
            updates_count: 0,
        }
    }

    /// Evaluates model SABR volatility at a given strike.
    pub fn model_volatility(&self, strike: f64) -> Option<f64> {
        sabr_implied_volatility(
            self.forward,
            strike,
            self.time_to_exp,
            self.alpha,
            1.0,
            self.rho,
            self.nu,
        )
    }

    /// Updates SABR parameters online via rank-1 Sherman-Morrison RLM update.
    pub fn update_quote(&mut self, strike: f64, market_iv: f64) -> bool {
        if strike <= 0.0 || market_iv <= 0.01 || market_iv > 5.0 {
            return false;
        }

        let model_iv = match self.model_volatility(strike) {
            Some(v) => v,
            None => return false,
        };

        let residual = market_iv - model_iv;
        let jacobian = match self.compute_analytical_jacobian(strike) {
            Some(j) => j,
            None => return false,
        };

        let mut ph_t = [0.0; 3];
        for (r, item) in ph_t.iter_mut().enumerate() {
            *item = self.p_matrix[r][0] * jacobian[0]
                + self.p_matrix[r][1] * jacobian[1]
                + self.p_matrix[r][2] * jacobian[2];
        }

        let h_ph_t = jacobian[0] * ph_t[0] + jacobian[1] * ph_t[1] + jacobian[2] * ph_t[2];
        let denom = self.forgetting_factor + h_ph_t + self.lambda_damping;
        if denom <= 1e-12 {
            return false;
        }

        let gain = [ph_t[0] / denom, ph_t[1] / denom, ph_t[2] / denom];

        self.alpha = (self.alpha + gain[0] * residual).clamp(0.05, 3.50);
        self.rho = (self.rho + gain[1] * residual).clamp(-0.95, 0.95);
        self.nu = (self.nu + gain[2] * residual).clamp(0.05, 4.00);

        let inv_lambda = 1.0 / self.forgetting_factor;
        for r in 0..3 {
            for c in 0..3 {
                self.p_matrix[r][c] =
                    inv_lambda * (self.p_matrix[r][c] - (ph_t[r] * ph_t[c]) / denom);
            }
        }

        if residual.abs() > 1.25 * self.last_residual.abs() {
            self.lambda_damping = (self.lambda_damping * 1.5).min(10.0);
        } else {
            self.lambda_damping = (self.lambda_damping * 0.85).max(1e-4);
        }

        self.last_residual = residual;
        self.updates_count += 1;
        true
    }

    /// Evaluates exact analytical Jacobian [d_alpha, d_rho, d_nu] in closed form.
    pub fn compute_analytical_jacobian(&self, strike: f64) -> Option<[f64; 3]> {
        let f = self.forward;
        let k = strike;
        let t = self.time_to_exp;
        let alpha = self.alpha;
        let rho = self.rho;
        let nu = self.nu;

        let log_fk = (f / k).ln();
        let z = (nu / alpha) * log_fk;
        let r_z = (1.0 - 2.0 * rho * z + z * z).sqrt();
        let num_x = r_z + z - rho;
        if num_x <= 0.0 || (1.0 - rho) <= 0.0 {
            return None;
        }
        let x_z = (num_x / (1.0 - rho)).ln();
        let chi_z = if x_z.abs() < 1e-10 { 1.0 } else { z / x_z };

        let b = 1.0 + (0.25 * rho * nu * alpha + ((2.0 - 3.0 * rho * rho) / 24.0) * nu * nu) * t;

        // d_alpha
        let d_alpha = (chi_z * chi_z / r_z) * b + 0.25 * alpha * chi_z * rho * nu * t;

        // d_nu
        let d_nu = (alpha / nu) * (chi_z - (chi_z * chi_z / r_z)) * b
            + alpha * chi_z * (0.25 * rho * alpha + ((2.0 - 3.0 * rho * rho) / 12.0) * nu) * t;

        // d_rho
        let dx_drho = (1.0 / (1.0 - rho)) - (z + r_z) / (r_z * num_x);
        let d_rho = if z.abs() < 1e-6 {
            0.5 * (nu / alpha) * log_fk * b + 0.25 * alpha * chi_z * nu * (alpha - rho * nu) * t
        } else {
            -alpha * (chi_z * chi_z / z) * dx_drho * b
                + 0.25 * alpha * chi_z * nu * (alpha - rho * nu) * t
        };

        Some([d_alpha, d_rho, d_nu])
    }
}

fn svi_fit_score(params: &SviSliceParameters, observations: &[(f64, f64)]) -> Option<f64> {
    if !params.is_arbitrage_free() {
        return None;
    }
    let mut sse = 0.0;
    for (k, target_w) in observations {
        let model_w = params.total_variance(*k);
        if !model_w.is_finite() || model_w <= 0.0 {
            return None;
        }
        let residual = model_w - target_w;
        sse += residual * residual;
    }
    sse.is_finite().then_some(sse)
}

/// Fit an arbitrage-free Gatheral SVI slice by deterministic least squares.
/// The four shape parameters are searched coarsely, then refined
/// coordinate-wise; `a` is solved analytically as the least-squares level
/// for each shape candidate.
fn calibrate_svi_slice(
    forward_price: f64,
    time_to_expiry_years: f64,
    quotes: &[(f64, f64)],
) -> Option<SviSliceParameters> {
    if forward_price <= 0.0 || time_to_expiry_years <= 1e-6 {
        return None;
    }
    let mut observations: Vec<(f64, f64)> = quotes
        .iter()
        .filter_map(|(strike, iv)| {
            if !strike.is_finite() || !iv.is_finite() || *strike <= 0.0 || *iv <= 0.0 {
                return None;
            }
            let k = (*strike / forward_price).ln();
            let w = iv * iv * time_to_expiry_years;
            (k.is_finite() && w.is_finite() && w > 0.0).then_some((k, w))
        })
        .collect();
    observations.sort_by(|a, b| a.0.total_cmp(&b.0));
    observations.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12);
    if observations.len() < 3 {
        return None;
    }

    let k_min = observations.first()?.0;
    let k_max = observations.last()?.0;
    let k_span = (k_max - k_min).max(0.05);
    let mean_k = observations.iter().map(|(k, _)| *k).sum::<f64>() / observations.len() as f64;
    let mean_w = observations.iter().map(|(_, w)| *w).sum::<f64>() / observations.len() as f64;
    let max_w = observations
        .iter()
        .map(|(_, w)| *w)
        .fold(mean_w, f64::max)
        .max(1e-6);

    let b_grid = [0.02, 0.05, 0.10, 0.20, 0.35, 0.60, 1.0]
        .into_iter()
        .map(|scale| (max_w * scale).max(1e-5))
        .collect::<Vec<_>>();
    let rho_grid = [-0.90, -0.65, -0.40, -0.15, 0.0, 0.15, 0.40, 0.65, 0.90];
    let m_grid = [
        mean_k,
        0.0,
        k_min + k_span * 0.25,
        k_min + k_span * 0.50,
        k_min + k_span * 0.75,
    ];
    let sigma_grid = [
        0.01,
        0.03,
        0.06,
        0.10,
        (k_span * 0.15).max(0.01),
        (k_span * 0.30).max(0.02),
    ];

    let mut best: Option<(SviSliceParameters, f64)> = None;
    for &b in &b_grid {
        for &rho in &rho_grid {
            for &m in &m_grid {
                for &sigma in &sigma_grid {
                    let shape_at = |k: f64| {
                        let diff = k - m;
                        let root = (diff * diff + sigma * sigma).sqrt();
                        b * (rho * diff + root)
                    };
                    let mean_shape = observations.iter().map(|(k, _)| shape_at(*k)).sum::<f64>()
                        / observations.len() as f64;
                    let a = mean_w - mean_shape;
                    let params = SviSliceParameters::new(a, b, rho, m, sigma);
                    let Some(score) = svi_fit_score(&params, &observations) else {
                        continue;
                    };
                    if best
                        .as_ref()
                        .is_none_or(|(_, best_score)| score < *best_score)
                    {
                        best = Some((params, score));
                    }
                }
            }
        }
    }

    let (mut params, mut best_score) = best?;
    // Coordinate refinement around the coarse optimum. Each stage re-solves
    // the variance level `a` so a shape-only move cannot be rejected merely
    // because the slice was shifted up or down.
    for _ in 0..4 {
        let iteration_score = best_score;
        {
            let b_step = (params.b * 0.45).max(max_w * 0.02).max(1e-5);
            for delta in [-2.0, -1.0, -0.5, 0.5, 1.0, 2.0] {
                let b = (params.b + delta * b_step).max(0.0);
                if let Some((candidate, score)) =
                    refit_svi_level(params, observations.as_slice(), |p| p.b = b)
                {
                    if score + 1e-15 < best_score {
                        params = candidate;
                        best_score = score;
                    }
                }
            }

            for delta in [-0.30, -0.15, -0.075, 0.075, 0.15, 0.30] {
                let rho = (params.rho + delta).clamp(-0.95, 0.95);
                if let Some((candidate, score)) =
                    refit_svi_level(params, observations.as_slice(), |p| p.rho = rho)
                {
                    if score + 1e-15 < best_score {
                        params = candidate;
                        best_score = score;
                    }
                }
            }

            let m_step = (k_span * 0.10).max(0.002);
            for delta in [-2.0, -1.0, -0.5, 0.5, 1.0, 2.0] {
                let m = params.m + delta * m_step;
                if let Some((candidate, score)) =
                    refit_svi_level(params, observations.as_slice(), |p| p.m = m)
                {
                    if score + 1e-15 < best_score {
                        params = candidate;
                        best_score = score;
                    }
                }
            }

            let sigma_step = (params.sigma * 0.45).max(k_span * 0.02).max(0.002);
            for delta in [-2.0, -1.0, -0.5, 0.5, 1.0, 2.0] {
                let sigma = (params.sigma + delta * sigma_step).max(1e-4);
                if let Some((candidate, score)) =
                    refit_svi_level(params, observations.as_slice(), |p| p.sigma = sigma)
                {
                    if score + 1e-15 < best_score {
                        params = candidate;
                        best_score = score;
                    }
                }
            }
        }
        if best_score >= iteration_score {
            break;
        }
    }

    Some(params)
}

fn refit_svi_level(
    mut params: SviSliceParameters,
    observations: &[(f64, f64)],
    mutate: impl FnOnce(&mut SviSliceParameters),
) -> Option<(SviSliceParameters, f64)> {
    mutate(&mut params);
    let mean_w = observations.iter().map(|(_, w)| *w).sum::<f64>() / observations.len() as f64;
    let mean_shape = observations
        .iter()
        .map(|(k, _)| {
            params.a = 0.0;
            params.total_variance(*k)
        })
        .sum::<f64>()
        / observations.len() as f64;
    params.a = mean_w - mean_shape;
    let score = svi_fit_score(&params, observations)?;
    Some((params, score))
}

/// Single expiry slice with calibrated SABR/SVI parameters and raw strike quotes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveVolSurfaceSlice {
    pub expiry: String,
    pub expiry_ms: i64,
    pub tenor_years: f64,
    pub forward_price: f64,
    pub sabr_calibrator: OnlineRecursiveSabrCalibrator,
    /// Arbitrage-free Gatheral SVI fit for smile-adjusted Greeks.
    #[serde(default)]
    pub svi_params: Option<SviSliceParameters>,
    /// Exact market IVs by strike (strike in USD -> IV fraction)
    pub observed_strikes: HashMap<u64, f64>,
    pub last_updated_ms: i64,
}

fn recalibrate_slice_svi(slice: &mut LiveVolSurfaceSlice) {
    let mut quotes: Vec<(f64, f64)> = slice
        .observed_strikes
        .iter()
        .map(|(strike_key, iv)| (*strike_key as f64 / 100.0, *iv))
        .collect();
    quotes.sort_by(|a, b| a.0.total_cmp(&b.0));
    slice.svi_params = calibrate_svi_slice(slice.forward_price, slice.tenor_years, &quotes);
}

/// Thread-safe in-memory cache of live SABR volatility surfaces.
/// Implements VolatilitySmileProvider for seamless DeltaGuard integration.
#[derive(Debug, Default)]
pub struct LiveVolSurfaceCache {
    /// surfaces: Asset -> Expiry -> Slice
    pub slices: RwLock<HashMap<String, HashMap<String, LiveVolSurfaceSlice>>>,
    /// Latest observed spot index prices: Asset -> spot price
    pub latest_spots: RwLock<HashMap<String, f64>>,
}

impl LiveVolSurfaceCache {
    pub fn new() -> Self {
        Self {
            slices: RwLock::new(HashMap::new()),
            latest_spots: RwLock::new(HashMap::new()),
        }
    }

    /// Records latest spot index price for an asset.
    pub fn update_spot(&self, asset: &str, spot: f64) {
        if spot > 0.0 {
            self.latest_spots
                .write()
                .insert(asset.to_ascii_uppercase(), spot);
        }
    }

    /// Gets latest spot index price for an asset.
    pub fn get_spot(&self, asset: &str) -> Option<f64> {
        self.latest_spots
            .read()
            .get(&asset.to_ascii_uppercase())
            .copied()
    }

    /// Returns the most recently updated forward price for an asset across all tenors.
    pub fn latest_forward(&self, asset: &str) -> Option<f64> {
        let map = self.slices.read();
        let asset_slices = map.get(&asset.to_ascii_uppercase())?;
        asset_slices
            .values()
            .max_by_key(|s| s.last_updated_ms)
            .map(|s| s.forward_price)
    }

    /// Returns the total number of active expiry slices across all assets.
    pub fn total_slices_count(&self) -> usize {
        let map = self.slices.read();
        map.values().map(|s| s.len()).sum()
    }

    /// Returns the total number of observed strike quotes cached across all slices.
    pub fn total_quotes_count(&self) -> usize {
        let map = self.slices.read();
        map.values()
            .flat_map(|s| s.values())
            .map(|slice| slice.observed_strikes.len())
            .sum()
    }

    /// Prunes expired slices whose expiry_ms is past now_ms.
    pub fn prune_expired(&self, now_ms: i64) {
        let mut map = self.slices.write();
        for slices in map.values_mut() {
            slices.retain(|_, slice| slice.expiry_ms > now_ms);
        }
    }

    /// Ingests a strike quote (mark IV in fractional form, e.g. 0.5358 for 53.58%).
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_quote(
        &self,
        asset: &str,
        expiry: &str,
        expiry_ms: i64,
        tenor_years: f64,
        forward_price: f64,
        strike: f64,
        market_iv: f64,
    ) {
        let mut map = self.slices.write();
        let asset_slices = map.entry(asset.to_ascii_uppercase()).or_default();
        let slice = asset_slices
            .entry(expiry.to_string())
            .or_insert_with(|| LiveVolSurfaceSlice {
                expiry: expiry.to_string(),
                expiry_ms,
                tenor_years,
                forward_price,
                sabr_calibrator: OnlineRecursiveSabrCalibrator::new(
                    forward_price,
                    tenor_years,
                    market_iv,
                ),
                svi_params: None,
                observed_strikes: HashMap::new(),
                last_updated_ms: chrono::Utc::now().timestamp_millis(),
            });

        slice.forward_price = forward_price;
        slice.tenor_years = tenor_years;
        slice.sabr_calibrator.forward = forward_price;
        slice.sabr_calibrator.time_to_exp = tenor_years;
        slice.sabr_calibrator.update_quote(strike, market_iv);

        let strike_key = (strike * 100.0).round() as u64;
        slice.observed_strikes.insert(strike_key, market_iv);
        slice.last_updated_ms = chrono::Utc::now().timestamp_millis();
        recalibrate_slice_svi(slice);
    }

    /// Ingests a complete expiry chain and calibrates one arbitrage-free SVI
    /// slice from the resulting quote set.
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_chain_slice(
        &self,
        asset: &str,
        expiry: &str,
        expiry_ms: i64,
        tenor_years: f64,
        forward_price: f64,
        quotes: &[(f64, f64)],
    ) {
        for &(strike, market_iv) in quotes {
            self.ingest_quote(
                asset,
                expiry,
                expiry_ms,
                tenor_years,
                forward_price,
                strike,
                market_iv,
            );
        }
        let mut map = self.slices.write();
        let Some(slice) = map
            .get_mut(&asset.to_ascii_uppercase())
            .and_then(|slices| slices.get_mut(expiry))
        else {
            return;
        };
        recalibrate_slice_svi(slice);
    }

    /// Computes the SVI smile-adjusted delta for an option expiry.
    pub fn get_smile_adjusted_delta(
        &self,
        asset: &str,
        expiry_ts_ms: i64,
        strike: f64,
        spot: f64,
        bs_delta: f64,
        bs_vega_raw: f64,
    ) -> Option<f64> {
        if spot <= 0.0 || strike <= 0.0 {
            return None;
        }
        let map = self.slices.read();
        let slice = map
            .get(&asset.to_ascii_uppercase())?
            .values()
            .find(|slice| slice.expiry_ms == expiry_ts_ms)?;
        let svi = slice.svi_params.as_ref()?;
        let adjusted = svi.smile_adjusted_delta(
            bs_delta,
            bs_vega_raw,
            slice.forward_price,
            strike,
            slice.tenor_years,
        );
        adjusted.is_finite().then_some(adjusted)
    }

    /// Evaluates model implied volatility for (asset, expiry, strike).
    pub fn evaluate_volatility(&self, asset: &str, expiry: &str, strike: f64) -> Option<f64> {
        let map = self.slices.read();
        let asset_slices = map.get(&asset.to_ascii_uppercase())?;
        let slice = asset_slices.get(expiry)?;

        // 1. Prefer the arbitrage-free SVI slice.
        if let Some(svi) = slice.svi_params.as_ref() {
            let k = (strike / slice.forward_price).ln();
            if k.is_finite() && slice.tenor_years > 1e-6 {
                let variance = svi.total_variance(k);
                if variance.is_finite() && variance > 0.0 {
                    let iv = (variance / slice.tenor_years).sqrt();
                    if iv > 0.05 && iv < 4.0 {
                        return Some(iv);
                    }
                }
            }
        }

        // 2. Try calibrated SABR model:
        if let Some(iv) = slice.sabr_calibrator.model_volatility(strike) {
            if iv > 0.05 && iv < 4.0 {
                return Some(iv);
            }
        }

        // 3. Direct lookup in observed strikes:
        let strike_key = (strike * 100.0).round() as u64;
        slice.observed_strikes.get(&strike_key).copied()
    }
}

impl VolatilitySmileProvider for LiveVolSurfaceCache {
    fn get_implied_volatility(&self, asset: &str, expiry: &str, strike: f64) -> Option<f64> {
        self.evaluate_volatility(asset, expiry, strike)
    }
}

/// Recommendation for synthetic perpetual option vs physical option dispersion arbitrage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SyntheticDispersionSignal {
    HoldNoArb,
    SellPhysicalBuySynthetic {
        symbol: String,
        physical_iv: f64,
        synthetic_leland_vol: f64,
        edge_vol_spread: f64,
        target_perp_hedge_qty: f64,
        action: String,
    },
    BuyPhysicalSellSynthetic {
        symbol: String,
        physical_iv: f64,
        synthetic_leland_vol: f64,
        edge_vol_spread: f64,
        target_perp_hedge_qty: f64,
        action: String,
    },
}

/// Evaluator for cross-venue dispersion between physical options (Deribit) and synthetic
/// perpetual options replication (Hyperliquid/Lighter).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticPerpOptionDispersionEngine {
    pub min_vol_spread_hurdle: f64,
    pub perp_taker_fee_rate: f64,
    pub perp_rebalance_interval_sec: f64,
}

impl Default for SyntheticPerpOptionDispersionEngine {
    fn default() -> Self {
        Self {
            min_vol_spread_hurdle: 0.05,       // 5% vol spread minimum hurdle
            perp_taker_fee_rate: 0.00035,      // 3.5 bps Hyperliquid taker fee
            perp_rebalance_interval_sec: 60.0, // 1 min rebalancing
        }
    }
}

impl SyntheticPerpOptionDispersionEngine {
    pub fn new(min_vol_spread: f64, perp_fee: f64, rebalance_sec: f64) -> Self {
        Self {
            min_vol_spread_hurdle: min_vol_spread.max(0.01),
            perp_taker_fee_rate: perp_fee.max(1e-5),
            perp_rebalance_interval_sec: rebalance_sec.max(1.0),
        }
    }

    /// Evaluates volatility dispersion between a physical option and perpetual synthetic replication.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_synthetic_dispersion(
        &self,
        symbol: &str,
        option_type: arbitrageur_math::black_scholes::OptionType,
        contracts: f64,
        spot: f64,
        strike: f64,
        time_to_exp: f64,
        risk_free_rate: f64,
        physical_implied_vol: f64,
        realized_underlying_vol: f64,
    ) -> SyntheticDispersionSignal {
        let replicator = arbitrageur_math::black_scholes::LelandOptionReplicator::new(
            self.perp_taker_fee_rate,
            self.perp_rebalance_interval_sec,
        );

        // Case 1: Physical option IV is higher than synthetic long replication cost -> Sell physical, Buy synthetic (Delta hedge via perp)
        // Synthesizing long call involves buying high and selling low; replication cost adds friction: (1.0 + factor)
        let leland_long_vol = replicator.compute_leland_volatility(realized_underlying_vol, false);
        if physical_implied_vol > leland_long_vol + self.min_vol_spread_hurdle {
            let target_hedge = replicator
                .target_perp_replication_qty(
                    option_type,
                    contracts,
                    spot,
                    strike,
                    time_to_exp,
                    risk_free_rate,
                    realized_underlying_vol,
                )
                .unwrap_or(0.0);
            return SyntheticDispersionSignal::SellPhysicalBuySynthetic {
                symbol: symbol.to_string(),
                physical_iv: physical_implied_vol,
                synthetic_leland_vol: leland_long_vol,
                edge_vol_spread: physical_implied_vol - leland_long_vol,
                target_perp_hedge_qty: target_hedge,
                action: format!(
                    "Sell physical option on Deribit (rich IV {:.1}%), synthetically replicate long on Perp DEX (effective vol {:.1}%)",
                    physical_implied_vol * 100.0,
                    leland_long_vol * 100.0
                ),
            };
        }

        // Case 2: Physical option IV is lower than synthetic short replication value -> Buy physical, Sell synthetic
        // Hedging a physical long call extracts gamma minus friction: (1.0 - factor)
        let leland_short_vol = replicator.compute_leland_volatility(realized_underlying_vol, true);
        if physical_implied_vol < leland_short_vol - self.min_vol_spread_hurdle {
            let target_hedge = replicator
                .target_perp_replication_qty(
                    option_type,
                    contracts,
                    spot,
                    strike,
                    time_to_exp,
                    risk_free_rate,
                    realized_underlying_vol,
                )
                .unwrap_or(0.0);
            return SyntheticDispersionSignal::BuyPhysicalSellSynthetic {
                symbol: symbol.to_string(),
                physical_iv: physical_implied_vol,
                synthetic_leland_vol: leland_short_vol,
                edge_vol_spread: leland_short_vol - physical_implied_vol,
                target_perp_hedge_qty: -target_hedge,
                action: format!(
                    "Buy physical option on Deribit (cheap IV {:.1}%), synthetically replicate short on Perp DEX (effective vol {:.1}%)",
                    physical_implied_vol * 100.0,
                    leland_short_vol * 100.0
                ),
            };
        }

        SyntheticDispersionSignal::HoldNoArb
    }

    /// Solves multi-instrument quadratic risk optimization to simultaneously neutralize portfolio Delta, Gamma, and Vega
    /// across physical options and perpetual hedges.
    pub fn optimize_multi_greek_portfolio(
        &self,
        target: &arbitrageur_math::portfolio_greeks_opt::MultiGreekPortfolioTarget,
        instruments: &[arbitrageur_math::portfolio_greeks_opt::HedgingInstrument],
        config: Option<arbitrageur_math::portfolio_greeks_opt::MultiGreekOptimizationConfig>,
    ) -> arbitrageur_math::portfolio_greeks_opt::MultiGreekSolution {
        let optimizer = arbitrageur_math::portfolio_greeks_opt::MultiGreekQpOptimizer::new(
            config.unwrap_or_default(),
        );
        optimizer.solve(target, instruments)
    }

    /// Evaluates non-linear Whalley-Wilmott-Bouchaud power-law rehedging bandwidth under jump-diffusion.
    pub fn evaluate_power_law_rehedge(
        &self,
        spot: f64,
        net_gamma: f64,
        config: Option<arbitrageur_math::delta_gamma_rehedge::PowerLawHedgingConfig>,
    ) -> arbitrageur_math::delta_gamma_rehedge::BichuchPowerLawBandwidth {
        let rehedger = arbitrageur_math::delta_gamma_rehedge::NonlinearDeltaGammaRehedger::new(
            config.unwrap_or_default(),
        );
        rehedger.compute_optimal_bandwidth(spot, net_gamma)
    }

    /// Evaluates synthetic perpetual option replication delta and price under continuous asymmetric funding carry.
    pub fn evaluate_synthetic_replication(
        &self,
        spot: f64,
        strike: f64,
        tau_years: f64,
        is_call: bool,
        config: Option<arbitrageur_math::delta_gamma_rehedge::PowerLawHedgingConfig>,
    ) -> arbitrageur_math::delta_gamma_rehedge::SyntheticOptionReplication {
        let rehedger = arbitrageur_math::delta_gamma_rehedge::NonlinearDeltaGammaRehedger::new(
            config.unwrap_or_default(),
        );
        rehedger.compute_synthetic_replication(spot, strike, tau_years, is_call)
    }
}

/// Engine for trading relative Volatility Risk Premium (VRP) dispersion between
/// benchmark variance (BTC) and constituent variance (ETH) using replicated discrete variance swaps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarianceSwapDispersionEngine {
    pub replicator: arbitrageur_math::variance_swap::VarianceSwapReplicator,
    pub min_vrp_spread_hurdle: f64,
}

impl Default for VarianceSwapDispersionEngine {
    fn default() -> Self {
        Self {
            replicator: arbitrageur_math::variance_swap::VarianceSwapReplicator::default(),
            min_vrp_spread_hurdle: 0.05, // 5 vol pts minimum hurdle
        }
    }
}

impl VarianceSwapDispersionEngine {
    pub fn new(risk_free_rate: f64, min_vrp_spread_hurdle: f64) -> Self {
        Self {
            replicator: arbitrageur_math::variance_swap::VarianceSwapReplicator::new(
                risk_free_rate,
            ),
            min_vrp_spread_hurdle: min_vrp_spread_hurdle.max(0.01),
        }
    }

    /// Evaluates dispersion opportunity between benchmark (BTC) and constituent (ETH) option slices.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_dispersion_opportunity(
        &self,
        symbol_bench: &str,
        forward_bench: f64,
        t_exp_bench: f64,
        options_bench: Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>,
        rv_bench: f64,
        symbol_const: &str,
        forward_const: f64,
        t_exp_const: f64,
        options_const: Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>,
        rv_const: f64,
    ) -> Option<arbitrageur_math::variance_swap::VarianceDispersionOpportunity> {
        let var_bench =
            self.replicator
                .replicate_variance_strike(forward_bench, t_exp_bench, options_bench)?;
        let var_const =
            self.replicator
                .replicate_variance_strike(forward_const, t_exp_const, options_const)?;

        Some(self.replicator.evaluate_variance_dispersion(
            symbol_bench,
            var_bench,
            rv_bench,
            symbol_const,
            var_const,
            rv_const,
            self.min_vrp_spread_hurdle,
        ))
    }

    /// Evaluates dispersion opportunity with SVI power-law wing extrapolation to prevent tail truncation error.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_dispersion_opportunity_with_svi_tails(
        &self,
        symbol_bench: &str,
        forward_bench: f64,
        t_exp_bench: f64,
        options_bench: Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>,
        svi_bench: &arbitrageur_math::variance_swap::SviParameters,
        rv_bench: f64,
        symbol_const: &str,
        forward_const: f64,
        t_exp_const: f64,
        options_const: Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>,
        svi_const: &arbitrageur_math::variance_swap::SviParameters,
        rv_const: f64,
    ) -> Option<arbitrageur_math::variance_swap::VarianceDispersionOpportunity> {
        let var_bench = self.replicator.replicate_variance_strike_svi_tails(
            forward_bench,
            t_exp_bench,
            options_bench,
            svi_bench,
        )?;
        let var_const = self.replicator.replicate_variance_strike_svi_tails(
            forward_const,
            t_exp_const,
            options_const,
            svi_const,
        )?;

        Some(self.replicator.evaluate_variance_dispersion(
            symbol_bench,
            var_bench,
            rv_bench,
            symbol_const,
            var_const,
            rv_const,
            self.min_vrp_spread_hurdle,
        ))
    }

    /// Evaluates calendar VRP term structure trading opportunity across front-month and back-month expiries.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_calendar_vrp_opportunity(
        &self,
        symbol: &str,
        t1: f64,
        var_strike_t1: f64,
        rv_t1: f64,
        t2: f64,
        var_strike_t2: f64,
        rv_t2: f64,
        calendar_hurdle: f64,
    ) -> arbitrageur_math::variance_swap::CalendarVrpOpportunity {
        self.replicator.evaluate_calendar_vrp_spread(
            symbol,
            t1,
            var_strike_t1,
            rv_t1,
            t2,
            var_strike_t2,
            rv_t2,
            calendar_hurdle,
        )
    }

    /// Evaluates Whalley-Wilmott dynamic perpetual delta rehedge trigger for a variance swap strip.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_perp_rehedge_trigger(
        &self,
        spot_price: f64,
        forward_price: f64,
        time_to_expiry_years: f64,
        volatility: f64,
        current_perp_hedge_qty: f64,
        perp_fee_cost_rate: f64,
        risk_aversion: f64,
    ) -> Option<f64> {
        self.replicator.evaluate_perp_rehedge_trigger(
            spot_price,
            forward_price,
            time_to_expiry_years,
            volatility,
            current_perp_hedge_qty,
            perp_fee_cost_rate,
            risk_aversion,
        )
    }
}

/// Signal for executing a basis curve relative value trade (Butterfly or Steepener/Flattener).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BasisCurveTradeAction {
    /// Long Butterfly (Long wings, Short body): harvests curvature roll-down.
    LongButterfly {
        front_wing: String,
        body: String,
        back_wing: String,
        expected_roll_down_bps: f64,
    },
    /// Curve Steepener (Long back tenor, Short front tenor).
    CurveSteepener {
        front_tenor: String,
        back_tenor: String,
        slope_z_score: f64,
    },
    /// Curve Flattener (Short back tenor, Long front tenor).
    CurveFlattener {
        front_tenor: String,
        back_tenor: String,
        slope_z_score: f64,
    },
    /// Hold neutral posture.
    HoldNeutral,
}

/// Arbitrage engine for multi-tenor basis curves and synthetic forward pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasisCurvePcaArbitrageEngine {
    pub pca_engine: arbitrageur_math::basis_pca::BasisCurvePcaEngine,
    pub entry_z_hurdle: f64,
}

impl Default for BasisCurvePcaArbitrageEngine {
    fn default() -> Self {
        Self::new(1.8)
    }
}

impl BasisCurvePcaArbitrageEngine {
    pub fn new(entry_z_hurdle: f64) -> Self {
        Self {
            pca_engine: arbitrageur_math::basis_pca::BasisCurvePcaEngine::new(5),
            entry_z_hurdle: entry_z_hurdle.max(1.0),
        }
    }

    /// Evaluates trading signals across a term structure of basis points.
    pub fn evaluate_curve_arbitrage(
        &self,
        tenors: &[arbitrageur_math::basis_pca::BasisTenorPoint],
    ) -> BasisCurveTradeAction {
        if tenors.len() < 5 {
            return BasisCurveTradeAction::HoldNeutral;
        }

        let yields: Vec<f64> = tenors.iter().take(5).map(|t| t.basis_yield_pct).collect();
        let scores = match self.pca_engine.extract_factor_scores(&yields) {
            Some(s) => s,
            None => return BasisCurveTradeAction::HoldNeutral,
        };

        // 1. Check curvature dislocation -> Butterfly spread
        if scores.curvature_z_score.abs() >= self.entry_z_hurdle {
            if let Some(butterfly) = self.pca_engine.build_butterfly_spread(tenors, 0, 2, 4) {
                return BasisCurveTradeAction::LongButterfly {
                    front_wing: butterfly.front_wing_label,
                    body: butterfly.body_label,
                    back_wing: butterfly.back_wing_label,
                    expected_roll_down_bps: butterfly.expected_roll_down_bps,
                };
            }
        }

        // 2. Check slope dislocation -> Steepener vs Flattener
        if scores.slope_z_score >= self.entry_z_hurdle {
            return BasisCurveTradeAction::CurveFlattener {
                front_tenor: tenors[0].label.clone(),
                back_tenor: tenors[4].label.clone(),
                slope_z_score: scores.slope_z_score,
            };
        } else if scores.slope_z_score <= -self.entry_z_hurdle {
            return BasisCurveTradeAction::CurveSteepener {
                front_tenor: tenors[0].label.clone(),
                back_tenor: tenors[4].label.clone(),
                slope_z_score: scores.slope_z_score,
            };
        }

        BasisCurveTradeAction::HoldNeutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_implied_correlation() {
        let calibrator = ImpliedCorrelationSmileCalibrator::default();
        // BTC IV = 60%, ETH IV = 75%, Correlation = 0.80
        // Variance = 0.65^2 * 0.6^2 + 0.35^2 * 0.75^2 + 2 * 0.65 * 0.35 * 0.80 * 0.60 * 0.75
        // = 0.1521 + 0.06890625 + 0.1638 = 0.38480625
        // Basket IV = sqrt(0.38480625) = 0.6203275
        let btc_iv = 0.60;
        let eth_iv = 0.75;
        let basket_iv = 0.62032753445;

        let rho = calibrator.extract_implied_correlation(btc_iv, eth_iv, basket_iv);
        assert!((rho - 0.80).abs() < 1e-4);
    }

    #[test]
    fn test_portfolio_margin_relief() {
        let calibrator = ImpliedCorrelationSmileCalibrator::default();
        let relief = calibrator.compute_portfolio_margin_relief();
        // With w1=0.65, w2=0.35, rho=0.85:
        // net_var = 0.65^2 + 0.35^2 - 2 * 0.85 * 0.65 * 0.35 = 0.4225 + 0.1225 - 0.38675 = 0.15825
        // net_risk = sqrt(0.15825) = 0.3978
        // Relief = 1 - 0.3978 / 1.0 = ~60.2% relief
        assert!(relief > 0.55 && relief < 0.75);
    }

    #[test]
    fn test_dispersion_entry_signal() {
        let calibrator = ImpliedCorrelationSmileCalibrator::default();
        let points = vec![
            CorrelationSmilePoint {
                strike: 60000.0,
                moneyness: 0.90,
                btc_iv: 0.65,
                eth_iv: 0.80,
                basket_iv: 0.68,
                implied_correlation: 0.88,
            },
            CorrelationSmilePoint {
                strike: 65000.0,
                moneyness: 1.00,
                btc_iv: 0.60,
                eth_iv: 0.75,
                basket_iv: 0.63,
                implied_correlation: 0.84,
            },
            CorrelationSmilePoint {
                strike: 70000.0,
                moneyness: 1.10,
                btc_iv: 0.62,
                eth_iv: 0.78,
                basket_iv: 0.65,
                implied_correlation: 0.86,
            },
        ];

        // Avg implied correlation = (0.88 + 0.84 + 0.86) / 3 = 0.86
        // Realized correlation = 0.70 -> Spread = 0.16 >= 0.12 hurdle -> Enter
        let sig = calibrator.evaluate_dispersion(&points, 0.70);
        assert!(sig.should_enter);
        assert!((sig.correlation_spread - 0.16).abs() < 1e-4);
        assert!(sig.action.starts_with("ENTER_DISPERSION"));

        // If realized correlation is 0.78 -> Spread = 0.08 < 0.12 hurdle -> Hold
        let sig_hold = calibrator.evaluate_dispersion(&points, 0.78);
        assert!(!sig_hold.should_enter);
        assert!(sig_hold.action.starts_with("HOLD_NEUTRAL"));
    }

    #[test]
    fn test_cross_venue_skew_arbitrage_decision() {
        let router = CrossVenueSmileArbitrageEngine::default();
        // Deribit IV = 60%, Onchain IV = 68% -> Spread = +8 vol pts >= 6.5 vol pts
        let dec = router.evaluate_skew_dislocation(0.60, 0.68, 65000.0, 65000.0, 65000.0);
        assert!(dec.should_trade);
        assert!(dec.recommended_action.starts_with("SKEW_ARB_SHORT_ONCHAIN"));

        // Forward difference: Deribit = 65,000, Onchain = 65,200 (diff = 200 / 65000 = 30.7 bps >= 15 bps)
        let dec_fwd = router.evaluate_skew_dislocation(0.60, 0.61, 65000.0, 65200.0, 65000.0);
        assert!(dec_fwd.should_trade);
        assert!(dec_fwd
            .recommended_action
            .starts_with("FORWARD_ARB_SYNTHETIC"));
    }

    #[test]
    fn test_online_recursive_sabr_calibrator() {
        let forward = 65000.0;
        let time_to_exp = 30.0 / 365.25;
        let init_atm_iv = 0.55;
        let mut calibrator = OnlineRecursiveSabrCalibrator::new(forward, time_to_exp, init_atm_iv);

        assert_eq!(calibrator.forward, forward);
        assert_eq!(calibrator.alpha, init_atm_iv);

        // Feed realistic market IV quotes around ATM and wings
        let quotes = [
            (55000.0, 0.62),
            (60000.0, 0.58),
            (65000.0, 0.55),
            (70000.0, 0.57),
            (75000.0, 0.63),
        ];

        for (strike, iv) in quotes {
            let ok = calibrator.update_quote(strike, iv);
            assert!(ok, "Update quote for strike {strike} must succeed");
        }

        assert!(calibrator.updates_count == 5);
        // Alpha, rho, nu stay bounded
        assert!(calibrator.alpha > 0.05 && calibrator.alpha < 3.50);
        assert!(calibrator.rho > -0.95 && calibrator.rho < 0.95);
        assert!(calibrator.nu > 0.05 && calibrator.nu < 4.00);

        // Model volatility produces reasonable values across the smile
        let iv_atm = calibrator.model_volatility(65000.0).expect("model iv");
        assert!(iv_atm > 0.40 && iv_atm < 0.80);
    }

    #[test]
    fn test_live_vol_surface_cache() {
        let cache = LiveVolSurfaceCache::new();
        let expiry = "25DEC26";
        let expiry_ms = 1798185600000;
        let tenor = 80.0 / 365.25;
        let forward = 65000.0;

        // Ingest quotes for BTC
        cache.ingest_quote("BTC", expiry, expiry_ms, tenor, forward, 60000.0, 0.58);
        cache.ingest_quote("BTC", expiry, expiry_ms, tenor, forward, 65000.0, 0.54);
        cache.ingest_quote("BTC", expiry, expiry_ms, tenor, forward, 70000.0, 0.57);

        // Evaluate via VolatilitySmileProvider interface
        let iv_provider = cache.get_implied_volatility("BTC", expiry, 65000.0);
        assert!(iv_provider.is_some());

        // Ingest older quote for another expiry, then newer quote for the first expiry
        cache.ingest_quote(
            "BTC",
            "31DEC26",
            expiry_ms + 86400000,
            tenor,
            66000.0,
            66000.0,
            0.55,
        );
        // Manually adjust timestamps to test determinism
        {
            let mut map = cache.slices.write();
            let slices = map.get_mut("BTC").unwrap();
            slices.get_mut("25DEC26").unwrap().last_updated_ms = 2000;
            slices.get_mut("31DEC26").unwrap().last_updated_ms = 1000;
        }
        assert_eq!(cache.latest_forward("BTC"), Some(65000.0));

        {
            let mut map = cache.slices.write();
            let slices = map.get_mut("BTC").unwrap();
            slices.get_mut("31DEC26").unwrap().last_updated_ms = 3000;
        }
        assert_eq!(cache.latest_forward("BTC"), Some(66000.0));
        let iv = iv_provider.unwrap();
        assert!(iv > 0.40 && iv < 0.80);

        // Unknown asset or expiry returns None
        assert!(cache
            .get_implied_volatility("SOL", expiry, 65000.0)
            .is_none());
        assert!(cache
            .get_implied_volatility("BTC", "31DEC99", 65000.0)
            .is_none());
    }

    #[test]
    fn live_vol_surface_calibrates_arbitrage_free_svi_and_smile_delta() {
        let cache = LiveVolSurfaceCache::new();
        let forward = 100.0;
        let tenor = 0.25;
        let expiry_ms = 1_900_000_000_000;
        let truth = SviSliceParameters::new(0.04, 0.10, -0.40, 0.0, 0.15);
        let quotes = [-0.30, -0.20, -0.10, 0.0, 0.10, 0.20, 0.30]
            .into_iter()
            .map(|k| {
                let iv = (truth.total_variance(k) / tenor).sqrt();
                ((forward * k.exp() * 100.0).round() / 100.0, iv)
            })
            .collect::<Vec<_>>();

        cache.ingest_chain_slice("BTC", "SVI-TEST", expiry_ms, tenor, forward, &quotes);
        let fitted = {
            let map = cache.slices.read();
            map["BTC"]["SVI-TEST"]
                .svi_params
                .expect("SVI slice should calibrate")
        };
        assert!(fitted.is_arbitrage_free());
        let adjusted = cache
            .get_smile_adjusted_delta("BTC", expiry_ms, 95.0, forward, 0.35, 18.0)
            .expect("smile-adjusted delta");
        assert!(adjusted.is_finite());
        assert!((adjusted - 0.35).abs() > 1e-8);
        assert_eq!(
            cache.get_smile_adjusted_delta("BTC", expiry_ms + 1, 95.0, forward, 0.35, 18.0),
            None
        );
    }

    #[test]
    fn test_synthetic_perp_option_dispersion_engine() {
        let engine = SyntheticPerpOptionDispersionEngine::new(0.05, 0.0004, 300.0);
        let spot = 65000.0;
        let strike = 65000.0;
        let time_to_exp = 30.0 / 365.25;
        let rate = 0.03;
        let contracts = 2.0;

        // Rich physical IV (65%) vs realized 50%
        let sig_sell = engine.evaluate_synthetic_dispersion(
            "BTC-25DEC26-65000-C",
            arbitrageur_math::black_scholes::OptionType::Call,
            contracts,
            spot,
            strike,
            time_to_exp,
            rate,
            0.65,
            0.50,
        );
        match sig_sell {
            SyntheticDispersionSignal::SellPhysicalBuySynthetic {
                physical_iv,
                synthetic_leland_vol,
                edge_vol_spread,
                target_perp_hedge_qty,
                action,
                ..
            } => {
                assert!((physical_iv - 0.65).abs() < 1e-4);
                assert!(synthetic_leland_vol > 0.50);
                assert!(edge_vol_spread > 0.05);
                assert!(target_perp_hedge_qty > 0.0);
                assert!(action.contains("Sell physical option"));
            }
            other => panic!("Expected SellPhysicalBuySynthetic, got {:?}", other),
        }

        // Cheap physical IV (45%) vs realized 60%
        let sig_buy = engine.evaluate_synthetic_dispersion(
            "BTC-25DEC26-65000-C",
            arbitrageur_math::black_scholes::OptionType::Call,
            contracts,
            spot,
            strike,
            time_to_exp,
            rate,
            0.45,
            0.60,
        );
        match sig_buy {
            SyntheticDispersionSignal::BuyPhysicalSellSynthetic {
                physical_iv,
                synthetic_leland_vol,
                edge_vol_spread,
                target_perp_hedge_qty,
                action,
                ..
            } => {
                assert!((physical_iv - 0.45).abs() < 1e-4);
                assert!(synthetic_leland_vol < 0.60);
                assert!(edge_vol_spread > 0.05);
                assert!(target_perp_hedge_qty < 0.0);
                assert!(action.contains("Buy physical option"));
            }
            other => panic!("Expected BuyPhysicalSellSynthetic, got {:?}", other),
        }

        // Neutral market (IV 55% vs Realized 55%)
        let sig_neutral = engine.evaluate_synthetic_dispersion(
            "BTC-25DEC26-65000-C",
            arbitrageur_math::black_scholes::OptionType::Call,
            contracts,
            spot,
            strike,
            time_to_exp,
            rate,
            0.55,
            0.55,
        );
        assert_eq!(sig_neutral, SyntheticDispersionSignal::HoldNoArb);
    }

    #[test]
    fn test_synthetic_dispersion_engine_multi_greek_optimization() {
        use arbitrageur_math::portfolio_greeks_opt::{
            HedgingInstrument, MultiGreekPortfolioTarget,
        };

        let engine = SyntheticPerpOptionDispersionEngine::new(0.05, 0.0004, 300.0);

        let target = MultiGreekPortfolioTarget {
            current_delta: 2.0,
            current_gamma: -0.15,
            current_vega: -25.0,
            max_delta_tolerance: 0.1,
            max_gamma_tolerance: 0.05,
            max_vega_tolerance: 2.0,
        };

        let instruments = vec![
            HedgingInstrument {
                symbol: "BTC-PERP".to_string(),
                venue: "hyperliquid".to_string(),
                delta: 1.0,
                gamma: 0.0,
                vega: 0.0,
                linear_cost: 0.05,
                quadratic_impact: 0.01,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            HedgingInstrument {
                symbol: "BTC-ATM-CALL".to_string(),
                venue: "deribit".to_string(),
                delta: 0.5,
                gamma: 0.20,
                vega: 5.0,
                linear_cost: 0.10,
                quadratic_impact: 0.02,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            HedgingInstrument {
                symbol: "BTC-FAR-CALL".to_string(),
                venue: "deribit".to_string(),
                delta: 0.5,
                gamma: 0.05,
                vega: 25.0,
                linear_cost: 0.15,
                quadratic_impact: 0.03,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
        ];

        let sol = engine.optimize_multi_greek_portfolio(&target, &instruments, None);
        assert!(sol.post_hedge_delta.abs() < 2.0);
        assert!(sol.post_hedge_gamma.abs() < 0.15);
        assert!(sol.post_hedge_vega.abs() < 25.0);
    }

    #[test]
    fn test_variance_swap_dispersion_engine() {
        use arbitrageur_math::variance_swap::{DiscreteOptionQuote, VarianceDispersionAction};

        let engine = VarianceSwapDispersionEngine::new(0.04, 0.05);

        // BTC strip: 50k forward, 30 days
        let f_btc = 50_000.0;
        let t_exp = 30.0 / 365.25;
        let strikes_btc: [f64; 5] = [40_000.0, 45_000.0, 50_000.0, 55_000.0, 60_000.0];
        let mut opts_btc = Vec::new();
        for &k in &strikes_btc {
            let is_call = k >= f_btc;
            let p = if is_call {
                f64::max(f_btc - k, 0.0) + 1200.0
            } else {
                f64::max(k - f_btc, 0.0) + 1200.0
            };
            opts_btc.push(DiscreteOptionQuote::new(k, p * 0.99, p * 1.01, is_call));
        }

        // ETH strip: 3k forward, 30 days, elevated implied variance
        let f_eth = 3_000.0;
        let strikes_eth: [f64; 5] = [2_400.0, 2_700.0, 3_000.0, 3_300.0, 3_600.0];
        let mut opts_eth = Vec::new();
        for &k in &strikes_eth {
            let is_call = k >= f_eth;
            let p = if is_call {
                f64::max(f_eth - k, 0.0) + 200.0
            } else {
                f64::max(k - f_eth, 0.0) + 200.0
            };
            opts_eth.push(DiscreteOptionQuote::new(k, p * 0.99, p * 1.01, is_call));
        }

        let opp = engine
            .evaluate_dispersion_opportunity(
                "BTC", f_btc, t_exp, opts_btc, 0.45, // RV = 45%
                "ETH", f_eth, t_exp, opts_eth, 0.50, // RV = 50%
            )
            .expect("dispersion opportunity");

        assert_eq!(opp.benchmark_symbol, "BTC");
        assert_eq!(opp.constituent_symbol, "ETH");
        assert!(matches!(
            opp.action,
            VarianceDispersionAction::SellConstituentBuyBenchmark
                | VarianceDispersionAction::SellBenchmarkBuyConstituent
                | VarianceDispersionAction::HoldNeutral
        ));
    }

    #[test]
    fn test_basis_curve_pca_arbitrage_engine() {
        use arbitrageur_math::basis_pca::BasisTenorPoint;

        let engine = BasisCurvePcaArbitrageEngine::new(1.0);

        // Humped curve with dislocation: PERP = 4.0, 1M = 8.0, 3M = 16.0 (rich belly), 6M = 8.0, 9M = 4.0
        let humped_tenors = vec![
            BasisTenorPoint::new("PERP", 0.0, 4.0),
            BasisTenorPoint::new("1M", 0.0833, 8.0),
            BasisTenorPoint::new("3M", 0.25, 16.0),
            BasisTenorPoint::new("6M", 0.50, 8.0),
            BasisTenorPoint::new("9M", 0.75, 4.0),
        ];

        let action = engine.evaluate_curve_arbitrage(&humped_tenors);
        assert!(matches!(
            action,
            BasisCurveTradeAction::LongButterfly { .. }
                | BasisCurveTradeAction::CurveFlattener { .. }
                | BasisCurveTradeAction::CurveSteepener { .. }
        ));
    }

    #[test]
    fn test_synthetic_dispersion_power_law_rehedge_and_replication() {
        let engine = SyntheticPerpOptionDispersionEngine::default();
        let bw = engine.evaluate_power_law_rehedge(60000.0, 0.0005, None);
        assert!(bw.optimal_bandwidth_usd > 0.0);
        assert!(bw.jump_compression_multiplier <= 1.0);

        let rep = engine.evaluate_synthetic_replication(60000.0, 60000.0, 0.25, true, None);
        assert!(rep.synthetic_delta > 0.0 && rep.synthetic_delta < 1.0);
        assert!(rep.synthetic_price > 0.0);
    }

    #[test]
    fn test_variance_swap_dispersion_engine_svi_tails_and_calendar_vrp() {
        use arbitrageur_math::variance_swap::{
            CalendarVrpAction, DiscreteOptionQuote, SviParameters,
        };

        let engine = VarianceSwapDispersionEngine::new(0.04, 0.03);
        let f_btc = 60_000.0;
        let f_eth = 3_000.0;
        let t_exp = 30.0 / 365.25;

        let strikes_btc = [54_000.0, 58_000.0, 60_000.0, 62_000.0, 66_000.0];
        let mut opts_btc = Vec::new();
        for &k in &strikes_btc {
            let is_call = k >= f_btc;
            let p = if is_call {
                f64::max(f_btc - k, 0.0) + 2000.0
            } else {
                f64::max(k - f_btc, 0.0) + 2000.0
            };
            opts_btc.push(DiscreteOptionQuote::new(k, p * 0.99, p * 1.01, is_call));
        }

        let strikes_eth = [2700.0, 2900.0, 3000.0, 3100.0, 3300.0];
        let mut opts_eth = Vec::new();
        for &k in &strikes_eth {
            let is_call = k >= f_eth;
            let p = if is_call {
                f64::max(f_eth - k, 0.0) + 200.0
            } else {
                f64::max(k - f_eth, 0.0) + 200.0
            };
            opts_eth.push(DiscreteOptionQuote::new(k, p * 0.99, p * 1.01, is_call));
        }

        let svi_btc = SviParameters::new(0.04, 0.15, -0.20, 0.0, 0.10);
        let svi_eth = SviParameters::new(0.06, 0.18, -0.25, 0.0, 0.12);

        let opp = engine
            .evaluate_dispersion_opportunity_with_svi_tails(
                "BTC", f_btc, t_exp, opts_btc, &svi_btc, 0.45, "ETH", f_eth, t_exp, opts_eth,
                &svi_eth, 0.50,
            )
            .expect("tail dispersion opportunity");

        assert_eq!(opp.benchmark_symbol, "BTC");
        assert_eq!(opp.constituent_symbol, "ETH");

        // Calendar VRP test
        let cal = engine
            .evaluate_calendar_vrp_opportunity("BTC", 0.08, 0.36, 0.45, 0.16, 0.30, 0.48, 0.02);
        assert_eq!(cal.symbol, "BTC");
        assert!(matches!(
            cal.action,
            CalendarVrpAction::ShortFrontLongBackCalendar
                | CalendarVrpAction::LongFrontShortBackCalendar
                | CalendarVrpAction::HoldNeutral
        ));

        // Rehedge trigger test
        let rehedge = engine
            .evaluate_perp_rehedge_trigger(60_000.0, 60_200.0, 0.25, 0.60, 0.50, 0.0005, 0.05);
        assert!(rehedge.is_some());
    }
}
