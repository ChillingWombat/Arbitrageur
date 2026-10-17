//! Path Signature Volatility Surface Embeddings & Non-Markovian Cross-Smile Dispersion.
//!
//! Evaluates linear functional implied volatility surfaces over truncated rough path
//! signatures $\mathbb{S}^{\le 3}(X) \in \mathbb{R}^{84}$ (Chevyrev-Lyons 2016, Friz-Victoir 2010):
//!
//! $$\hat{\Sigma}(k, \tau) = \sigma_0(k, \tau) + \langle \mathbf{W}_{k, \tau}, \mathbb{S}^{\le 3}(X) \rangle$$
//!
//! Captures non-Markovian memory, Lévy stochastic area, and cross-asset lead-lag geometry
//! in sub-microsecond forward passes. Detects cross-smile dislocations across crypto options
//! venues (Deribit, OKX, Aevo) and generates delta-neutral, vega-neutral dispersion baskets.

use arbitrageur_math::rough_path_signature::{RoughPathPoint, RoughPathSignatureEngine};
use serde::{Deserialize, Serialize};

/// Strike and maturity coordinate on the implied volatility surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceCoordinate {
    /// Log-moneyness $k = \ln(K / S)$.
    pub log_moneyness: f64,
    /// Time to expiration in years $\tau$.
    pub tenor_years: f64,
}

/// Linear functional parameters for a specific surface grid point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureSurfaceKernel {
    pub coord: SurfaceCoordinate,
    /// Base unconditional volatility $\sigma_0(k, \tau)$.
    pub base_vol: f64,
    /// Linear functional weight vector $\mathbf{W} \in \mathbb{R}^{84}$.
    pub weights: Vec<f64>,
}

impl SignatureSurfaceKernel {
    pub fn new(coord: SurfaceCoordinate, base_vol: f64, weights: Vec<f64>) -> Self {
        let mut w = weights;
        if w.len() != 84 {
            w.resize(84, 0.0);
        }
        Self {
            coord,
            base_vol,
            weights: w,
        }
    }

    /// Evaluates predicted implied volatility from an 84-dimensional flattened path signature.
    pub fn predict_vol(&self, signature_vector: &[f64; 84]) -> f64 {
        let mut dot = 0.0;
        for (i, &sig_val) in signature_vector.iter().enumerate() {
            dot += self.weights[i] * sig_val;
        }
        (self.base_vol + dot).clamp(0.01, 5.0)
    }
}

/// Observed quote from an options exchange for a particular strike/tenor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsMarketQuote {
    pub symbol: String,
    pub venue: String,
    pub is_call: bool,
    pub strike: f64,
    pub spot_price: f64,
    pub tenor_years: f64,
    pub observed_iv: f64,
    pub delta: f64,
    pub vega: f64,
    pub bid_price_usd: f64,
    pub ask_price_usd: f64,
}

/// An individual trade leg in a dispersion basket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispersionTradeLeg {
    pub symbol: String,
    pub venue: String,
    pub is_buy: bool,
    pub quantity: f64,
    pub execution_price_usd: f64,
    pub iv_mispricing_bps: f64,
    pub delta_contribution: f64,
    pub vega_contribution: f64,
}

/// Multi-leg dispersion arbitrage signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSmileDispersionSignal {
    pub underlying_symbol: String,
    pub legs: Vec<DispersionTradeLeg>,
    pub net_delta: f64,
    pub net_vega: f64,
    pub expected_edge_usd: f64,
    pub required_margin_usd: f64,
    pub timestamp_ms: i64,
}

/// Strategy Engine for Path Signature Surface Dispersion.
pub struct PathSignatureSurfaceDispersionEngine {
    pub underlying_symbol: String,
    pub tracker: RoughPathSignatureEngine,
    pub kernels: Vec<SignatureSurfaceKernel>,
    pub entry_threshold_vol_pts: f64,
    pub min_edge_usd: f64,
    pub max_net_delta_tolerance: f64,
}

impl PathSignatureSurfaceDispersionEngine {
    pub fn new(
        underlying_symbol: String,
        initial_point: RoughPathPoint,
        kernels: Vec<SignatureSurfaceKernel>,
        entry_threshold_vol_pts: f64,
        min_edge_usd: f64,
    ) -> Self {
        let mut tracker = RoughPathSignatureEngine::new();
        tracker.update(initial_point);
        Self {
            underlying_symbol,
            tracker,
            kernels,
            entry_threshold_vol_pts: entry_threshold_vol_pts.max(0.005), // min 0.5 vol pts
            min_edge_usd: min_edge_usd.max(5.0),
            max_net_delta_tolerance: 0.05,
        }
    }

    /// Ingests streaming market update and updates path signature in exact O(84) time.
    pub fn update_market_path(&mut self, next_point: RoughPathPoint) {
        self.tracker.update(next_point);
    }

    /// Evaluates current signature surface for all registered kernels.
    pub fn evaluate_surface(&self) -> Vec<(SurfaceCoordinate, f64)> {
        let sig_vec = self.tracker.current_signature.to_flat_vector();
        self.kernels
            .iter()
            .map(|k| (k.coord.clone(), k.predict_vol(&sig_vec)))
            .collect()
    }

    /// Identifies mispricings and constructs delta-neutral, vega-neutral dispersion baskets.
    pub fn scan_dispersion_opportunity(
        &self,
        quotes: &[OptionsMarketQuote],
        timestamp_ms: i64,
    ) -> Option<CrossSmileDispersionSignal> {
        let sig_vec = self.tracker.current_signature.to_flat_vector();

        let mut underpriced_quotes: Vec<(&OptionsMarketQuote, f64)> = Vec::new();
        let mut overpriced_quotes: Vec<(&OptionsMarketQuote, f64)> = Vec::new();

        for quote in quotes {
            let log_m = (quote.strike / quote.spot_price).ln();

            // Find closest matching kernel on grid
            let matching_kernel = self.kernels.iter().min_by(|a, b| {
                let dist_a =
                    (a.coord.log_moneyness - log_m).hypot(a.coord.tenor_years - quote.tenor_years);
                let dist_b =
                    (b.coord.log_moneyness - log_m).hypot(b.coord.tenor_years - quote.tenor_years);
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(kernel) = matching_kernel {
                let model_iv = kernel.predict_vol(&sig_vec);
                let mispricing = quote.observed_iv - model_iv;

                if mispricing < -self.entry_threshold_vol_pts {
                    // Underpriced in market: Buy candidate
                    underpriced_quotes.push((quote, mispricing.abs()));
                } else if mispricing > self.entry_threshold_vol_pts {
                    // Overpriced in market: Sell candidate
                    overpriced_quotes.push((quote, mispricing));
                }
            }
        }

        if underpriced_quotes.is_empty() || overpriced_quotes.is_empty() {
            return None;
        }

        // Sort by greatest mispricing edge
        underpriced_quotes
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        overpriced_quotes
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (buy_candidate, buy_edge_iv) = underpriced_quotes[0];
        let (sell_candidate, sell_edge_iv) = overpriced_quotes[0];

        // Solve for vega-neutral hedge ratio:
        // q_buy * vega_buy = q_sell * vega_sell => q_sell = q_buy * (vega_buy / vega_sell)
        let q_buy = 1.0;
        let vega_ratio = buy_candidate.vega / sell_candidate.vega.max(1e-4);
        let q_sell = (q_buy * vega_ratio).clamp(0.1, 10.0);

        let delta_buy = q_buy * buy_candidate.delta;
        let delta_sell = -q_sell * sell_candidate.delta;
        let net_options_delta = delta_buy + delta_sell;

        // Underlying delta rebalance leg:
        let underlying_hedge_qty = -net_options_delta;

        let buy_leg = DispersionTradeLeg {
            symbol: buy_candidate.symbol.clone(),
            venue: buy_candidate.venue.clone(),
            is_buy: true,
            quantity: q_buy,
            execution_price_usd: buy_candidate.ask_price_usd,
            iv_mispricing_bps: buy_edge_iv * 10_000.0,
            delta_contribution: delta_buy,
            vega_contribution: q_buy * buy_candidate.vega,
        };

        let sell_leg = DispersionTradeLeg {
            symbol: sell_candidate.symbol.clone(),
            venue: sell_candidate.venue.clone(),
            is_buy: false,
            quantity: q_sell,
            execution_price_usd: sell_candidate.bid_price_usd,
            iv_mispricing_bps: sell_edge_iv * 10_000.0,
            delta_contribution: delta_sell,
            vega_contribution: -q_sell * sell_candidate.vega,
        };

        let underlying_leg = DispersionTradeLeg {
            symbol: format!("{}-PERP", self.underlying_symbol),
            venue: buy_candidate.venue.clone(),
            is_buy: underlying_hedge_qty > 0.0,
            quantity: underlying_hedge_qty.abs(),
            execution_price_usd: buy_candidate.spot_price,
            iv_mispricing_bps: 0.0,
            delta_contribution: underlying_hedge_qty,
            vega_contribution: 0.0,
        };

        let final_net_delta = delta_buy + delta_sell + underlying_hedge_qty;
        let final_net_vega = (q_buy * buy_candidate.vega) - (q_sell * sell_candidate.vega);

        // Expected edge USD: sum of mispricings times vega minus taker fees
        let total_edge_usd = (buy_edge_iv * buy_candidate.vega * 100.0 * q_buy)
            + (sell_edge_iv * sell_candidate.vega * 100.0 * q_sell)
            - 15.0; // conservative fee & friction estimate

        if total_edge_usd < self.min_edge_usd {
            return None;
        }

        let required_margin = (q_buy * buy_candidate.ask_price_usd
            + q_sell * sell_candidate.bid_price_usd
            + underlying_hedge_qty.abs() * buy_candidate.spot_price * 0.1)
            .max(100.0);

        Some(CrossSmileDispersionSignal {
            underlying_symbol: self.underlying_symbol.clone(),
            legs: vec![buy_leg, sell_leg, underlying_leg],
            net_delta: final_net_delta,
            net_vega: final_net_vega,
            expected_edge_usd: total_edge_usd,
            required_margin_usd: required_margin,
            timestamp_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_surface_kernel_prediction() {
        let mut weights = vec![0.0; 84];
        // Weight on Level 1 price increment (index 1) and rough vol (index 3)
        weights[1] = 0.05;
        weights[3] = 0.10;

        let kernel = SignatureSurfaceKernel::new(
            SurfaceCoordinate {
                log_moneyness: 0.0,
                tenor_years: 30.0 / 365.0,
            },
            0.55,
            weights,
        );

        let mut sig_vec = [0.0; 84];
        sig_vec[1] = 0.2; // positive price run
        sig_vec[3] = 0.4; // elevated rough vol

        let predicted_iv = kernel.predict_vol(&sig_vec);
        // Base 0.55 + 0.05 * 0.2 (0.01) + 0.10 * 0.4 (0.04) = 0.60
        assert!((predicted_iv - 0.60).abs() < 1e-4);
    }

    #[test]
    fn test_dispersion_opportunity_scanning_and_delta_neutrality() {
        let p0 = RoughPathPoint::new(0.0, 11.0, 0.0, 0.50);
        let mut weights_atm = vec![0.0; 84];
        weights_atm[0] = 0.01;

        let kernels = vec![
            SignatureSurfaceKernel::new(
                SurfaceCoordinate {
                    log_moneyness: -0.1,
                    tenor_years: 0.1,
                },
                0.60,
                weights_atm.clone(),
            ),
            SignatureSurfaceKernel::new(
                SurfaceCoordinate {
                    log_moneyness: 0.1,
                    tenor_years: 0.1,
                },
                0.55,
                weights_atm,
            ),
        ];

        let mut engine = PathSignatureSurfaceDispersionEngine::new(
            "BTC".into(),
            p0,
            kernels,
            0.03, // 3 vol pts threshold
            20.0, // $20 min edge
        );

        let p1 = RoughPathPoint::new(1.0, 11.05, 0.3, 0.52);
        engine.update_market_path(p1);

        // Quotes:
        // Put (log_moneyness -0.1): Model IV ~ 0.61. Market IV = 0.54 (Underpriced by 7 pts -> Buy!)
        // Call (log_moneyness +0.1): Model IV ~ 0.56. Market IV = 0.63 (Overpriced by 7 pts -> Sell!)
        let quotes = vec![
            OptionsMarketQuote {
                symbol: "BTC-PUT-55000".into(),
                venue: "deribit".into(),
                is_call: false,
                strike: 55_000.0,
                spot_price: 60_000.0,
                tenor_years: 0.1,
                observed_iv: 0.54,
                delta: -0.25,
                vega: 15.0,
                bid_price_usd: 800.0,
                ask_price_usd: 820.0,
            },
            OptionsMarketQuote {
                symbol: "BTC-CALL-66000".into(),
                venue: "deribit".into(),
                is_call: true,
                strike: 66_000.0,
                spot_price: 60_000.0,
                tenor_years: 0.1,
                observed_iv: 0.63,
                delta: 0.30,
                vega: 15.0,
                bid_price_usd: 950.0,
                ask_price_usd: 970.0,
            },
        ];

        let opportunity = engine
            .scan_dispersion_opportunity(&quotes, 1791200000000)
            .expect("Expected dispersion opportunity");

        assert_eq!(opportunity.legs.len(), 3);
        // Verify legs: Buy put, Sell call, Hedge underlying
        assert!(opportunity.legs[0].is_buy);
        assert!(!opportunity.legs[1].is_buy);

        // Verify delta neutrality: net delta must be essentially 0.0
        assert!(opportunity.net_delta.abs() < 1e-4);
        // Verify vega neutrality: net vega close to 0.0
        assert!(opportunity.net_vega.abs() < 1e-4);
        // Verify positive net expected edge
        assert!(opportunity.expected_edge_usd >= 20.0);
    }
}
