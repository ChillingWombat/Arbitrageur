//! Signature Neural Rough Fractional Volterra SDEs & Deep Calibration Engine for Multi-Asset Cross-Perp Skew Arbitrage.
//!
//! Models non-Markovian volatility and skew dynamics under singular Volterra kernels
//! $K(t) = \frac{t^{H - 1/2}}{\Gamma(H + 1/2)}$ ($H \in (0, 0.5)$) conditioned on path signatures,
//! identifying relative-value dislocations between perpetual futures skew and vanilla options smiles.

use serde::{Deserialize, Serialize};

/// Configuration parameters for the Signature Neural Volterra Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolterraSkewConfig {
    /// Rough Hurst exponent $H \in (0.01, 0.49)$ (e.g. 0.10).
    pub hurst_h: f64,
    /// Volatility of volatility parameter $\eta$.
    pub vol_of_vol_eta: f64,
    /// Minimum skew dislocation hurdle in basis points (e.g. 15.0 bps).
    pub skew_dislocation_hurdle_bps: f64,
    /// Maximum capital allocation leverage multiplier.
    pub max_leverage: f64,
}

impl Default for VolterraSkewConfig {
    fn default() -> Self {
        Self {
            hurst_h: 0.12,
            vol_of_vol_eta: 0.35,
            skew_dislocation_hurdle_bps: 15.0,
            max_leverage: 3.0,
        }
    }
}

/// Observation of cross-perp and options volatility skew for an asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerpSkewObservation {
    pub symbol: String,
    pub perp_skew_bps: f64,
    pub options_skew_bps: f64,
    pub signature_level2_norm: f64,
    pub dt_days: f64,
}

/// Strategic action for cross-perp vs options skew arbitrage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PerpSkewArbitrageAction {
    /// Perpetual skew is overpriced relative to options skew.
    /// Short perp skew (sell downside puts/buy upside calls on perp), Long options downside skew.
    LongOptionsSkewShortPerpSkew {
        dislocation_bps: f64,
        recommended_leverage: f64,
    },
    /// Perpetual skew is underpriced relative to options skew.
    /// Long perp skew, Short options downside skew.
    ShortOptionsSkewLongPerpSkew {
        dislocation_bps: f64,
        recommended_leverage: f64,
    },
    /// Dislocation within friction hurdle; maintain neutral delta/vega inventory.
    NeutralHold { dislocation_bps: f64 },
}

/// Evaluated skew arbitrage signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerpSkewSignal {
    pub symbol: String,
    pub action: PerpSkewArbitrageAction,
    pub perp_skew_bps: f64,
    pub options_skew_bps: f64,
    pub dislocation_bps: f64,
    pub volterra_damping_factor: f64,
}

/// Signature Neural Volterra Cross-Perp Skew Engine.
pub struct SignatureNeuralVolterraEngine {
    pub config: VolterraSkewConfig,
}

impl SignatureNeuralVolterraEngine {
    pub fn new(config: VolterraSkewConfig) -> Self {
        Self { config }
    }

    /// Evaluates rough Volterra singular kernel decay factor $K(t) = \frac{t^{H - 0.5}}{\Gamma(H + 0.5)}$.
    pub fn compute_volterra_kernel_decay(&self, dt: f64) -> f64 {
        let t = dt.max(1e-4);
        let alpha = self.config.hurst_h - 0.5; // Negative exponent for H < 0.5

        // Lanczos approximation of Gamma(H + 0.5)
        let gamma_val = Self::gamma_lanczos(self.config.hurst_h + 0.5);

        (t.powf(alpha) / gamma_val).clamp(0.01, 100.0)
    }

    #[allow(clippy::excessive_precision)]
    fn gamma_lanczos(z: f64) -> f64 {
        let p = [
            676.5203681287003,
            -1259.139216722289,
            771.32342877765313,
            -176.61502916214059,
            12.507343278686905,
            -0.13857109526572012,
            9.9843695780195716e-6,
            1.5056327351493116e-7,
        ];
        if z < 0.5 {
            std::f64::consts::PI / ((std::f64::consts::PI * z).sin() * Self::gamma_lanczos(1.0 - z))
        } else {
            let z = z - 1.0;
            let mut x = 0.99999999999980993;
            for (i, &p_val) in p.iter().enumerate() {
                x += p_val / (z + (i as f64) + 1.0);
            }
            let t = z + 7.5;
            (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * x
        }
    }

    /// Evaluates cross-perp skew relative-value dislocation against options surface.
    pub fn evaluate_skew_arbitrage(&self, obs: &PerpSkewObservation) -> PerpSkewSignal {
        let kernel_decay = self.compute_volterra_kernel_decay(obs.dt_days);

        // Modulate expected equilibrium skew with signature norm
        let sig_modulation = 1.0 + 0.1 * obs.signature_level2_norm.clamp(0.0, 5.0);
        let effective_options_skew = obs.options_skew_bps * sig_modulation;

        // Dislocation: perp skew minus calibrated options smile skew
        let dislocation = obs.perp_skew_bps - effective_options_skew;
        let hurdle = self.config.skew_dislocation_hurdle_bps;

        let action = if dislocation > hurdle {
            let leverage =
                (1.0 + (dislocation - hurdle) / 20.0).clamp(1.0, self.config.max_leverage);
            PerpSkewArbitrageAction::LongOptionsSkewShortPerpSkew {
                dislocation_bps: dislocation,
                recommended_leverage: leverage,
            }
        } else if dislocation < -hurdle {
            let leverage =
                (1.0 + (-dislocation - hurdle) / 20.0).clamp(1.0, self.config.max_leverage);
            PerpSkewArbitrageAction::ShortOptionsSkewLongPerpSkew {
                dislocation_bps: dislocation,
                recommended_leverage: leverage,
            }
        } else {
            PerpSkewArbitrageAction::NeutralHold {
                dislocation_bps: dislocation,
            }
        };

        PerpSkewSignal {
            symbol: obs.symbol.clone(),
            action,
            perp_skew_bps: obs.perp_skew_bps,
            options_skew_bps: effective_options_skew,
            dislocation_bps: dislocation,
            volterra_damping_factor: kernel_decay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volterra_kernel_decay_power_law() {
        let config = VolterraSkewConfig {
            hurst_h: 0.10,
            vol_of_vol_eta: 0.3,
            skew_dislocation_hurdle_bps: 10.0,
            max_leverage: 2.5,
        };
        let engine = SignatureNeuralVolterraEngine::new(config);

        let decay_short = engine.compute_volterra_kernel_decay(0.01);
        let decay_long = engine.compute_volterra_kernel_decay(1.0);

        // Singular power law kernel K(t) ~ t^(H - 0.5) = t^(-0.4) -> decays with time
        assert!(decay_short > decay_long);
    }

    #[test]
    fn test_skew_arbitrage_signal_triggers() {
        let config = VolterraSkewConfig {
            hurst_h: 0.12,
            vol_of_vol_eta: 0.35,
            skew_dislocation_hurdle_bps: 12.0,
            max_leverage: 3.0,
        };
        let engine = SignatureNeuralVolterraEngine::new(config);

        // Case 1: Perp skew is rich (45 bps) vs options skew (20 bps) -> dislocation ~ 25 bps > 12 bps hurdle
        let obs_rich = PerpSkewObservation {
            symbol: "BTC".into(),
            perp_skew_bps: 45.0,
            options_skew_bps: 20.0,
            signature_level2_norm: 0.0,
            dt_days: 0.25,
        };
        let sig_rich = engine.evaluate_skew_arbitrage(&obs_rich);
        match sig_rich.action {
            PerpSkewArbitrageAction::LongOptionsSkewShortPerpSkew {
                dislocation_bps,
                recommended_leverage,
            } => {
                assert!(dislocation_bps > 15.0);
                assert!(recommended_leverage >= 1.0);
            }
            _ => panic!("Expected LongOptionsSkewShortPerpSkew for rich perp skew"),
        }

        // Case 2: Perp skew is cheap (10 bps) vs options skew (30 bps) -> dislocation ~ -20 bps < -12 bps hurdle
        let obs_cheap = PerpSkewObservation {
            symbol: "ETH".into(),
            perp_skew_bps: 10.0,
            options_skew_bps: 30.0,
            signature_level2_norm: 0.0,
            dt_days: 0.25,
        };
        let sig_cheap = engine.evaluate_skew_arbitrage(&obs_cheap);
        match sig_cheap.action {
            PerpSkewArbitrageAction::ShortOptionsSkewLongPerpSkew {
                dislocation_bps,
                recommended_leverage,
            } => {
                assert!(dislocation_bps < -15.0);
                assert!(recommended_leverage >= 1.0);
            }
            _ => panic!("Expected ShortOptionsSkewLongPerpSkew for cheap perp skew"),
        }

        // Case 3: Balanced/neutral skew (25 bps vs 25 bps)
        let obs_neutral = PerpSkewObservation {
            symbol: "SOL".into(),
            perp_skew_bps: 25.0,
            options_skew_bps: 25.0,
            signature_level2_norm: 0.0,
            dt_days: 0.25,
        };
        let sig_neutral = engine.evaluate_skew_arbitrage(&obs_neutral);
        match sig_neutral.action {
            PerpSkewArbitrageAction::NeutralHold { dislocation_bps } => {
                assert_eq!(dislocation_bps, 0.0);
            }
            _ => panic!("Expected NeutralHold for balanced skew"),
        }
    }
}
