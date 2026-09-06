use std::collections::VecDeque;
use arbitrager_core::types::{dec, Decimal};

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
