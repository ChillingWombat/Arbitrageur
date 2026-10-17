//! Rough Volatility DeepONet Surface Predictor & Zero-Static-Arbitrage Smile Arbitrage (Tranche 74).
//!
//! Approximates the non-Markovian rough Bergomi / rough Heston solution operator mapping a
//! discretized forward variance curve $\xi_t(\cdot) \in \mathbb{R}^M$ and rough Hurst exponent
//! $H \in (0, 0.5)$ directly to the total implied variance surface $w(k, \tau) = \sigma_{\text{IV}}^2(k, \tau)\tau$
//! via a Deep Operator Network (DeepONet):
//! $$w(k, \tau) = b_0 + \sum_{j=1}^p B_j(\xi_t, H) \, T_j(k, \tau)$$
//!
//! Enforces exact calendar-spread absence of static arbitrage ($\partial_\tau w(k, \tau) \ge 0$)
//! by construction via Softplus-constrained non-negative branch-trunk weights and monotonically
//! increasing tenor basis functions, plus Durrleman's condition $g(k) \ge 0$ for butterfly
//! convexity across log-moneyness $k$.

use serde::{Deserialize, Serialize};

/// Number of latent operator basis modes $p$.
pub const DEEPONET_LATENT_MODES: usize = 8;

/// Number of forward variance curve grid sensors $M$.
pub const FORWARD_VARIANCE_SENSORS: usize = 8;

/// Softplus activation $\operatorname{softplus}(x) = \ln(1 + e^x)$ ensuring strict positivity.
#[inline]
pub fn softplus(x: f64) -> f64 {
    if x > 20.0 {
        x
    } else if x < -20.0 {
        x.exp()
    } else {
        (1.0 + x.exp()).ln()
    }
}

/// Derivative of Softplus: the logistic sigmoid $\sigma(x) = \frac{1}{1 + e^{-x}} > 0$.
#[inline]
pub fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let ex = x.exp();
        ex / (1.0 + ex)
    }
}

/// Input sensor state for the Rough Volatility Branch Network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughVarianceSensorState {
    /// Forward variance curve $\xi_t(\tau_m) > 0$ sampled at $M = 8$ canonical tenors.
    pub forward_variance_curve: [f64; FORWARD_VARIANCE_SENSORS],
    /// Rough volatility Hurst exponent $H \in (0.01, 0.49)$ (empirically $H \approx 0.10$ in crypto).
    pub hurst_exponent: f64,
    /// Vol-of-vol parameter $\eta > 0$.
    pub eta_vol_of_vol: f64,
    /// Spot-variance correlation $\rho \in (-1, 1)$.
    pub rho_spot_vol: f64,
}

impl RoughVarianceSensorState {
    pub fn is_valid(&self) -> bool {
        self.hurst_exponent > 0.0
            && self.hurst_exponent < 0.5
            && self.eta_vol_of_vol > 0.0
            && self.eta_vol_of_vol.is_finite()
            && self.rho_spot_vol > -1.0
            && self.rho_spot_vol < 1.0
            && self
                .forward_variance_curve
                .iter()
                .all(|&v| v > 0.0 && v.is_finite())
    }
}

/// Arbitrage-Free Rough Volatility DeepONet Operator.
///
/// Parametrizes total variance $w(k, \tau)$ as:
/// $$w(k, \tau) = \sum_{j=1}^p B_j(\xi_t, H, \eta, \rho) \, \Phi_j(k) \, \Psi_j(\tau)$$
/// where:
/// - $B_j = \operatorname{softplus}(z_j) > 0$ are strictly positive branch outputs,
/// - $\Phi_j(k) > 0$ are positive SVI-like convex log-moneyness basis functions,
/// - $\Psi_j(\tau) = \tau^{\alpha_j}$ with $\alpha_j = 2 H + 0.5 + 0.1 j > 0$ are strictly
///   increasing tenor basis functions with $\Psi_j(0) = 0$ and $\partial_\tau \Psi_j(\tau) > 0$.
///
/// Consequently, $\partial_\tau w(k, \tau) = \sum_{j=1}^p B_j \Phi_j(k) \partial_\tau \Psi_j(\tau) > 0$
/// holds identically everywhere, eliminating static calendar arbitrage by construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughDeepONetPredictor {
    /// Branch linear weights mapping $(M + 3)$ features to $p$ latent modes.
    pub branch_weights: [[f64; FORWARD_VARIANCE_SENSORS + 3]; DEEPONET_LATENT_MODES],
    /// Branch bias vector.
    pub branch_bias: [f64; DEEPONET_LATENT_MODES],
    /// Log-moneyness trunk center shifts $m_j$.
    pub trunk_shifts: [f64; DEEPONET_LATENT_MODES],
    /// Log-moneyness trunk slope parameters $s_j \in (-0.9, 0.9)$.
    pub trunk_slopes: [f64; DEEPONET_LATENT_MODES],
    /// Log-moneyness trunk curvature widths $\sigma_j > 0$.
    pub trunk_widths: [f64; DEEPONET_LATENT_MODES],
}

impl Default for RoughDeepONetPredictor {
    fn default() -> Self {
        let mut branch_weights = [[0.0; FORWARD_VARIANCE_SENSORS + 3]; DEEPONET_LATENT_MODES];
        for (j, row) in branch_weights.iter_mut().enumerate() {
            // Couple mode j primarily to sensor j and rough parameters
            row[j] = 1.2;
            row[FORWARD_VARIANCE_SENSORS] = -0.5; // Hurst H sensitivity
            row[FORWARD_VARIANCE_SENSORS + 1] = 0.35; // eta sensitivity
            row[FORWARD_VARIANCE_SENSORS + 2] = -0.25 * ((j as f64) - 3.5).signum();
            // rho skew sensitivity
        }
        let branch_bias = [-2.2, -2.3, -2.4, -2.5, -2.5, -2.6, -2.7, -2.8];
        let trunk_shifts = [-0.20, -0.12, -0.05, 0.0, 0.05, 0.12, 0.20, 0.0];
        let trunk_slopes = [-0.35, -0.25, -0.15, -0.05, 0.05, 0.15, 0.25, -0.10];
        let trunk_widths = [0.15, 0.18, 0.20, 0.22, 0.22, 0.25, 0.28, 0.30];

        Self {
            branch_weights,
            branch_bias,
            trunk_shifts,
            trunk_slopes,
            trunk_widths,
        }
    }
}

/// Pointwise surface evaluation with exact analytical derivatives for Durrleman & Calendar checks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeepONetSurfacePoint {
    pub log_moneyness: f64,
    pub tenor_years: f64,
    /// Total implied variance $w(k, \tau) = \sigma_{\text{IV}}^2 \tau$.
    pub total_variance: f64,
    /// Implied volatility $\sigma_{\text{IV}}(k, \tau) = \sqrt{w(k, \tau) / \tau}$.
    pub implied_volatility: f64,
    /// First log-moneyness derivative $\partial_k w(k, \tau)$.
    pub dw_dk: f64,
    /// Second log-moneyness derivative $\partial_{kk}^2 w(k, \tau)$.
    pub d2w_dk2: f64,
    /// First maturity derivative $\partial_\tau w(k, \tau)$ (must be $\ge 0$ for no calendar arb).
    pub dw_dtau: f64,
    /// Durrleman butterfly density function $g(k)$ (must be $\ge 0$ for no butterfly arb).
    pub durrleman_g: f64,
    /// True iff both $\partial_\tau w \ge 0$ and $g(k) \ge 0$.
    pub is_static_arbitrage_free: bool,
}

impl RoughDeepONetPredictor {
    /// Evaluates the positive branch activations $B_j(\xi_t, H, \eta, \rho) > 0$.
    pub fn evaluate_branch(
        &self,
        sensor: &RoughVarianceSensorState,
    ) -> [f64; DEEPONET_LATENT_MODES] {
        let mut features = [0.0; FORWARD_VARIANCE_SENSORS + 3];
        features[..FORWARD_VARIANCE_SENSORS].copy_from_slice(&sensor.forward_variance_curve);
        features[FORWARD_VARIANCE_SENSORS] = sensor.hurst_exponent;
        features[FORWARD_VARIANCE_SENSORS + 1] = sensor.eta_vol_of_vol;
        features[FORWARD_VARIANCE_SENSORS + 2] = sensor.rho_spot_vol;

        let mean_xi =
            sensor.forward_variance_curve.iter().sum::<f64>() / (FORWARD_VARIANCE_SENSORS as f64);
        let mut b = [0.0; DEEPONET_LATENT_MODES];
        for (j, b_val) in b.iter_mut().enumerate() {
            let mut z = self.branch_bias[j];
            for (f_idx, &f_val) in features.iter().enumerate() {
                z += self.branch_weights[j][f_idx] * f_val;
            }
            // Scale by mean forward variance so total variance scales naturally with market vol level
            *b_val = mean_xi * softplus(z);
        }
        b
    }

    /// Evaluates the surface point $w(k, \tau)$ and its exact analytical derivatives
    /// $\partial_k w$, $\partial_{kk}^2 w$, $\partial_\tau w$, and Durrleman density $g(k)$.
    pub fn evaluate_surface_point(
        &self,
        sensor: &RoughVarianceSensorState,
        k: f64,
        tau: f64,
    ) -> Option<DeepONetSurfacePoint> {
        if !sensor.is_valid() || !k.is_finite() || !tau.is_finite() || tau <= 1e-6 {
            return None;
        }

        let b = self.evaluate_branch(sensor);
        let h = sensor.hurst_exponent;

        let mut w = 0.0;
        let mut dw_dk = 0.0;
        let mut d2w_dk2 = 0.0;
        let mut dw_dtau = 0.0;

        for (j, &weight) in b.iter().enumerate() {
            let dk = k - self.trunk_shifts[j];
            let s_j = self.trunk_slopes[j].clamp(-0.85, 0.85);
            let sig_j = self.trunk_widths[j].max(0.05);
            let rad = (dk * dk + sig_j * sig_j).sqrt();

            // SVI-inspired strictly positive convex log-moneyness basis:
            // \Phi_j(k) = 1.0 + 0.25 * (s_j * dk + rad) > 0
            let phi = 1.0 + 0.25 * (s_j * dk + rad);
            let dphi_dk = 0.25 * (s_j + dk / rad);
            let d2phi_dk2 = 0.25 * (sig_j * sig_j) / (rad * rad * rad);

            // Rough-volatility power-law tenor basis:
            // Short-dated skew explodes as \tau^{H - 0.5} in IV, so total variance w \sim \tau^{0.8 + 0.4 H + 0.04 j}
            let alpha_j = 0.80 + 0.40 * h + 0.04 * (j as f64);
            let psi = tau.powf(alpha_j);
            let dpsi_dtau = alpha_j * tau.powf(alpha_j - 1.0);

            w += weight * phi * psi;
            dw_dk += weight * dphi_dk * psi;
            d2w_dk2 += weight * d2phi_dk2 * psi;
            dw_dtau += weight * phi * dpsi_dtau;
        }

        if w <= 1e-12 {
            return None;
        }

        let iv = (w / tau).sqrt();

        // Durrleman's butterfly density condition:
        // g(k) = (1 - \frac{k w'}{2 w})^2 - \frac{(w')^2}{4}(\frac{1}{w} + \frac{1}{4}) + \frac{w''}{2}
        let term1 = 1.0 - (k * dw_dk) / (2.0 * w);
        let term2 = 0.25 * (dw_dk * dw_dk) * (1.0 / w + 0.25);
        let term3 = 0.5 * d2w_dk2;
        let durrleman_g = term1 * term1 - term2 + term3;

        let is_static_arbitrage_free = dw_dtau >= -1e-12 && durrleman_g >= -1e-12;

        Some(DeepONetSurfacePoint {
            log_moneyness: k,
            tenor_years: tau,
            total_variance: w,
            implied_volatility: iv,
            dw_dk,
            d2w_dk2,
            dw_dtau,
            durrleman_g,
            is_static_arbitrage_free,
        })
    }
}

/// Market option quote for Rough DeepONet smile arbitrage scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughSurfaceQuote {
    pub symbol: String,
    pub venue: String,
    pub is_call: bool,
    pub strike: f64,
    pub spot_price: f64,
    pub tenor_years: f64,
    pub market_iv: f64,
    pub delta: f64,
    pub vega: f64,
    pub bid_usd: f64,
    pub ask_usd: f64,
}

/// Actionable zero-static-arbitrage smile dislocation signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughDeepONetArbSignal {
    pub symbol: String,
    pub venue: String,
    pub log_moneyness: f64,
    pub tenor_years: f64,
    pub model_iv: f64,
    pub market_iv: f64,
    pub iv_dislocation: f64,
    /// True if buying the underpriced option (`market_iv < model_iv`), false if selling overpriced.
    pub buy_option: bool,
    /// Delta hedge quantity in underlying perpetual (`-sign * delta`).
    pub perp_delta_hedge_ratio: f64,
    /// Net expected edge in USD after crossing option bid-ask spread.
    pub net_expected_edge_usd: f64,
    /// Verified Durrleman butterfly density $g(k) > 0$.
    pub durrleman_g: f64,
    /// Verified calendar derivative $\partial_\tau w > 0$.
    pub calendar_dw_dtau: f64,
}

/// Scans live option chains against the Rough Volatility DeepONet surface predictor,
/// gating every signal on strict static arbitrage-freeness ($g(k) > 0$ and $\partial_\tau w > 0$).
pub fn scan_rough_deeponet_smile_arb(
    predictor: &RoughDeepONetPredictor,
    sensor: &RoughVarianceSensorState,
    quotes: &[RoughSurfaceQuote],
    min_iv_dislocation: f64,
    min_net_edge_usd: f64,
) -> Vec<RoughDeepONetArbSignal> {
    if !sensor.is_valid() {
        return Vec::new();
    }

    let mut signals = Vec::new();
    for q in quotes {
        if q.spot_price <= 0.0
            || q.strike <= 0.0
            || q.tenor_years <= 1e-4
            || q.market_iv <= 0.0
            || q.vega <= 0.0
            || q.ask_usd < q.bid_usd
        {
            continue;
        }

        let k = (q.strike / q.spot_price).ln();
        let Some(pt) = predictor.evaluate_surface_point(sensor, k, q.tenor_years) else {
            continue;
        };

        // Fail-closed: never trade against a surface point that violates static arbitrage bounds
        if !pt.is_static_arbitrage_free || pt.durrleman_g <= 0.0 || pt.dw_dtau <= 0.0 {
            continue;
        }

        let iv_diff = pt.implied_volatility - q.market_iv;
        if iv_diff.abs() < min_iv_dislocation {
            continue;
        }

        let half_spread_usd = 0.5 * (q.ask_usd - q.bid_usd);
        // Vega is USD per 1.00 (100 vol points) or per 0.01 (1 vol point); standardize as USD per 1.00 IV change
        let gross_edge_usd = iv_diff.abs() * q.vega;
        let net_edge_usd = gross_edge_usd - half_spread_usd;

        if net_edge_usd >= min_net_edge_usd {
            let buy_option = iv_diff > 0.0;
            let sign = if buy_option { 1.0 } else { -1.0 };
            signals.push(RoughDeepONetArbSignal {
                symbol: q.symbol.clone(),
                venue: q.venue.clone(),
                log_moneyness: k,
                tenor_years: q.tenor_years,
                model_iv: pt.implied_volatility,
                market_iv: q.market_iv,
                iv_dislocation: iv_diff,
                buy_option,
                perp_delta_hedge_ratio: -sign * q.delta,
                net_expected_edge_usd: net_edge_usd,
                durrleman_g: pt.durrleman_g,
                calendar_dw_dtau: pt.dw_dtau,
            });
        }
    }

    signals.sort_by(|a, b| {
        b.net_expected_edge_usd
            .partial_cmp(&a.net_expected_edge_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    signals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sensor() -> RoughVarianceSensorState {
        RoughVarianceSensorState {
            forward_variance_curve: [0.25, 0.26, 0.27, 0.28, 0.29, 0.30, 0.31, 0.32],
            hurst_exponent: 0.12,
            eta_vol_of_vol: 1.15,
            rho_spot_vol: -0.45,
        }
    }

    #[test]
    fn test_rough_deeponet_static_arbitrage_free_across_grid() {
        let predictor = RoughDeepONetPredictor::default();
        let sensor = sample_sensor();

        for &tau in &[0.01, 0.05, 0.10, 0.25, 0.50, 1.00] {
            let mut prev_w_at_k = Vec::new();
            for k_idx in -10..=10 {
                let k = (k_idx as f64) * 0.04;
                let pt = predictor
                    .evaluate_surface_point(&sensor, k, tau)
                    .expect("Valid surface point");
                assert!(
                    pt.dw_dtau > 0.0,
                    "Calendar spread derivative must be strictly positive at k={}, tau={}",
                    k,
                    tau
                );
                assert!(
                    pt.durrleman_g > 0.0,
                    "Durrleman butterfly condition g(k) must be strictly positive at k={}, tau={}, got {}",
                    k,
                    tau,
                    pt.durrleman_g
                );
                assert!(pt.is_static_arbitrage_free);
                prev_w_at_k.push(pt.total_variance);
            }
        }
    }

    #[test]
    fn test_rough_deeponet_smile_arb_detection_and_delta_hedge() {
        let predictor = RoughDeepONetPredictor::default();
        let sensor = sample_sensor();

        let atm_pt = predictor
            .evaluate_surface_point(&sensor, -0.05, 0.10)
            .unwrap();

        let quotes = vec![
            RoughSurfaceQuote {
                symbol: "BTC-27DEC-57000-P".into(),
                venue: "deribit".into(),
                is_call: false,
                strike: 57_000.0,
                spot_price: 60_000.0,
                tenor_years: 0.10,
                market_iv: (atm_pt.implied_volatility - 0.06).max(0.10), // 6 vol points cheap
                delta: -0.35,
                vega: 1200.0, // $12 per vol point
                bid_usd: 950.0,
                ask_usd: 970.0,
            },
            RoughSurfaceQuote {
                symbol: "BTC-27DEC-60000-C".into(),
                venue: "deribit".into(),
                is_call: true,
                strike: 60_000.0,
                spot_price: 60_000.0,
                tenor_years: 0.10,
                market_iv: atm_pt.implied_volatility, // Fairly priced
                delta: 0.52,
                vega: 1400.0,
                bid_usd: 1500.0,
                ask_usd: 1520.0,
            },
        ];

        let signals = scan_rough_deeponet_smile_arb(&predictor, &sensor, &quotes, 0.03, 25.0);
        assert_eq!(signals.len(), 1);
        let sig = &signals[0];
        assert_eq!(sig.symbol, "BTC-27DEC-57000-P");
        assert!(sig.buy_option);
        assert!((sig.perp_delta_hedge_ratio - 0.35).abs() < 1e-9);
        assert!(sig.net_expected_edge_usd > 40.0);
    }

    #[test]
    fn test_invalid_hurst_fails_closed() {
        let predictor = RoughDeepONetPredictor::default();
        let mut bad_sensor = sample_sensor();
        bad_sensor.hurst_exponent = 0.65; // Out of rough range (0, 0.5)
        assert!(predictor
            .evaluate_surface_point(&bad_sensor, 0.0, 0.1)
            .is_none());
    }
}
