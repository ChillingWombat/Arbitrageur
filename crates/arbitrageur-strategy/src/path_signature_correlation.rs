//! Path Signature Calibrated Local Correlation & Cross-Asset Multi-Basket Dispersion Engine.
//!
//! Evaluates multivariate rough path signatures up to level 2, computing anti-symmetric
//! Lévy areas $A_{ij}$ for lead-lag detection and symmetric quadratic covariations $Q_{ij}$ for
//! continuous local correlation estimation. Scans implied vs. realized cross-asset correlation
//! to trade index vs. constituent options and perpetual variance dispersion.

use serde::{Deserialize, Serialize};

/// Constituent asset in a cross-asset basket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstituentAsset {
    pub symbol: String,
    pub weight: f64,
    pub price_series: Vec<f64>,
}

/// Configuration parameters for the Path Signature Dispersion Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureCorrelationConfig {
    /// Correlation dislocation hurdle required to enter dispersion (e.g. 0.12 or 12%).
    pub dispersion_hurdle: f64,
    /// Minimum path length required for signature iterated integrals.
    pub min_path_length: usize,
    /// Maximum allowable leverage cap.
    pub max_leverage: f64,
}

impl Default for SignatureCorrelationConfig {
    fn default() -> Self {
        Self {
            dispersion_hurdle: 0.12,
            min_path_length: 10,
            max_leverage: 3.0,
        }
    }
}

/// Action decision for dispersion arbitrage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DispersionAction {
    /// Implied correlation is overpriced relative to realized signature correlation.
    /// Short index options/variance, Long constituent options/variance.
    SellIndexBuyConstituents {
        correlation_gap: f64,
        suggested_leverage: f64,
    },
    /// Implied correlation is underpriced relative to realized signature correlation.
    /// Long index options/variance, Short constituent options/variance.
    BuyIndexSellConstituents {
        correlation_gap: f64,
        suggested_leverage: f64,
    },
    /// Dislocation within friction hurdle; maintain delta/vega-neutral hold.
    NeutralHold { correlation_gap: f64 },
}

/// Evaluated dispersion signal containing pairwise signature metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispersionSignal {
    pub action: DispersionAction,
    pub implied_correlation: f64,
    pub signature_realized_correlation: f64,
    pub correlation_gap: f64,
    pub pairwise_correlations: Vec<((String, String), f64)>,
    pub lead_lag_scores: Vec<((String, String), f64)>,
}

/// Path Signature Correlation & Dispersion Engine.
pub struct PathSignatureCorrelationEngine {
    pub config: SignatureCorrelationConfig,
    pub constituents: Vec<ConstituentAsset>,
}

impl PathSignatureCorrelationEngine {
    pub fn new(config: SignatureCorrelationConfig, constituents: Vec<ConstituentAsset>) -> Self {
        Self {
            config,
            constituents,
        }
    }

    /// Computes (Lévy area $A_{ij}$, quadratic covariation $Q_{ij}$, signature correlation $\rho_{ij}$)
    /// for two discrete price paths using level-2 path signatures.
    pub fn compute_levy_area_and_covar(series_i: &[f64], series_j: &[f64]) -> (f64, f64, f64) {
        let n = series_i.len().min(series_j.len());
        if n < 2 {
            return (0.0, 0.0, 0.0);
        }

        let x0 = series_i[0];
        let y0 = series_j[0];

        // Level-2 iterated integrals: \int (X_u - X_0) dY_u and \int (Y_u - Y_0) dX_u
        let mut int_x_dy = 0.0;
        let mut int_y_dx = 0.0;
        let mut int_x_dx = 0.0;
        let mut int_y_dy = 0.0;

        for k in 0..(n - 1) {
            let x_mid = series_i[k] - x0;
            let y_mid = series_j[k] - y0;

            let dx = series_i[k + 1] - series_i[k];
            let dy = series_j[k + 1] - series_j[k];

            int_x_dy += x_mid * dy;
            int_y_dx += y_mid * dx;
            int_x_dx += x_mid * dx;
            int_y_dy += y_mid * dy;
        }

        // Anti-symmetric Lévy Area A_ij = 0.5 * (\int X dY - \int Y dX)
        let levy_area = 0.5 * (int_x_dy - int_y_dx);

        // Symmetric tensor Q_ij = 0.5 * (\int X dY + \int Y dX)
        let q_ij = 0.5 * (int_x_dy + int_y_dx);
        let q_ii = int_x_dx.abs().max(1e-12);
        let q_jj = int_y_dy.abs().max(1e-12);

        let denom = (q_ii * q_jj).sqrt();
        let corr = if denom > 1e-12 {
            (q_ij / denom).clamp(-0.999, 0.999)
        } else {
            0.0
        };

        (levy_area, q_ij, corr)
    }

    /// Evaluates the basket dispersion arbitrage signal against market implied correlation.
    pub fn evaluate_dispersion_signal(&self, implied_correlation: f64) -> DispersionSignal {
        let k = self.constituents.len();
        if k < 2 {
            return DispersionSignal {
                action: DispersionAction::NeutralHold {
                    correlation_gap: 0.0,
                },
                implied_correlation,
                signature_realized_correlation: implied_correlation,
                correlation_gap: 0.0,
                pairwise_correlations: Vec::new(),
                lead_lag_scores: Vec::new(),
            };
        }

        let mut pairwise_corrs = Vec::new();
        let mut lead_lag = Vec::new();

        let mut weighted_corr_sum = 0.0;
        let mut total_weight_pairs = 0.0;

        for i in 0..k {
            for j in (i + 1)..k {
                let asset_i = &self.constituents[i];
                let asset_j = &self.constituents[j];

                if asset_i.price_series.len() < self.config.min_path_length
                    || asset_j.price_series.len() < self.config.min_path_length
                {
                    continue;
                }

                let (levy, _q, corr) =
                    Self::compute_levy_area_and_covar(&asset_i.price_series, &asset_j.price_series);

                let pair_names = (asset_i.symbol.clone(), asset_j.symbol.clone());
                pairwise_corrs.push((pair_names.clone(), corr));
                lead_lag.push((pair_names, levy));

                let w_pair = asset_i.weight * asset_j.weight;
                weighted_corr_sum += w_pair * corr;
                total_weight_pairs += w_pair;
            }
        }

        let realized_corr = if total_weight_pairs > 1e-12 {
            (weighted_corr_sum / total_weight_pairs).clamp(-0.999, 0.999)
        } else {
            0.5
        };

        let gap = implied_correlation - realized_corr;
        let hurdle = self.config.dispersion_hurdle;

        let action = if gap > hurdle {
            let mult = ((gap - hurdle) / 0.1).clamp(1.0, self.config.max_leverage);
            DispersionAction::SellIndexBuyConstituents {
                correlation_gap: gap,
                suggested_leverage: mult,
            }
        } else if gap < -hurdle {
            let mult = ((-gap - hurdle) / 0.1).clamp(1.0, self.config.max_leverage);
            DispersionAction::BuyIndexSellConstituents {
                correlation_gap: gap,
                suggested_leverage: mult,
            }
        } else {
            DispersionAction::NeutralHold {
                correlation_gap: gap,
            }
        };

        DispersionSignal {
            action,
            implied_correlation,
            signature_realized_correlation: realized_corr,
            correlation_gap: gap,
            pairwise_correlations: pairwise_corrs,
            lead_lag_scores: lead_lag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levy_area_lead_lag_detection() {
        // Asset 0 is sin(t), Asset 1 is cos(t) = sin(t + pi/2)
        // Asset 1 leads Asset 0 by phase pi/2 -> cycle is clockwise in (x, y) plane
        let n = 100;
        let mut x = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);

        for step in 0..n {
            let t = step as f64 * 0.1;
            x.push(100.0 + 10.0 * t.sin());
            y.push(100.0 + 10.0 * t.cos());
        }

        let (levy_xy, _q_xy, _corr_xy) =
            PathSignatureCorrelationEngine::compute_levy_area_and_covar(&x, &y);
        let (levy_yx, _q_yx, _corr_yx) =
            PathSignatureCorrelationEngine::compute_levy_area_and_covar(&y, &x);

        // Anti-symmetry: A_xy = -A_yx != 0
        assert!(levy_xy.abs() > 10.0);
        assert!((levy_xy + levy_yx).abs() < 1e-10);
    }

    #[test]
    fn test_dispersion_signal_generation() {
        let n = 50;
        let mut btc = Vec::with_capacity(n);
        let mut eth = Vec::with_capacity(n);

        for step in 0..n {
            let t = step as f64 * 0.05;
            btc.push(100.0 + 5.0 * t);
            eth.push(10.0 + 0.5 * t);
        }

        let constituents = vec![
            ConstituentAsset {
                symbol: "BTC".into(),
                weight: 0.6,
                price_series: btc,
            },
            ConstituentAsset {
                symbol: "ETH".into(),
                weight: 0.4,
                price_series: eth,
            },
        ];

        let config = SignatureCorrelationConfig {
            dispersion_hurdle: 0.10,
            min_path_length: 10,
            max_leverage: 2.5,
        };

        let engine = PathSignatureCorrelationEngine::new(config, constituents);

        // Realized correlation between linearly rising series is ~1.0
        // Case 1: Market implied correlation is 0.6 (cheap implied correlation)
        // gap = 0.6 - 1.0 = -0.4 < -0.10 hurdle -> BuyIndexSellConstituents
        let sig1 = engine.evaluate_dispersion_signal(0.60);
        match sig1.action {
            DispersionAction::BuyIndexSellConstituents {
                correlation_gap,
                suggested_leverage,
            } => {
                assert!(correlation_gap < -0.3);
                assert!(suggested_leverage > 1.0);
            }
            _ => panic!("Expected BuyIndexSellConstituents signal"),
        }

        // Case 2: Market implied correlation is close to realized (e.g. 0.95)
        // gap = 0.95 - 1.0 = -0.05 inside [-0.10, 0.10] hurdle -> NeutralHold
        let sig2 = engine.evaluate_dispersion_signal(0.95);
        match sig2.action {
            DispersionAction::NeutralHold { correlation_gap } => {
                assert!(correlation_gap.abs() <= 0.10);
            }
            _ => panic!("Expected NeutralHold signal"),
        }
    }
}
