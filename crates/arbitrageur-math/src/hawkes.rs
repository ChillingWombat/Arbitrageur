//! Multivariate Mutually Exciting Hawkes Process with Exponential Kernel.
//!
//! Models high-frequency trade/quote arrival clustering and cross-excitation
//! across multiple venues and sides (e.g. Venue 1 Buy, Venue 1 Sell, Venue 2 Buy, Venue 2 Sell).
//!
//! The spectral radius of the branching ratio matrix \rho(\Gamma) classifies market regimes:
//! - Subcritical (\rho < 0.65): Exogenous order flow, rapid mean reversion.
//! - Critical (0.65 <= \rho < 0.95): Endogenous clustering, spreading risk.
//! - Supercritical (\rho >= 0.95): Toxic blowout / liquidation cascade regime.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HawkesRegime {
    /// Independent, exogenous liquidity arrivals with rapid mean reversion.
    Subcritical,
    /// Clustered endogenous arrivals; elevated spread variance.
    Critical,
    /// Self-exciting cascading blowout; cross-venue adverse selection imminent.
    Supercritical,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateHawkes {
    pub num_streams: usize,
    /// Baseline exogenous arrival intensities \mu_i > 0
    pub mu: Vec<f64>,
    /// Excitation magnitudes \alpha_ij >= 0
    pub alpha: Vec<Vec<f64>>,
    /// Exponential decay parameters \beta_ij > 0
    pub beta: Vec<Vec<f64>>,
    /// Current excited component of intensities \lambda_{ij}^{exc}(t) >= 0
    pub excited: Vec<Vec<f64>>,
    /// Last update timestamp in fractional seconds
    pub last_timestamp_sec: Option<f64>,
}

impl MultivariateHawkes {
    pub fn new(num_streams: usize) -> Self {
        Self {
            num_streams,
            mu: vec![0.1; num_streams],
            alpha: vec![vec![0.1; num_streams]; num_streams],
            beta: vec![vec![1.0; num_streams]; num_streams],
            excited: vec![vec![0.0; num_streams]; num_streams],
            last_timestamp_sec: None,
        }
    }

    /// Computes the Branching Ratio Matrix \Gamma_ij = \alpha_ij / \beta_ij
    #[allow(clippy::needless_range_loop)]
    pub fn branching_ratio_matrix(&self) -> Vec<Vec<f64>> {
        let mut gamma = vec![vec![0.0; self.num_streams]; self.num_streams];
        for i in 0..self.num_streams {
            for j in 0..self.num_streams {
                let a = if i < self.alpha.len() && j < self.alpha[i].len() {
                    self.alpha[i][j].max(0.0)
                } else {
                    0.0
                };
                let b = if i < self.beta.len() && j < self.beta[i].len() {
                    self.beta[i][j].max(1e-9)
                } else {
                    1.0
                };
                gamma[i][j] = a / b;
            }
        }
        gamma
    }

    /// Computes spectral radius \rho(\Gamma) via power iteration with Rayleigh quotient.
    #[allow(clippy::needless_range_loop)]
    pub fn spectral_radius(&self) -> f64 {
        if self.num_streams == 0 {
            return 0.0;
        }
        if self.num_streams == 1 {
            return (self.alpha[0][0] / self.beta[0][0].max(1e-9)).max(0.0);
        }
        let gamma = self.branching_ratio_matrix();
        let n = self.num_streams;
        let mut b = vec![1.0 / (n as f64).sqrt(); n];
        for _ in 0..40 {
            let mut next_b = vec![0.0; n];
            for i in 0..n {
                for j in 0..n {
                    next_b[i] += gamma[i][j] * b[j];
                }
            }
            let norm = next_b.iter().map(|v| v * v).sum::<f64>().sqrt();
            if norm < 1e-12 {
                return 0.0;
            }
            for i in 0..n {
                b[i] = next_b[i] / norm;
            }
        }

        // Rayleigh quotient: (b^T Gamma b) / (b^T b)
        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..n {
            let mut gb_i = 0.0;
            for j in 0..n {
                gb_i += gamma[i][j] * b[j];
            }
            num += b[i] * gb_i;
            den += b[i] * b[i];
        }
        (num / den.max(1e-12)).max(0.0)
    }

    /// Returns the market regime based on the spectral radius.
    pub fn classify_regime(&self) -> HawkesRegime {
        let rho = self.spectral_radius();
        if rho < 0.65 {
            HawkesRegime::Subcritical
        } else if rho < 0.95 {
            HawkesRegime::Critical
        } else {
            HawkesRegime::Supercritical
        }
    }

    /// Decays the excited intensities over elapsed time dt > 0:
    /// \lambda_{ij}^{exc}(t + dt) = \lambda_{ij}^{exc}(t) * exp(-\beta_{ij} * dt)
    pub fn decay(&mut self, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        for i in 0..self.num_streams {
            for j in 0..self.num_streams {
                let b = if i < self.beta.len() && j < self.beta[i].len() {
                    self.beta[i][j].max(1e-9)
                } else {
                    1.0
                };
                if i < self.excited.len() && j < self.excited[i].len() {
                    self.excited[i][j] *= (-b * dt).exp();
                }
            }
        }
    }

    /// Records an event arrival on `source_stream`, exciting all target streams i:
    /// \lambda_{ij}^{exc}(t^+) = \lambda_{ij}^{exc}(t) + \alpha_{ij}
    pub fn record_event(&mut self, source_stream: usize) {
        if source_stream >= self.num_streams {
            return;
        }
        for i in 0..self.num_streams {
            let a = if i < self.alpha.len() && source_stream < self.alpha[i].len() {
                self.alpha[i][source_stream].max(0.0)
            } else {
                0.0
            };
            if i < self.excited.len() && source_stream < self.excited[i].len() {
                self.excited[i][source_stream] += a;
            }
        }
    }

    /// Advances time to `timestamp_sec`, decays intensities, and optionally records an arrival event.
    pub fn update_at_timestamp(&mut self, timestamp_sec: f64, event_stream: Option<usize>) {
        if let Some(prev) = self.last_timestamp_sec {
            let dt = (timestamp_sec - prev).max(0.0);
            self.decay(dt);
        }
        self.last_timestamp_sec = Some(timestamp_sec);
        if let Some(stream) = event_stream {
            self.record_event(stream);
        }
    }

    /// Returns the current total intensity of stream i:
    /// \lambda_i(t) = \mu_i + \sum_j \lambda_{ij}^{exc}(t)
    pub fn current_intensity(&self, stream: usize) -> f64 {
        if stream >= self.num_streams {
            return 0.0;
        }
        let base = if stream < self.mu.len() {
            self.mu[stream]
        } else {
            0.1
        };
        let exc: f64 = if stream < self.excited.len() {
            self.excited[stream].iter().copied().sum()
        } else {
            0.0
        };
        (base + exc).max(0.0)
    }

    /// Returns the current total intensities across all streams.
    pub fn all_intensities(&self) -> Vec<f64> {
        (0..self.num_streams)
            .map(|i| self.current_intensity(i))
            .collect()
    }

    /// Computes directional order flow imbalance between buy stream and sell stream:
    /// I_{flow} = (\lambda_{buy} - \lambda_{sell}) / (\lambda_{buy} + \lambda_{sell} + \epsilon) \in [-1.0, 1.0]
    pub fn flow_imbalance(&self, buy_stream: usize, sell_stream: usize) -> f64 {
        let lam_buy = self.current_intensity(buy_stream);
        let lam_sell = self.current_intensity(sell_stream);
        let sum = lam_buy + lam_sell;
        if sum < 1e-9 {
            0.0
        } else {
            (lam_buy - lam_sell) / sum
        }
    }

    /// Computes the toxic reservation price adjustment (in basis points) under
    /// adverse selection flow:
    /// \Delta r_{bps} = I_{flow} * \gamma_{adverse} * \sigma * \sqrt{\Delta t}
    ///
    /// When buy arrivals dominate (cascade buying), \Delta r > 0 (fair price shifts up).
    /// When sell arrivals dominate (liquidation dumping), \Delta r < 0 (fair price shifts down).
    pub fn compute_toxic_reservation_offset_bps(
        &self,
        buy_stream: usize,
        sell_stream: usize,
        annualized_vol: f64,
        lead_horizon_sec: f64,
        gamma_adverse: f64,
    ) -> f64 {
        let imb = self.flow_imbalance(buy_stream, sell_stream);
        let horizon_years = (lead_horizon_sec.max(0.001) / (365.25 * 86400.0)).max(1e-12);
        let vol_t = annualized_vol.max(0.01) * horizon_years.sqrt();
        // Convert to basis points: 10,000 bps = 100%
        imb * gamma_adverse.max(0.1) * vol_t * 10000.0
    }

    /// Returns the instantaneous market regime taking into account both
    /// the static spectral radius \rho(\Gamma) and dynamic intensity surges.
    pub fn classify_instantaneous_regime(&self) -> HawkesRegime {
        let rho = self.spectral_radius();
        if rho >= 0.95 {
            return HawkesRegime::Supercritical;
        }

        // Check for instantaneous surge: if any stream's excited intensity is >= 3x baseline
        let has_critical_surge = (0..self.num_streams).any(|i| {
            let base = if i < self.mu.len() { self.mu[i] } else { 0.1 };
            self.current_intensity(i) >= 3.0 * base.max(1e-6)
        });

        let has_supercritical_surge = (0..self.num_streams).any(|i| {
            let base = if i < self.mu.len() { self.mu[i] } else { 0.1 };
            self.current_intensity(i) >= 6.0 * base.max(1e-6)
        });

        if has_supercritical_surge && rho >= 0.60 {
            HawkesRegime::Supercritical
        } else if rho >= 0.65 || has_critical_surge {
            HawkesRegime::Critical
        } else {
            HawkesRegime::Subcritical
        }
    }

    /// Computes the optimal dynamic maker peg depth (in basis points) under
    /// self-exciting Hawkes intensity:
    /// \delta^*(t) = \delta_{\text{base}} + \frac{1}{\kappa} \ln\left(\frac{\lambda_i(t)}{\mu_i}\right)
    pub fn optimal_maker_peg_offset_bps(
        &self,
        stream: usize,
        base_depth_bps: f64,
        kappa_elasticity: f64,
    ) -> f64 {
        if stream >= self.num_streams {
            return base_depth_bps;
        }
        let mu = if stream < self.mu.len() {
            self.mu[stream].max(1e-6)
        } else {
            0.1
        };
        let lambda = self.current_intensity(stream);
        let ratio = (lambda / mu).max(1.0);
        let kappa = kappa_elasticity.max(0.1);
        base_depth_bps + (1.0 / kappa) * ratio.ln() * 100.0
    }

    /// Evaluates whether a stream is experiencing an extreme toxic flow burst
    /// commanding immediate cancellation/pulling of resting maker quotes.
    /// Default threshold ratio is 15.0 (\lambda(t) >= 15 * \mu).
    pub fn is_toxic_pull(&self, stream: usize, ratio_threshold: f64) -> bool {
        if stream >= self.num_streams {
            return false;
        }
        let mu = if stream < self.mu.len() {
            self.mu[stream].max(1e-6)
        } else {
            0.1
        };
        let lambda = self.current_intensity(stream);
        let thresh = ratio_threshold.max(3.0);
        (lambda / mu) >= thresh
    }

    /// Computes cumulative queuing hazard H(\tau) over queue delay \tau:
    /// H(\tau) = \mu \tau + \frac{\lambda(t) - \mu}{\beta - \alpha} (1 - e^{-(\beta - \alpha)\tau})
    pub fn cumulative_hazard(&self, stream: usize, tau_sec: f64) -> f64 {
        if stream >= self.num_streams {
            return 0.0;
        }
        let mu = if stream < self.mu.len() {
            self.mu[stream]
        } else {
            0.1
        };
        let alpha = if stream < self.alpha.len() && stream < self.alpha[stream].len() {
            self.alpha[stream][stream]
        } else {
            0.1
        };
        let beta = if stream < self.beta.len() && stream < self.beta[stream].len() {
            self.beta[stream][stream].max(1e-6)
        } else {
            1.0
        };
        let lambda = self.current_intensity(stream);
        let net_decay = (beta - alpha).max(1e-4);
        let tau = tau_sec.max(0.0);
        mu * tau + ((lambda - mu) / net_decay) * (1.0 - (-net_decay * tau).exp())
    }

    /// Computes adverse selection fill probability for resting limit depth d:
    /// P_{adverse}(\tau | d) = p_{toxic} * (1 - e^{-H(\tau) / d})
    pub fn adverse_selection_probability(
        &self,
        stream: usize,
        tau_sec: f64,
        queue_depth: f64,
        p_toxic_base: f64,
    ) -> f64 {
        let h = self.cumulative_hazard(stream, tau_sec);
        let d = queue_depth.max(1.0);
        p_toxic_base.clamp(0.0, 1.0) * (1.0 - (-h / d).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hawkes_subcritical_regime() {
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.alpha = vec![vec![0.2, 0.1], vec![0.1, 0.2]];
        hawkes.beta = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let rho = hawkes.spectral_radius();
        assert!(rho < 0.65, "Expected subcritical, got rho={rho}");
        assert_eq!(hawkes.classify_regime(), HawkesRegime::Subcritical);
    }

    #[test]
    fn test_hawkes_supercritical_regime() {
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.alpha = vec![vec![0.8, 0.5], vec![0.5, 0.8]];
        hawkes.beta = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let rho = hawkes.spectral_radius();
        assert!(rho >= 0.95, "Expected supercritical, got rho={rho}");
        assert_eq!(hawkes.classify_regime(), HawkesRegime::Supercritical);
    }

    #[test]
    fn test_hawkes_online_decay_and_excitation() {
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.mu = vec![0.2, 0.2];
        hawkes.alpha = vec![vec![0.5, 0.1], vec![0.1, 0.5]];
        hawkes.beta = vec![vec![2.0, 1.0], vec![1.0, 2.0]];

        // Baseline intensity
        assert!((hawkes.current_intensity(0) - 0.2).abs() < 1e-6);
        assert!((hawkes.current_intensity(1) - 0.2).abs() < 1e-6);

        // Record event on stream 0 at t = 100.0
        hawkes.update_at_timestamp(100.0, Some(0));
        // Stream 0 excites: 0.2 + 0.5 = 0.7; Stream 1 excites: 0.2 + 0.1 = 0.3
        assert!((hawkes.current_intensity(0) - 0.7).abs() < 1e-6);
        assert!((hawkes.current_intensity(1) - 0.3).abs() < 1e-6);

        // Advance 1 second without events: dt = 1.0
        hawkes.update_at_timestamp(101.0, None);
        // Stream 0 excited decays by exp(-2.0 * 1.0) ~ 0.135335 -> 0.5 * 0.135335 = 0.067668 -> total ~ 0.267668
        let int0 = hawkes.current_intensity(0);
        assert!(int0 > 0.2 && int0 < 0.7);
        assert!((int0 - (0.2 + 0.5 * (-2.0_f64).exp())).abs() < 1e-5);
    }

    #[test]
    fn test_hawkes_flow_imbalance_and_toxic_reservation() {
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.mu = vec![0.1, 0.1];
        hawkes.alpha = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        hawkes.beta = vec![vec![1.0, 1.0], vec![1.0, 1.0]];

        // Stream 0 = Buy, Stream 1 = Sell
        // Simulate aggressive buy cascade: 5 buy events
        for i in 0..5 {
            hawkes.update_at_timestamp(10.0 + i as f64 * 0.01, Some(0));
        }

        let imb = hawkes.flow_imbalance(0, 1);
        assert!(
            imb > 0.8,
            "Expected strong positive buy imbalance, got {imb}"
        );

        // Toxic reservation offset should be positive (fair price drifted upwards)
        let offset_bps = hawkes.compute_toxic_reservation_offset_bps(0, 1, 0.60, 2.0, 1.5);
        assert!(
            offset_bps > 0.0,
            "Expected positive bps offset, got {offset_bps}"
        );

        // Instantaneous regime should detect the surge
        assert_ne!(
            hawkes.classify_instantaneous_regime(),
            HawkesRegime::Subcritical
        );
    }

    #[test]
    fn test_hawkes_maker_pegging_and_toxic_pull() {
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.mu = vec![0.1, 0.1];
        hawkes.alpha = vec![vec![1.5, 0.0], vec![0.0, 1.5]];
        hawkes.beta = vec![vec![1.0, 1.0], vec![1.0, 1.0]];

        // In calm conditions, baseline peg offset equals base depth
        let calm_peg = hawkes.optimal_maker_peg_offset_bps(0, 5.0, 1.5);
        assert!((calm_peg - 5.0).abs() < 1e-4);
        assert!(!hawkes.is_toxic_pull(0, 15.0));

        // Burst of arrivals on stream 0
        for i in 0..12 {
            hawkes.update_at_timestamp(1.0 + i as f64 * 0.01, Some(0));
        }

        // Peg offset widens logarithmically
        let excited_peg = hawkes.optimal_maker_peg_offset_bps(0, 5.0, 1.5);
        assert!(
            excited_peg > calm_peg,
            "Expected peg depth to widen during arrival burst, got {excited_peg} vs {calm_peg}"
        );

        // Toxic pull triggers when intensity ratio exceeds threshold
        assert!(hawkes.is_toxic_pull(0, 15.0));

        // Queuing hazard and adverse selection probability
        let hazard = hawkes.cumulative_hazard(0, 0.5);
        assert!(hazard > 0.0);
        let p_adverse = hawkes.adverse_selection_probability(0, 0.5, 10.0, 0.8);
        assert!(p_adverse > 0.0 && p_adverse <= 0.8);
    }
}
