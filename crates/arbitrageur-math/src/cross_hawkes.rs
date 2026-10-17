//! Multi-Asset High-Frequency Cross-Hawkes Co-Jump Intensity & Asymmetric Cross-Impact Engine.
//!
//! Models cross-asset and cross-venue mutual jump excitations, contagion cascades,
//! and directional order flow spillovers (e.g. BTC cascades exciting ETH/SOL liquidations).
//!
//! Features:
//! 1. Structured block cross-excitation matrix $\mathbf{A} = \mathbf{A}_{\text{self}} + \mathbf{A}_{\text{lead-lag}} + \mathbf{A}_{\text{cross-asset}}$.
//! 2. Rayleigh quotient power iteration for spectral radius $\rho(\mathbf{A})$ under Perron-Frobenius stationarity ($\rho < 1$).
//! 3. Cross-asset flow imbalance $\mathbf{I}_{\text{flow}}$ and toxic reservation price drift $\Delta \mathbf{r}_{\text{toxic}}$.
//! 4. Asymmetric cross-impact kernel $\Gamma_{ij}(\tau) = \rho_{ij} \sqrt{\Gamma_{ii}\Gamma_{jj}} (\text{ADV}_i / \text{ADV}_j)^{0.25} e^{-\beta \tau}$.
//! 5. Sub-1.5ms toxic co-jump quote withdrawal circuit breaker.

use serde::{Deserialize, Serialize};

/// High-frequency mutual excitation market regime based on spectral radius $\rho(\mathbf{A})$.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossHawkesRegime {
    /// Independent, exogenous arrivals with rapid mean reversion ($\rho < 0.70$).
    Subcritical,
    /// Clustered endogenous arrivals; elevated cross-asset spread variance ($0.70 \le \rho < 0.92$).
    Critical,
    /// Self-exciting cascading blowout; cross-venue adverse selection imminent ($\rho \ge 0.92$).
    Supercritical,
}

/// Configuration for the multi-asset cross-Hawkes engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossHawkesConfig {
    pub num_assets: usize,
    pub num_venues: usize,
    pub baseline_intensity: f64,
    pub self_excitation_alpha: f64,
    pub cross_venue_excitation_alpha: f64,
    pub cross_asset_excitation_alpha: f64,
    pub decay_beta: f64,
    pub beta_impact_decay: f64,
    pub toxic_cojump_threshold: f64,
}

impl Default for CrossHawkesConfig {
    fn default() -> Self {
        Self {
            num_assets: 3, // e.g. BTC, ETH, SOL
            num_venues: 2, // e.g. Binance, Hyperliquid
            baseline_intensity: 0.10,
            self_excitation_alpha: 0.15,
            cross_venue_excitation_alpha: 0.08,
            cross_asset_excitation_alpha: 0.05,
            decay_beta: 1.20,
            beta_impact_decay: 2.50,
            toxic_cojump_threshold: 4.50,
        }
    }
}

/// Stream identifier for a specific asset, venue, and side.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamKey {
    pub asset_index: usize,
    pub venue_index: usize,
    pub is_buy: bool,
}

/// Multi-Asset High-Frequency Cross-Hawkes Co-Jump Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAssetCrossHawkesEngine {
    pub config: CrossHawkesConfig,
    pub total_streams: usize,
    pub asset_symbols: Vec<String>,
    pub venue_names: Vec<String>,
    pub baseline_mu: Vec<f64>,
    pub alpha_matrix: Vec<Vec<f64>>,
    pub beta_matrix: Vec<Vec<f64>>,
    pub excited_state: Vec<Vec<f64>>,
    pub adv_usd: Vec<f64>,
    pub correlation_matrix: Vec<Vec<f64>>,
    pub self_impact_gamma: Vec<f64>,
    pub last_timestamp_sec: Option<f64>,
}

impl MultiAssetCrossHawkesEngine {
    /// Constructs a structured Multi-Asset Cross-Hawkes Engine with block excitation.
    #[allow(clippy::needless_range_loop)]
    pub fn new(
        asset_symbols: Vec<String>,
        venue_names: Vec<String>,
        adv_usd: Vec<f64>,
        correlations: Vec<Vec<f64>>,
        self_impact: Vec<f64>,
        config: CrossHawkesConfig,
    ) -> Self {
        let num_assets = asset_symbols.len();
        let num_venues = venue_names.len();
        // Each asset x venue has 2 streams: Buy (0) and Sell (1)
        let total_streams = num_assets * num_venues * 2;

        let baseline_mu = vec![config.baseline_intensity; total_streams];
        let mut alpha_matrix = vec![vec![0.0; total_streams]; total_streams];
        let beta_matrix = vec![vec![config.decay_beta; total_streams]; total_streams];
        let excited_state = vec![vec![0.0; total_streams]; total_streams];

        // Populate structured block excitation matrix A_ij
        for i in 0..total_streams {
            let (asset_i, venue_i, is_buy_i) = Self::decode_stream(i, num_venues);
            for j in 0..total_streams {
                let (asset_j, venue_j, is_buy_j) = Self::decode_stream(j, num_venues);

                let is_same_asset = asset_i == asset_j;
                let is_same_venue = venue_i == venue_j;
                let is_same_side = is_buy_i == is_buy_j;

                if is_same_asset && is_same_venue && is_same_side {
                    // 1. Self-excitation
                    alpha_matrix[i][j] = config.self_excitation_alpha;
                } else if is_same_asset && !is_same_venue {
                    // 2. Cross-venue lead-lag excitation for the same asset
                    let lead_lag_alpha = config.cross_venue_excitation_alpha;
                    alpha_matrix[i][j] = if is_same_side {
                        lead_lag_alpha
                    } else {
                        lead_lag_alpha * 0.20 // Opposite side cross-arbitrage friction
                    };
                } else if !is_same_asset {
                    // 3. Cross-asset co-jump excitation scaled by return correlation
                    let rho =
                        if asset_i < correlations.len() && asset_j < correlations[asset_i].len() {
                            correlations[asset_i][asset_j].abs().clamp(0.0, 1.0)
                        } else {
                            0.50
                        };
                    let cross_alpha = config.cross_asset_excitation_alpha * rho;
                    alpha_matrix[i][j] = if is_same_side {
                        cross_alpha
                    } else {
                        cross_alpha * 0.10
                    };
                } else {
                    // Same asset, same venue, opposite side (e.g. Buy exciting Sell)
                    alpha_matrix[i][j] = config.self_excitation_alpha * 0.25;
                }
            }
        }

        Self {
            config,
            total_streams,
            asset_symbols,
            venue_names,
            baseline_mu,
            alpha_matrix,
            beta_matrix,
            excited_state,
            adv_usd,
            correlation_matrix: correlations,
            self_impact_gamma: self_impact,
            last_timestamp_sec: None,
        }
    }

    /// Decodes stream index into (asset_index, venue_index, is_buy).
    #[inline]
    pub fn decode_stream(stream_idx: usize, num_venues: usize) -> (usize, usize, bool) {
        let is_buy = stream_idx.is_multiple_of(2);
        let pair_idx = stream_idx / 2;
        let asset_idx = pair_idx / num_venues;
        let venue_idx = pair_idx % num_venues;
        (asset_idx, venue_idx, is_buy)
    }

    /// Encodes (asset_index, venue_index, is_buy) into a stream index.
    #[inline]
    pub fn encode_stream(
        asset_idx: usize,
        venue_idx: usize,
        is_buy: bool,
        num_venues: usize,
    ) -> usize {
        let pair_idx = asset_idx * num_venues + venue_idx;
        pair_idx * 2 + if is_buy { 0 } else { 1 }
    }

    /// Computes the Branching Ratio Matrix $\mathbf{\Gamma}_{ij} = \alpha_{ij} / \beta_{ij}$.
    #[allow(clippy::needless_range_loop)]
    pub fn branching_ratio_matrix(&self) -> Vec<Vec<f64>> {
        let mut gamma = vec![vec![0.0; self.total_streams]; self.total_streams];
        for i in 0..self.total_streams {
            for j in 0..self.total_streams {
                let a = self.alpha_matrix[i][j].max(0.0);
                let b = self.beta_matrix[i][j].max(1e-6);
                gamma[i][j] = a / b;
            }
        }
        gamma
    }

    /// Computes the spectral radius $\rho(\mathbf{\Gamma})$ using Rayleigh quotient power iteration.
    pub fn spectral_radius(&self) -> f64 {
        let n = self.total_streams;
        if n == 0 {
            return 0.0;
        }
        if n == 1 {
            return (self.alpha_matrix[0][0] / self.beta_matrix[0][0].max(1e-6)).max(0.0);
        }

        let gamma = self.branching_ratio_matrix();
        let mut v = vec![1.0 / (n as f64).sqrt(); n];

        for _ in 0..45 {
            let mut next_v = vec![0.0; n];
            for i in 0..n {
                for j in 0..n {
                    next_v[i] += gamma[i][j] * v[j];
                }
            }
            let norm = next_v.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm < 1e-12 {
                return 0.0;
            }
            for i in 0..n {
                v[i] = next_v[i] / norm;
            }
        }

        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..n {
            let mut gv_i = 0.0;
            for j in 0..n {
                gv_i += gamma[i][j] * v[j];
            }
            num += v[i] * gv_i;
            den += v[i] * v[i];
        }

        if den.abs() < 1e-12 {
            0.0
        } else {
            (num / den).max(0.0)
        }
    }

    /// Classifies the global market regime across all monitored assets and venues.
    pub fn classify_regime(&self) -> CrossHawkesRegime {
        let rho = self.spectral_radius();
        if rho < 0.70 {
            CrossHawkesRegime::Subcritical
        } else if rho < 0.92 {
            CrossHawkesRegime::Critical
        } else {
            CrossHawkesRegime::Supercritical
        }
    }

    /// Evaluates the asymmetric cross-impact kernel $\Gamma_{ij}(\tau)$.
    pub fn asymmetric_cross_impact(&self, asset_i: usize, asset_j: usize, tau_sec: f64) -> f64 {
        if asset_i >= self.adv_usd.len() || asset_j >= self.adv_usd.len() {
            return 0.0;
        }
        let tau = tau_sec.max(0.0);
        let rho = if asset_i < self.correlation_matrix.len()
            && asset_j < self.correlation_matrix[asset_i].len()
        {
            self.correlation_matrix[asset_i][asset_j].clamp(-1.0, 1.0)
        } else {
            0.0
        };

        let gamma_i = if asset_i < self.self_impact_gamma.len() {
            self.self_impact_gamma[asset_i].max(1e-6)
        } else {
            1e-4
        };
        let gamma_j = if asset_j < self.self_impact_gamma.len() {
            self.self_impact_gamma[asset_j].max(1e-6)
        } else {
            1e-4
        };

        let adv_i = self.adv_usd[asset_i].max(1.0);
        let adv_j = self.adv_usd[asset_j].max(1.0);
        let adv_ratio = (adv_i / adv_j).powf(0.25);

        rho * (gamma_i * gamma_j).sqrt() * adv_ratio * (-self.config.beta_impact_decay * tau).exp()
    }

    /// Decays intensities and records a discrete jump or aggressive sweep event.
    pub fn record_event(&mut self, stream_idx: usize, timestamp_sec: f64) {
        if stream_idx >= self.total_streams {
            return;
        }

        if let Some(prev_t) = self.last_timestamp_sec {
            let dt = (timestamp_sec - prev_t).max(0.0);
            for i in 0..self.total_streams {
                for j in 0..self.total_streams {
                    let beta = self.beta_matrix[i][j].max(1e-6);
                    self.excited_state[i][j] *= (-beta * dt).exp();
                }
            }
        }

        for i in 0..self.total_streams {
            self.excited_state[i][stream_idx] += self.alpha_matrix[i][stream_idx];
        }

        self.last_timestamp_sec = Some(timestamp_sec);
    }

    /// Computes current instantaneous arrival intensities across all streams.
    #[allow(clippy::needless_range_loop)]
    pub fn current_intensities(&self, current_time_sec: f64) -> Vec<f64> {
        let dt = match self.last_timestamp_sec {
            Some(prev_t) => (current_time_sec - prev_t).max(0.0),
            None => 0.0,
        };

        let mut intensities = vec![0.0; self.total_streams];
        for i in 0..self.total_streams {
            let mut sum_excited = 0.0;
            for j in 0..self.total_streams {
                let beta = self.beta_matrix[i][j].max(1e-6);
                sum_excited += self.excited_state[i][j] * (-beta * dt).exp();
            }
            intensities[i] = self.baseline_mu[i] + sum_excited;
        }
        intensities
    }

    /// Computes normalized directional order flow imbalance $I_{\text{flow}, k} \in [-1, 1]$ per asset.
    pub fn asset_flow_imbalances(&self, current_time_sec: f64) -> Vec<f64> {
        let intensities = self.current_intensities(current_time_sec);
        let num_venues = self.venue_names.len();
        let mut imbalances = vec![0.0; self.asset_symbols.len()];

        for (k, imb) in imbalances.iter_mut().enumerate() {
            let mut buy_intensity = 0.0;
            let mut sell_intensity = 0.0;
            for v in 0..num_venues {
                let buy_idx = Self::encode_stream(k, v, true, num_venues);
                let sell_idx = Self::encode_stream(k, v, false, num_venues);
                buy_intensity += intensities[buy_idx];
                sell_intensity += intensities[sell_idx];
            }
            let denom = buy_intensity + sell_intensity;
            *imb = if denom > 1e-6 {
                (buy_intensity - sell_intensity) / denom
            } else {
                0.0
            };
        }

        imbalances
    }

    /// Evaluates cross-asset directional toxic reservation shift $\Delta r_{\text{toxic}, k}$ (in bps).
    pub fn toxic_reservation_drift_bps(&self, asset_idx: usize, current_time_sec: f64) -> f64 {
        let imbalances = self.asset_flow_imbalances(current_time_sec);
        if asset_idx >= imbalances.len() {
            return 0.0;
        }

        let mut total_drift_bps = 0.0;
        for (j, imb_j) in imbalances.iter().enumerate() {
            let rho = if asset_idx < self.correlation_matrix.len()
                && j < self.correlation_matrix[asset_idx].len()
            {
                self.correlation_matrix[asset_idx][j]
            } else if asset_idx == j {
                1.0
            } else {
                0.50
            };

            let weight = if asset_idx == j {
                1.0
            } else {
                0.40 * rho.abs()
            };
            total_drift_bps += imb_j * weight * 15.0; // 15 bps base adverse scaling
        }

        total_drift_bps.clamp(-50.0, 50.0)
    }

    /// Checks if a joint toxic co-jump event mandates immediate quote withdrawal.
    pub fn check_cojump_quote_withdrawal(
        &self,
        asset_i: usize,
        asset_j: usize,
        current_time_sec: f64,
    ) -> (bool, f64) {
        let intensities = self.current_intensities(current_time_sec);
        let num_venues = self.venue_names.len();

        let mut lambda_i = 0.0;
        let mut lambda_j = 0.0;
        for v in 0..num_venues {
            let buy_i = Self::encode_stream(asset_i, v, true, num_venues);
            let sell_i = Self::encode_stream(asset_i, v, false, num_venues);
            let buy_j = Self::encode_stream(asset_j, v, true, num_venues);
            let sell_j = Self::encode_stream(asset_j, v, false, num_venues);

            lambda_i += intensities[buy_i] + intensities[sell_i];
            lambda_j += intensities[buy_j] + intensities[sell_j];
        }

        let mu_i = (self.config.baseline_intensity * num_venues as f64 * 2.0).max(1e-6);
        let mu_j = (self.config.baseline_intensity * num_venues as f64 * 2.0).max(1e-6);

        let dt = match self.last_timestamp_sec {
            Some(prev_t) => (current_time_sec - prev_t).max(0.0),
            None => 0.0,
        };

        let mut exc_i = 0.0;
        let mut exc_j = 0.0;
        for v in 0..num_venues {
            let buy_i = Self::encode_stream(asset_i, v, true, num_venues);
            let buy_j = Self::encode_stream(asset_j, v, true, num_venues);
            for k in 0..self.total_streams {
                exc_i += self.excited_state[buy_i][k] * (-self.beta_matrix[buy_i][k] * dt).exp();
                exc_j += self.excited_state[buy_j][k] * (-self.beta_matrix[buy_j][k] * dt).exp();
            }
        }

        let rho = if asset_i < self.correlation_matrix.len()
            && asset_j < self.correlation_matrix[asset_i].len()
        {
            self.correlation_matrix[asset_i][asset_j].abs()
        } else {
            0.50
        };

        let joint_hazard = ((lambda_i * lambda_j) / (mu_i * mu_j).sqrt()) * (0.25 + 0.75 * rho)
            + (exc_i.max(0.0) * exc_j.max(0.0)).sqrt();

        let should_withdraw = joint_hazard >= self.config.toxic_cojump_threshold
            || self.classify_regime() == CrossHawkesRegime::Supercritical;

        (should_withdraw, joint_hazard)
    }
}

/// Verdict from the Multi-Level Hawkes Queue Drain Forecaster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueDrainVerdict {
    pub pickoff_probability: f64,
    pub drain_velocity: f64,
    pub replenishment_velocity: f64,
    pub drain_to_add_ratio: f64,
    pub is_l2_cancellation_surging: bool,
    pub should_evacuate_passive_quote: bool,
    pub reason: String,
}

/// Multi-Level Hawkes Queue Drain Forecaster.
/// Models the cross-excitation between Level 1 trade sweeps, Level 1 cancellations,
/// Level 2 sub-touch precursor cancellations, and passive quote replenishment additions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MultiLevelHawkesQueueDrainForecaster {
    /// Arrival intensity of aggressive trade sweeps hitting Level 1 (\lambda_1)
    pub sweep_intensity: f64,
    /// Arrival intensity of limit order cancellations at Level 1 (\lambda_2)
    pub cancel_l1_intensity: f64,
    /// Arrival intensity of precursor limit order cancellations at Level 2 (\lambda_3)
    pub cancel_l2_intensity: f64,
    /// Arrival intensity of passive limit order additions at Level 1 (\lambda_4)
    pub add_l1_intensity: f64,
    /// Mean order size for sweeps
    pub mean_sweep_size: f64,
    /// Mean order size for cancellations
    pub mean_cancel_size: f64,
    /// Mean order size for additions
    pub mean_add_size: f64,
    /// Critical pick-off probability threshold (default 0.68)
    pub critical_pickoff_threshold: f64,
    /// Critical drain-to-add ratio threshold (default 3.80)
    pub critical_drain_add_ratio: f64,
}

impl Default for MultiLevelHawkesQueueDrainForecaster {
    fn default() -> Self {
        Self {
            sweep_intensity: 1.0,
            cancel_l1_intensity: 1.2,
            cancel_l2_intensity: 0.8,
            add_l1_intensity: 2.0,
            mean_sweep_size: 1.0,
            mean_cancel_size: 1.0,
            mean_add_size: 1.0,
            critical_pickoff_threshold: 0.68,
            critical_drain_add_ratio: 3.80,
        }
    }
}

impl MultiLevelHawkesQueueDrainForecaster {
    pub fn new(
        sweep_intensity: f64,
        cancel_l1_intensity: f64,
        cancel_l2_intensity: f64,
        add_l1_intensity: f64,
    ) -> Self {
        Self {
            sweep_intensity: sweep_intensity.max(0.0),
            cancel_l1_intensity: cancel_l1_intensity.max(0.0),
            cancel_l2_intensity: cancel_l2_intensity.max(0.0),
            add_l1_intensity: add_l1_intensity.max(0.0),
            ..Default::default()
        }
    }

    /// Evaluates the queue drain hazard and whether resting maker quotes must be evacuated.
    pub fn evaluate_drain_hazard(
        &self,
        v_ahead: f64,
        v_total: f64,
        tau_cancel_sec: f64,
    ) -> QueueDrainVerdict {
        let v_ahead_safe = v_ahead.max(0.01);
        let v_total_safe = v_total.max(v_ahead_safe);
        let tau = tau_cancel_sec.max(0.005);

        // Queue depletion velocity: sweeps + cancellations proportionally ahead
        let sweep_vel = self.sweep_intensity * self.mean_sweep_size;
        let cancel_vel =
            self.cancel_l1_intensity * self.mean_cancel_size * (v_ahead_safe / v_total_safe);
        let mu_drain = (sweep_vel + cancel_vel).max(1e-4);
        let add_vel = (self.add_l1_intensity * self.mean_add_size).max(1e-4);
        let drain_add_ratio = (sweep_vel + cancel_vel) / add_vel;

        // Drain diffusion volatility
        let sigma_drain = (self.sweep_intensity + self.cancel_l1_intensity)
            .max(0.01)
            .sqrt();

        // First-passage pick-off probability via Inverse Gaussian / reflection principle
        let sigma_sqrt_tau = sigma_drain * tau.sqrt();
        let z1 = (v_ahead_safe - mu_drain * tau) / sigma_sqrt_tau;
        let z2 = (-v_ahead_safe - mu_drain * tau) / sigma_sqrt_tau;

        let phi1 = crate::black_scholes::standard_normal_cdf(z1);
        let phi2 = crate::black_scholes::standard_normal_cdf(z2);

        let exp_arg = (2.0 * mu_drain * v_ahead_safe) / (sigma_drain * sigma_drain);
        let term2 = if exp_arg > 50.0 {
            0.0
        } else {
            exp_arg.exp() * phi2
        };

        let survival_prob = (phi1 - term2).clamp(0.0, 1.0);
        let pickoff_prob = (1.0 - survival_prob).clamp(0.0, 1.0);

        let l2_cancellation_surging =
            self.cancel_l2_intensity > 5.0 && v_ahead_safe < 2.0 * self.mean_sweep_size;

        let mut should_evacuate = false;
        let mut reasons = Vec::new();

        if pickoff_prob >= self.critical_pickoff_threshold {
            should_evacuate = true;
            reasons.push(format!(
                "Pick-off probability {:.2} >= {:.2}",
                pickoff_prob, self.critical_pickoff_threshold
            ));
        }

        if drain_add_ratio >= self.critical_drain_add_ratio {
            should_evacuate = true;
            reasons.push(format!(
                "Drain/Add ratio {:.2} >= {:.2}",
                drain_add_ratio, self.critical_drain_add_ratio
            ));
        }

        if l2_cancellation_surging {
            should_evacuate = true;
            reasons.push(format!(
                "L2 cancel surge canary active (lambda_L2={:.2}, v_ahead={:.2})",
                self.cancel_l2_intensity, v_ahead_safe
            ));
        }

        let reason = if should_evacuate {
            reasons.join("; ")
        } else {
            "Normal queue liquidity".to_string()
        };

        QueueDrainVerdict {
            pickoff_probability: pickoff_prob,
            drain_velocity: mu_drain,
            replenishment_velocity: add_vel,
            drain_to_add_ratio: drain_add_ratio,
            is_l2_cancellation_surging: l2_cancellation_surging,
            should_evacuate_passive_quote: should_evacuate,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_engine() -> MultiAssetCrossHawkesEngine {
        let assets = vec!["BTC".to_string(), "ETH".to_string(), "SOL".to_string()];
        let venues = vec!["binance".to_string(), "hyperliquid".to_string()];
        let adv = vec![25_000_000_000.0, 12_000_000_000.0, 3_000_000_000.0];
        let correlations = vec![
            vec![1.00, 0.85, 0.75],
            vec![0.85, 1.00, 0.70],
            vec![0.75, 0.70, 1.00],
        ];
        let self_impact = vec![0.0001, 0.0002, 0.0005];

        let cfg = CrossHawkesConfig {
            num_assets: 3,
            num_venues: 2,
            baseline_intensity: 0.10,
            self_excitation_alpha: 0.20,
            cross_venue_excitation_alpha: 0.10,
            cross_asset_excitation_alpha: 0.05,
            decay_beta: 1.50,
            beta_impact_decay: 2.00,
            toxic_cojump_threshold: 5.00,
        };

        MultiAssetCrossHawkesEngine::new(assets, venues, adv, correlations, self_impact, cfg)
    }

    #[test]
    fn test_cross_hawkes_stream_encoding_decoding() {
        let num_venues = 2;
        // Asset 1 (ETH), Venue 0 (binance), Buy -> stream index 2*2 + 0 = 4
        let stream = MultiAssetCrossHawkesEngine::encode_stream(1, 0, true, num_venues);
        assert_eq!(stream, 4);

        let (a, v, is_buy) = MultiAssetCrossHawkesEngine::decode_stream(stream, num_venues);
        assert_eq!(a, 1);
        assert_eq!(v, 0);
        assert!(is_buy);

        // Asset 2 (SOL), Venue 1 (hyperliquid), Sell -> pair_idx = 2*2 + 1 = 5 -> stream = 5*2 + 1 = 11
        let stream_sell = MultiAssetCrossHawkesEngine::encode_stream(2, 1, false, num_venues);
        assert_eq!(stream_sell, 11);
        let (a2, v2, is_buy2) = MultiAssetCrossHawkesEngine::decode_stream(stream_sell, num_venues);
        assert_eq!(a2, 2);
        assert_eq!(v2, 1);
        assert!(!is_buy2);
    }

    #[test]
    fn test_cross_hawkes_spectral_radius_and_subcritical_regime() {
        let engine = make_test_engine();
        let rho = engine.spectral_radius();
        assert!(
            rho > 0.0 && rho < 0.70,
            "Initial regime should be Subcritical: rho={}",
            rho
        );
        assert_eq!(engine.classify_regime(), CrossHawkesRegime::Subcritical);
    }

    #[test]
    fn test_asymmetric_cross_impact() {
        let engine = make_test_engine();
        // BTC (0) on ETH (1)
        let impact_btc_on_eth = engine.asymmetric_cross_impact(0, 1, 0.0);
        // ETH (1) on BTC (0)
        let impact_eth_on_btc = engine.asymmetric_cross_impact(1, 0, 0.0);

        assert!(
            impact_btc_on_eth > impact_eth_on_btc,
            "BTC flow should exert higher asymmetric cross-impact on ETH than vice-versa: {} vs {}",
            impact_btc_on_eth,
            impact_eth_on_btc
        );
    }

    #[test]
    fn test_cojump_cascade_and_quote_withdrawal() {
        let mut engine = make_test_engine();
        let btc_binance_sell = MultiAssetCrossHawkesEngine::encode_stream(0, 0, false, 2);

        // Simulate massive burst of 25 aggressive sells on BTC Binance
        for i in 0..25 {
            engine.record_event(btc_binance_sell, 1.0 + i as f64 * 0.01);
        }

        let (withdraw, hazard) = engine.check_cojump_quote_withdrawal(0, 1, 1.30);
        assert!(
            hazard > 3.0,
            "Hazard should spike after BTC sell cascade: {}",
            hazard
        );
        assert!(
            withdraw,
            "Quote withdrawal should trigger after rapid cascade"
        );

        let drift_eth = engine.toxic_reservation_drift_bps(1, 1.30);
        assert!(
            drift_eth < -5.0,
            "ETH reservation price should shift down due to correlated BTC selling: {}",
            drift_eth
        );
    }

    #[test]
    fn test_multilevel_hawkes_queue_drain_forecaster() {
        let normal_forecaster = MultiLevelHawkesQueueDrainForecaster::default();
        let normal_verdict = normal_forecaster.evaluate_drain_hazard(10.0, 25.0, 0.05);
        assert!(
            !normal_verdict.should_evacuate_passive_quote,
            "Normal queue conditions should not trigger evacuation: {:?}",
            normal_verdict
        );

        // Heavy sweep storm: sweep intensity 40.0, thin queue ahead
        let toxic_sweeps = MultiLevelHawkesQueueDrainForecaster::new(40.0, 5.0, 1.0, 1.0);
        let toxic_verdict = toxic_sweeps.evaluate_drain_hazard(1.0, 10.0, 0.05);
        assert!(
            toxic_verdict.should_evacuate_passive_quote,
            "Toxic sweep storm should trigger evacuation: {:?}",
            toxic_verdict
        );
        assert!(
            toxic_verdict.drain_to_add_ratio >= 3.80 || toxic_verdict.pickoff_probability >= 0.68
        );

        // L2 cancellation canary surge
        let canary_surging = MultiLevelHawkesQueueDrainForecaster::new(1.0, 1.0, 12.0, 2.0);
        let canary_verdict = canary_surging.evaluate_drain_hazard(1.2, 5.0, 0.05);
        assert!(
            canary_verdict.should_evacuate_passive_quote,
            "L2 cancel surge canary should trigger evacuation: {:?}",
            canary_verdict
        );
        assert!(canary_verdict.is_l2_cancellation_surging);
    }
}
