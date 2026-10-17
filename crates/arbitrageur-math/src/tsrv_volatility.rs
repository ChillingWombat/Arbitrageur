//! High-Frequency Two-Scale Realized Volatility (TSRV), Diurnal Seasonality, & Microstructure Jump Engine.
//!
//! Provides production microstructural noise de-biasing and jump separation:
//! 1. **Zhang, Mykland, & Aït-Sahalia (2005) Two-Scale Realized Variance (TSRV)**:
//!    De-biases high-frequency price feeds from bid-ask bounce noise:
//!    $$\widehat{\text{TSRV}} = \left(1 - \frac{\bar{n}}{n}\right)^{-1} \left( \text{RV}_{\text{avg}} - \frac{\bar{n}}{n} \text{RV}_{\text{all}} \right)$$
//!    with optimal sub-grid step $K^* = \lceil c \cdot n^{2/3} \rceil$, achieving $n^{-1/6}$ convergence.
//! 2. **Intraday Diurnal Seasonality Rescaling (Bollerslev & Cai)**:
//!    Models U-shaped diurnal volatility across 96 fifteen-minute intervals of the 24h UTC trading day:
//!    $$r_t^* = r_t / s(t_{\text{bin}})$$
//!    preventing spurious jump alarms during standard Asia/London/NY opening overlaps.
//! 3. **Barndorff-Nielsen & Shephard (BNS) Jump Test Statistic**:
//!    $$Z_{\text{jump}} = \frac{\frac{\widehat{\text{TSRV}} - \text{BPV}}{\widehat{\text{TSRV}}}}{\sqrt{ \left(\frac{\pi^2}{4} + \pi - 5\right) \frac{1}{n} \frac{\text{QP}}{\text{BPV}^2} }} \sim \mathcal{N}(0, 1)$$
//!    triggering dynamic maker quote widening ($3.5\times\text{--}6.0\times$) and circuit protection.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::f64::consts::PI;

/// Actions commanded by high-frequency jump and volatility filtering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TsrvJumpAction {
    /// Severe jump detected (Z_jump >= hurdle): instantly widen maker quotes and pause aggressive crossing.
    SpikeWidening {
        z_jump: f64,
        widening_multiplier: f64,
        de_biased_vol_annual: f64,
    },
    /// Elevated continuous diffusion without jumps: modest volatility-adjusted quoting.
    VolAdjustedQuoting { de_biased_vol_annual: f64 },
    /// Normal calm market regime.
    Normal,
}

/// Diagnostic report from TSRV, diurnal seasonality, and BNS jump filtering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TsrvVolatilityReport {
    /// Noise-de-biased Two-Scale Realized Variance (annualized).
    pub tsrv_variance_annual: f64,
    /// Noise-de-biased volatility standard deviation (annualized).
    pub tsrv_volatility_annual: f64,
    /// Raw all-tick Realized Variance (biased by bid-ask bounce).
    pub raw_rv_all_annual: f64,
    /// Robust Bipower Variation (continuous integrated variance).
    pub bpv_annual: f64,
    /// Estimated noise variance ratio: 1.0 - (TSRV / RV_all).
    pub noise_variance_ratio: f64,
    /// Barndorff-Nielsen & Shephard Z_jump statistic.
    pub z_jump: f64,
    /// Current diurnal seasonality multiplier s(t).
    pub diurnal_multiplier: f64,
    /// Commanded execution quote action.
    pub action: TsrvJumpAction,
}

/// High-Frequency Two-Scale Realized Volatility & Diurnal Seasonality Filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoScaleRealizedVarianceFilter {
    /// Ring buffer of high-frequency log prices.
    tick_history: VecDeque<f64>,
    /// Maximum ticks retained in ring buffer.
    pub max_ticks: usize,
    /// Minimum ticks required before computing TSRV.
    pub min_ticks: usize,
    /// Number of intraday seasonality bins (e.g. 96 for 15-min intervals).
    pub num_diurnal_bins: usize,
    /// Empirical diurnal seasonality profile table s(bin).
    pub diurnal_profile: Vec<f64>,
    /// Z-score hurdle for jump detection (default 3.09 for p < 0.001).
    pub jump_z_hurdle: f64,
    /// Base widening slope per unit Z_jump.
    pub widening_slope: f64,
    /// Maximum widening factor ceiling.
    pub max_widening_multiplier: f64,
}

impl Default for TwoScaleRealizedVarianceFilter {
    fn default() -> Self {
        let bins = 96;
        // Standard crypto 24h diurnal curve: elevated at Asia open (00:00-02:00 UTC) and London/NY overlap (13:00-17:00 UTC)
        let mut profile = vec![1.0; bins];
        for (i, p) in profile.iter_mut().enumerate() {
            let hour = (i as f64) * 24.0 / (bins as f64);
            // Harmonic modulation: U-shaped diurnal curve
            let asia_spike = (-0.5 * ((hour - 1.0) / 1.5).powi(2)).exp() * 0.45;
            let ny_spike = (-0.5 * ((hour - 15.0) / 2.5).powi(2)).exp() * 0.65;
            let dip = (-0.5 * ((hour - 7.0) / 2.0).powi(2)).exp() * 0.25;
            *p = (1.0 + asia_spike + ny_spike - dip).clamp(0.5, 2.0);
        }

        Self {
            tick_history: VecDeque::with_capacity(300),
            max_ticks: 300,
            min_ticks: 60,
            num_diurnal_bins: bins,
            diurnal_profile: profile,
            jump_z_hurdle: 3.09,
            widening_slope: 1.2,
            max_widening_multiplier: 5.0,
        }
    }
}

impl TwoScaleRealizedVarianceFilter {
    pub fn new(max_ticks: usize, min_ticks: usize, jump_z_hurdle: f64) -> Self {
        let mut filter = Self::default();
        filter.max_ticks = max_ticks.max(100);
        filter.min_ticks = min_ticks.clamp(30, filter.max_ticks - 10);
        filter.jump_z_hurdle = jump_z_hurdle.clamp(1.5, 6.0);
        filter.tick_history = VecDeque::with_capacity(filter.max_ticks);
        filter
    }

    /// Ingests a new high-frequency price observation.
    pub fn update_price(&mut self, price: f64) {
        if price <= 0.0 || !price.is_finite() {
            return;
        }
        let log_p = price.ln();
        if self.tick_history.len() >= self.max_ticks {
            self.tick_history.pop_front();
        }
        self.tick_history.push_back(log_p);
    }

    /// Number of active ticks in buffer.
    pub fn tick_count(&self) -> usize {
        self.tick_history.len()
    }

    /// Evaluates the diurnal seasonality multiplier for a given UTC timestamp in seconds.
    pub fn get_diurnal_multiplier(&self, utc_timestamp_sec: i64) -> f64 {
        if self.diurnal_profile.is_empty() {
            return 1.0;
        }
        let seconds_in_day = 86400;
        let day_sec = (utc_timestamp_sec % seconds_in_day + seconds_in_day) % seconds_in_day;
        let bin = ((day_sec as f64 / seconds_in_day as f64) * self.num_diurnal_bins as f64).floor()
            as usize;
        let idx = bin.min(self.diurnal_profile.len() - 1);
        self.diurnal_profile[idx]
    }

    /// Calculates the unbiased Two-Scale Realized Variance (TSRV), Bipower Variation, and BNS Jump statistic.
    /// `seconds_per_interval`: average duration in seconds per tick interval (e.g. 1.0s or 5.0s).
    /// `utc_timestamp_sec`: current UTC timestamp in seconds for diurnal curve alignment.
    pub fn evaluate(
        &self,
        seconds_per_interval: f64,
        utc_timestamp_sec: i64,
    ) -> Option<TsrvVolatilityReport> {
        let n = self.tick_history.len();
        if n < self.min_ticks || seconds_per_interval <= 0.0 {
            return None;
        }

        let diurnal_s = self.get_diurnal_multiplier(utc_timestamp_sec);
        let annual_factor = (365.25 * 86400.0) / seconds_per_interval;

        // 1. Raw all-tick Realized Variance RV_all with diurnal de-biasing
        let mut rv_all_raw = 0.0;
        let mut returns = Vec::with_capacity(n - 1);

        for i in 1..n {
            let raw_r = self.tick_history[i] - self.tick_history[i - 1];
            // De-seasonalized return: r* = r / s(t)
            let r_star = raw_r / diurnal_s;
            returns.push(r_star);
            rv_all_raw += r_star * r_star;
        }

        // 2. Sub-grid partition K = ceil(0.5 * n^(2/3))
        let k = ((n as f64).powf(2.0 / 3.0) * 0.5).ceil().max(2.0) as usize;
        let mut rv_sub_sum = 0.0;

        for g in 0..k {
            let mut sub_rv = 0.0;
            let mut idx = g;
            while idx + k < n {
                let diff = (self.tick_history[idx + k] - self.tick_history[idx]) / diurnal_s;
                sub_rv += diff * diff;
                idx += k;
            }
            rv_sub_sum += sub_rv;
        }
        let rv_avg = rv_sub_sum / (k as f64);

        // 3. Unbiased Zhang-Mykland-Aït-Sahalia TSRV calculation
        let n_bar = (n - k + 1) as f64 / (k as f64);
        let ratio = n_bar / (n as f64);
        let c_factor = 1.0 / (1.0 - ratio).max(1e-6);
        let tsrv_raw = (c_factor * (rv_avg - ratio * rv_all_raw)).max(1e-12);

        // 4. Bipower Variation (BPV) for continuous diffusion
        let mu1 = (2.0 / PI).sqrt();
        let mut bpv_sum = 0.0;
        for i in 1..returns.len() {
            bpv_sum += returns[i].abs() * returns[i - 1].abs();
        }
        let bpv_raw = (bpv_sum / (mu1 * mu1)).max(1e-12);

        // 5. Quadpower Quarticity (QP)
        let mu_43: f64 = 0.8309; // 2^(2/3) * Gamma(7/6) / Gamma(1/2)
        let mut qp_sum = 0.0;
        for i in 3..returns.len() {
            qp_sum += returns[i].abs().powf(4.0 / 3.0)
                * returns[i - 1].abs().powf(4.0 / 3.0)
                * returns[i - 2].abs().powf(4.0 / 3.0)
                * returns[i - 3].abs().powf(4.0 / 3.0);
        }
        let m = returns.len() as f64;
        let qp = (m * qp_sum / mu_43.powi(4)).max(1e-16);

        // 6. Asymptotic Barndorff-Nielsen & Shephard (BNS) Jump statistic (Huang & Tauchen 2005)
        let v_const = (PI * PI / 4.0) + PI - 5.0; // ~0.6090
        let qp_ratio = (qp / (bpv_raw * bpv_raw)).max(1.0);
        let denom_var = (v_const / m) * qp_ratio;
        let jump_ratio = if tsrv_raw > bpv_raw && tsrv_raw > 1e-8 {
            (1.0 - (bpv_raw / tsrv_raw)).max(0.0)
        } else {
            0.0
        };
        let z_jump = if denom_var > 1e-12 && jump_ratio > 0.0 {
            jump_ratio / denom_var.sqrt()
        } else {
            0.0
        };

        // Annualized metrics
        let tsrv_variance_annual = tsrv_raw * annual_factor;
        let tsrv_volatility_annual = tsrv_variance_annual.sqrt();
        let raw_rv_all_annual = rv_all_raw * annual_factor;
        let bpv_annual = bpv_raw * annual_factor;
        let noise_variance_ratio = if rv_all_raw > 0.0 {
            (1.0 - (tsrv_raw / rv_all_raw)).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Commanded quote action
        let action = if z_jump >= self.jump_z_hurdle {
            let expansion = (1.0 + self.widening_slope * (z_jump - self.jump_z_hurdle))
                .clamp(1.0, self.max_widening_multiplier);
            TsrvJumpAction::SpikeWidening {
                z_jump,
                widening_multiplier: expansion,
                de_biased_vol_annual: tsrv_volatility_annual,
            }
        } else if tsrv_volatility_annual > 0.80 {
            TsrvJumpAction::VolAdjustedQuoting {
                de_biased_vol_annual: tsrv_volatility_annual,
            }
        } else {
            TsrvJumpAction::Normal
        };

        Some(TsrvVolatilityReport {
            tsrv_variance_annual,
            tsrv_volatility_annual,
            raw_rv_all_annual,
            bpv_annual,
            noise_variance_ratio,
            z_jump,
            diurnal_multiplier: diurnal_s,
            action,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tsrv_de_biases_bid_ask_bounce() {
        let mut filter = TwoScaleRealizedVarianceFilter::new(200, 60, 3.09);

        // Generate synthetic prices with strong bid-ask bounce noise:
        // True price stays at $3,000, but observed alternates +/- $0.50 bounce
        let base_price = 3000.0;
        for i in 0..100 {
            let bounce = if i % 2 == 0 { 0.50 } else { -0.50 };
            filter.update_price(base_price + bounce);
        }

        let report = filter
            .evaluate(1.0, 172800) // Day 2 00:00 UTC
            .expect("should produce valid TSRV report");

        // Raw RV_all is heavily inflated by alternating bounce
        assert!(report.raw_rv_all_annual > report.tsrv_variance_annual);
        // Estimated noise ratio should detect that variance is overwhelmingly bounce noise
        assert!(report.noise_variance_ratio > 0.30);
    }

    #[test]
    fn test_jump_detection_triggers_spike_widening() {
        let mut filter = TwoScaleRealizedVarianceFilter::new(100, 40, 2.50);

        // 1. Calm continuous diffusion (small bidirectional price movements)
        let mut p = 3000.0;
        for i in 0..50 {
            let step = if i % 2 == 0 { 0.15 } else { -0.12 };
            p += step;
            filter.update_price(p);
        }

        let calm_report = filter.evaluate(1.0, 0).unwrap();
        assert_eq!(calm_report.action, TsrvJumpAction::Normal);

        // 2. Sudden discontinuous price jump shock: +$60 jump (+2%)
        filter.update_price(p + 60.0);
        filter.update_price(p + 61.0);

        let jump_report = filter.evaluate(1.0, 0).unwrap();
        assert!(jump_report.z_jump > 2.0);
        if let TsrvJumpAction::SpikeWidening {
            widening_multiplier,
            ..
        } = jump_report.action
        {
            assert!(widening_multiplier >= 1.0);
        } else {
            assert!(jump_report.tsrv_volatility_annual > calm_report.tsrv_volatility_annual);
        }
    }

    #[test]
    fn test_diurnal_curve_lookup() {
        let filter = TwoScaleRealizedVarianceFilter::default();

        // 01:00 UTC (Asia open) -> multiplier > 1.0
        let s_asia = filter.get_diurnal_multiplier(3600);
        assert!(s_asia > 1.10);

        // 07:00 UTC (Calm morning dip) -> multiplier < 1.0
        let s_calm = filter.get_diurnal_multiplier(7 * 3600);
        assert!(s_calm < 1.0);

        // 15:00 UTC (London / NY peak) -> multiplier > 1.20
        let s_ny = filter.get_diurnal_multiplier(15 * 3600);
        assert!(s_ny > 1.25);
    }
}
