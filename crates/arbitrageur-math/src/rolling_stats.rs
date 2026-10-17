use arbitrageur_core::types::{dec, Decimal};
use std::collections::VecDeque;

/// Numerically stable online running mean and variance using Welford's algorithm.
/// Time complexity per update: O(1). Space complexity: O(1).
#[derive(Debug, Clone)]
pub struct OnlineWelfordStats {
    count: usize,
    mean: f64,
    m2: f64,
}

impl OnlineWelfordStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    pub fn update(&mut self, x: f64) {
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn mean(&self) -> f64 {
        self.mean
    }

    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }

    pub fn std_dev(&self) -> f64 {
        self.variance().sqrt()
    }

    pub fn z_score(&self, x: f64) -> Option<f64> {
        let std = self.std_dev();
        if std > 1e-12 {
            Some((x - self.mean) / std)
        } else {
            None
        }
    }
}

impl Default for OnlineWelfordStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Rolling window statistical accumulator with fixed maximum sample capacity.
#[derive(Debug, Clone)]
pub struct RollingWindowStats {
    capacity: usize,
    window: VecDeque<Decimal>,
    sum: Decimal,
}

impl RollingWindowStats {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "Capacity must be positive");
        Self {
            capacity,
            window: VecDeque::with_capacity(capacity),
            sum: dec!(0),
        }
    }

    pub fn push(&mut self, val: Decimal) {
        if self.window.len() == self.capacity {
            if let Some(old) = self.window.pop_front() {
                self.sum -= old;
            }
        }
        self.window.push_back(val);
        self.sum += val;
    }

    pub fn count(&self) -> usize {
        self.window.len()
    }

    pub fn is_warmed_up(&self, min_samples: usize) -> bool {
        self.window.len() >= min_samples
    }

    pub fn mean(&self) -> Option<Decimal> {
        if self.window.is_empty() {
            None
        } else {
            Some(self.sum / Decimal::from(self.window.len()))
        }
    }

    pub fn variance(&self) -> Option<Decimal> {
        let n = self.window.len();
        if n < 2 {
            return None;
        }
        let avg = self.mean()?;
        let mut sum_sq = dec!(0);
        for &v in &self.window {
            let diff = v - avg;
            sum_sq += diff * diff;
        }
        Some(sum_sq / Decimal::from(n - 1))
    }

    pub fn std_dev(&self) -> Option<Decimal> {
        let var_f64 = self.variance()?.to_string().parse::<f64>().ok()?;
        let std_f64 = var_f64.sqrt();
        Decimal::from_str_exact(&format!("{:.8}", std_f64))
            .ok()
            .or_else(|| Decimal::from_f64_retain(std_f64))
    }

    pub fn z_score(&self, current: Decimal) -> Option<Decimal> {
        let mean = self.mean()?;
        let std = self.std_dev()?;
        if std > dec!(0) {
            Some((current - mean) / std)
        } else {
            None
        }
    }
}

/// Exponential Moving Average (EMA) and Exponential Moving Variance (EMV).
#[derive(Debug, Clone)]
pub struct ExponentialMovingStats {
    alpha: f64,
    ema: f64,
    emv: f64,
    initialized: bool,
}

impl ExponentialMovingStats {
    pub fn new(alpha: f64) -> Self {
        assert!(alpha > 0.0 && alpha <= 1.0, "alpha must be in (0, 1]");
        Self {
            alpha,
            ema: 0.0,
            emv: 0.0,
            initialized: false,
        }
    }

    pub fn from_half_life(half_life_samples: f64) -> Self {
        let alpha = 1.0 - (-std::f64::consts::LN_2 / half_life_samples).exp();
        Self::new(alpha)
    }

    pub fn update(&mut self, x: f64) {
        if !self.initialized {
            self.ema = x;
            self.emv = 0.0;
            self.initialized = true;
        } else {
            let delta = x - self.ema;
            self.ema += self.alpha * delta;
            self.emv = (1.0 - self.alpha) * (self.emv + self.alpha * delta * delta);
        }
    }

    pub fn mean(&self) -> f64 {
        self.ema
    }

    pub fn variance(&self) -> f64 {
        self.emv
    }

    pub fn std_dev(&self) -> f64 {
        self.emv.sqrt()
    }
}

/// Single price interval bar for range-based volatility estimators.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BarOhlc {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

impl BarOhlc {
    pub fn new(open: f64, high: f64, low: f64, close: f64) -> Self {
        Self {
            open,
            high,
            low,
            close,
        }
    }
}

/// High-efficiency, drift-independent Yang-Zhang (2000) Realized Volatility Estimator.
///
/// Combines overnight jump variance $\sigma_o^2$, open-to-close variance $\sigma_c^2$,
/// and Rogers-Satchell continuous variance $\sigma_{RS}^2$:
/// $$\sigma_{YZ}^2 = \sigma_o^2 + k \sigma_c^2 + (1 - k) \sigma_{RS}^2$$
/// with optimal weighting $k = \frac{0.34}{1.34 + \frac{N+1}{N-1}}$.
///
/// Achieves 14.0x higher efficiency than simple close-to-close variance estimators
/// while remaining invariant to continuous drift and robust to market opening jumps.
#[derive(Debug, Clone)]
pub struct YangZhangVolatilityEstimator {
    pub min_periods: usize,
}

impl Default for YangZhangVolatilityEstimator {
    fn default() -> Self {
        Self { min_periods: 5 }
    }
}

impl YangZhangVolatilityEstimator {
    pub fn new(min_periods: usize) -> Self {
        Self {
            min_periods: min_periods.max(4),
        }
    }

    /// Computes annualized Yang-Zhang variance and standard deviation.
    /// `periods_per_year`: annualization scaling factor (e.g. 365.0 * 24.0 for 1-hour crypto bars).
    pub fn calculate(&self, bars: &[BarOhlc], periods_per_year: f64) -> Option<(f64, f64)> {
        let n = bars.len();
        if n < self.min_periods || periods_per_year <= 0.0 {
            return None;
        }

        // Validate all prices are positive and finite
        if bars.iter().any(|b| {
            b.open <= 0.0
                || b.high <= 0.0
                || b.low <= 0.0
                || b.close <= 0.0
                || !b.open.is_finite()
                || !b.high.is_finite()
                || !b.low.is_finite()
                || !b.close.is_finite()
                || b.high < b.low
                || b.high < b.open
                || b.high < b.close
                || b.low > b.open
                || b.low > b.close
        }) {
            return None;
        }

        let n_f = n as f64;
        let mut sum_o = 0.0;
        let mut sum_c = 0.0;
        let mut o_vec = Vec::with_capacity(n - 1);
        let mut c_vec = Vec::with_capacity(n - 1);
        let mut rs_sum = 0.0;

        for i in 1..n {
            let prev_c = bars[i - 1].close;
            let o = (bars[i].open / prev_c).ln();
            let c = (bars[i].close / bars[i].open).ln();
            let u = (bars[i].high / bars[i].open).ln();
            let d = (bars[i].low / bars[i].open).ln();

            o_vec.push(o);
            c_vec.push(c);
            sum_o += o;
            sum_c += c;

            // Rogers-Satchell component: u*(u - c) + d*(d - c)
            rs_sum += u * (u - c) + d * (d - c);
        }

        let m = (n - 1) as f64;
        if m < 2.0 {
            return None;
        }

        let mean_o = sum_o / m;
        let mean_c = sum_c / m;

        let var_o = o_vec.iter().map(|&o| (o - mean_o).powi(2)).sum::<f64>() / (m - 1.0);
        let var_c = c_vec.iter().map(|&c| (c - mean_c).powi(2)).sum::<f64>() / (m - 1.0);
        let var_rs = rs_sum / m;

        // Optimal weighting parameter k
        let k = 0.34 / (1.34 + (n_f + 1.0) / (n_f - 1.0));
        let var_yz = (var_o + k * var_c + (1.0 - k) * var_rs).max(0.0);
        let annualized_var = var_yz * periods_per_year;
        let annualized_std = annualized_var.sqrt();

        Some((annualized_var, annualized_std))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yang_zhang_volatility() {
        let estimator = YangZhangVolatilityEstimator::new(5);
        let bars = vec![
            BarOhlc::new(100.0, 102.0, 99.0, 101.0),
            BarOhlc::new(101.5, 103.5, 100.5, 102.0),
            BarOhlc::new(101.8, 104.0, 101.0, 103.5),
            BarOhlc::new(103.0, 105.0, 102.5, 104.0),
            BarOhlc::new(104.2, 106.0, 103.8, 105.5),
            BarOhlc::new(105.0, 107.0, 104.5, 106.2),
        ];

        let res = estimator.calculate(&bars, 365.0 * 24.0);
        assert!(res.is_some());
        let (var, std) = res.unwrap();
        assert!(var > 0.0);
        assert!(std > 0.0);
        assert!((std * std - var).abs() < 1e-6);
    }
}
