//! Asynchronous High-Frequency Lead-Lag Estimation and Multi-Level Order Flow Imbalance.
//!
//! Implements:
//! 1. **Hayashi-Yoshida (2005) Covariance Estimator**: Non-synchronous cross-correlation
//!    between two independent venue quote streams without artificial grid resampling
//!    or forward-fill bias, overcoming the high-frequency Epps effect.
//! 2. **Lead-Lag Ratio (LLR)**: Continuous hypothesis test identifying which venue leads
//!    price discovery and the latency offset $\tau^*$.
//! 3. **Multi-Level Order Flow Imbalance (MLOFI)**: Depth-weighted order flow pressure
//!    with exponential decay $w_k = \exp(-\kappa(k-1))$.
//! 4. **Adverse Selection / Toxicity Gate**: Identifies whether cross-venue spread
//!    dislocations are toxic lead-lag sweeps vs benign mean-reverting dislocations.

use arbitrageur_core::types::Side;
use serde::{Deserialize, Serialize};

/// High-frequency price and volume observation from an exchange feed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QuoteTick {
    /// Timestamp in milliseconds (exchange matching engine or local receive).
    pub timestamp_ms: i64,
    /// Mid or top-of-book execution price.
    pub price: f64,
    /// Optional volume associated with tick/trade.
    pub volume: f64,
    /// True if taker buy / bid lift, false if taker sell / bid hit.
    pub is_buy: bool,
}

impl QuoteTick {
    pub fn new(timestamp_ms: i64, price: f64) -> Self {
        Self {
            timestamp_ms,
            price,
            volume: 1.0,
            is_buy: true,
        }
    }

    pub fn with_volume(timestamp_ms: i64, price: f64, volume: f64, is_buy: bool) -> Self {
        Self {
            timestamp_ms,
            price,
            volume,
            is_buy,
        }
    }
}

/// Directional leader classification between Venue A and Venue B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeadVenue {
    /// Venue A leads price discovery; Venue B follows with latency $\tau^* > 0$.
    VenueALeads,
    /// Venue B leads price discovery; Venue A follows with latency $\tau^* < 0$.
    VenueBLeads,
    /// Contemporaneous price discovery / balanced co-movement ($0.67 \le LLR \le 1.50$).
    Synchronous,
}

/// Result of continuous Hayashi-Yoshida cross-correlation and lead-lag analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeadLagResult {
    /// Lead-Lag Ratio $LLR_{A \to B} = \int_{0}^{\tau_{\max}} \rho^2(\tau) d\tau / \int_{-\tau_{\max}}^{0} \rho^2(\tau) d\tau$.
    pub lead_lag_ratio: f64,
    /// Optimal lead latency offset in milliseconds $\tau^* = \arg\max_\tau \rho_{AB}(\tau)$.
    pub optimal_lag_ms: i64,
    /// Peak cross-correlation coefficient at optimal lag $\rho_{AB}(\tau^*)$.
    pub peak_correlation: f64,
    /// Classified price discovery leader.
    pub leader: LeadVenue,
    /// Contemporaneous zero-lag correlation $\rho_{AB}(0)$.
    pub zero_lag_correlation: f64,
}

/// Non-synchronous Hayashi-Yoshida (2005) cross-correlation and lead-lag estimator.
#[derive(Debug, Clone)]
pub struct HayashiYoshidaEstimator {
    /// Maximum search lag in milliseconds (e.g. 500ms or 1000ms).
    pub max_lag_ms: i64,
    /// Step size in milliseconds for scanning lag intervals (e.g. 5ms or 10ms).
    pub lag_step_ms: i64,
}

impl HayashiYoshidaEstimator {
    pub fn new(max_lag_ms: i64, lag_step_ms: i64) -> Self {
        Self {
            max_lag_ms: max_lag_ms.max(10),
            lag_step_ms: lag_step_ms.max(1),
        }
    }

    /// Evaluates continuous Hayashi-Yoshida cross-correlation $\rho_{AB}(\tau)$ at lag `tau_ms`.
    ///
    /// Interval overlap condition:
    /// $I_i^A(\tau) = (t_{i-1}^A + \tau, t_i^A + \tau]$ intersects $I_j^B = (t_{j-1}^B, t_j^B]$
    /// if and only if $\max(t_{i-1}^A + \tau, t_{j-1}^B) < \min(t_i^A + \tau, t_j^B)$.
    pub fn cross_correlation_at_lag(
        &self,
        ticks_a: &[QuoteTick],
        ticks_b: &[QuoteTick],
        tau_ms: i64,
    ) -> f64 {
        if ticks_a.len() < 2 || ticks_b.len() < 2 {
            return 0.0;
        }

        let mut cov_ab = 0.0;
        let mut var_a = 0.0;
        let mut var_b = 0.0;

        for i in 1..ticks_a.len() {
            let dp_a = ticks_a[i].price - ticks_a[i - 1].price;
            var_a += dp_a * dp_a;
            let t_a_start = ticks_a[i - 1].timestamp_ms + tau_ms;
            let t_a_end = ticks_a[i].timestamp_ms + tau_ms;

            if t_a_end <= t_a_start {
                continue;
            }

            for j in 1..ticks_b.len() {
                let t_b_start = ticks_b[j - 1].timestamp_ms;
                let t_b_end = ticks_b[j].timestamp_ms;

                if t_b_end <= t_b_start {
                    continue;
                }

                // Check continuous interval intersection
                if t_a_start.max(t_b_start) < t_a_end.min(t_b_end) {
                    let dp_b = ticks_b[j].price - ticks_b[j - 1].price;
                    cov_ab += dp_a * dp_b;
                }
            }
        }

        for j in 1..ticks_b.len() {
            let dp_b = ticks_b[j].price - ticks_b[j - 1].price;
            var_b += dp_b * dp_b;
        }

        let denom = (var_a * var_b).sqrt();
        if denom > 1e-12 {
            (cov_ab / denom).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    /// Evaluates lead-lag relationship across the entire lag spectrum $[-\tau_{\max}, +\tau_{\max}]$.
    pub fn evaluate_lead_lag(&self, ticks_a: &[QuoteTick], ticks_b: &[QuoteTick]) -> LeadLagResult {
        if ticks_a.len() < 2 || ticks_b.len() < 2 {
            return LeadLagResult {
                lead_lag_ratio: 1.0,
                optimal_lag_ms: 0,
                peak_correlation: 0.0,
                leader: LeadVenue::Synchronous,
                zero_lag_correlation: 0.0,
            };
        }

        let mut pos_area = 0.0;
        let mut neg_area = 0.0;
        let mut best_rho = -2.0;
        let mut best_lag = 0;
        let zero_lag_rho = self.cross_correlation_at_lag(ticks_a, ticks_b, 0);

        let mut lag = -self.max_lag_ms;
        while lag <= self.max_lag_ms {
            let rho = self.cross_correlation_at_lag(ticks_a, ticks_b, lag);
            let rho_sq = rho * rho;

            if lag > 0 {
                pos_area += rho_sq;
            } else if lag < 0 {
                neg_area += rho_sq;
            }

            if rho > best_rho {
                best_rho = rho;
                best_lag = lag;
            }

            lag += self.lag_step_ms;
        }

        let llr = if neg_area > 1e-12 {
            pos_area / neg_area
        } else if pos_area > 1e-12 {
            5.0 // Cap extreme ratios
        } else {
            1.0
        };

        let leader = if best_lag >= self.lag_step_ms
            && (llr > 1.05 || best_rho > zero_lag_rho + 0.02)
        {
            LeadVenue::VenueALeads
        } else if best_lag <= -self.lag_step_ms && (llr < 0.95 || best_rho > zero_lag_rho + 0.02) {
            LeadVenue::VenueBLeads
        } else {
            LeadVenue::Synchronous
        };

        LeadLagResult {
            lead_lag_ratio: llr,
            optimal_lag_ms: best_lag,
            peak_correlation: best_rho.clamp(-1.0, 1.0),
            leader,
            zero_lag_correlation: zero_lag_rho,
        }
    }
}

/// Toxicity assessment for an observed cross-venue price spread.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToxicityAssessment {
    /// Safe mean-reverting dislocation: order flow confirms stability or mean reversion.
    BenignMeanReverting,
    /// Adverse selection danger: leading venue has massive directional OFI sweeping quotes,
    /// predicting that the lagging venue's quote will be wiped out before execution.
    ToxicAdverseSelection {
        expected_drift_bps: f64,
        hurdle_expansion_bps: f64,
    },
    /// Stale quote warning: lack of recent orderbook updates indicates a phantom quote.
    StalePhantomSpread,
}

/// Multi-Level Order Flow Imbalance and Spread Toxicity Evaluator.
#[derive(Debug, Clone)]
pub struct SpreadToxicityEvaluator {
    /// Depth decay rate $\kappa$ for multi-level weights $w_k = \exp(-\kappa (k - 1))$.
    pub depth_decay_kappa: f64,
    /// Maximum acceptable quote age in milliseconds before flagging stale.
    pub max_quote_age_ms: i64,
    /// OFI threshold in standard deviations to flag an aggressive sweep.
    pub ofi_z_threshold: f64,
}

impl Default for SpreadToxicityEvaluator {
    fn default() -> Self {
        Self {
            depth_decay_kappa: 0.40,
            max_quote_age_ms: 350,
            ofi_z_threshold: 2.0,
        }
    }
}

impl SpreadToxicityEvaluator {
    pub fn new(depth_decay_kappa: f64, max_quote_age_ms: i64, ofi_z_threshold: f64) -> Self {
        Self {
            depth_decay_kappa: depth_decay_kappa.max(0.01),
            max_quote_age_ms: max_quote_age_ms.max(50),
            ofi_z_threshold: ofi_z_threshold.max(0.5),
        }
    }

    /// Evaluates multi-level OFI across $K$ depth levels.
    ///
    /// Each level's delta:
    /// $\Delta N^B = Q_k^B(t) - Q_k^B(t-1)$
    /// $\Delta N^A = Q_k^A(t) - Q_k^A(t-1)$
    /// $OFI = \sum_k e^{-\kappa(k-1)} (\Delta N^B - \Delta N^A)$.
    pub fn compute_multi_level_ofi(&self, bid_deltas: &[f64], ask_deltas: &[f64]) -> f64 {
        let n = bid_deltas.len().min(ask_deltas.len());
        let mut ofi = 0.0;
        for k in 0..n {
            let weight = (-self.depth_decay_kappa * k as f64).exp();
            let level_ofi = bid_deltas[k] - ask_deltas[k];
            ofi += weight * level_ofi;
        }
        ofi
    }

    /// Evaluates whether an observed cross-venue spread is benign or toxic adverse selection.
    ///
    /// * `quote_age_a_ms`, `quote_age_b_ms`: Age of quotes from respective venue matching engines.
    /// * `lead_lag`: Hayashi-Yoshida lead-lag analysis result.
    /// * `lead_venue_ofi_z`: Normalized Z-score of multi-level OFI on the leading venue.
    /// * `spread_bps`: Gross cross-venue spread (Bid_B - Ask_A).
    pub fn evaluate_toxicity(
        &self,
        quote_age_a_ms: i64,
        quote_age_b_ms: i64,
        lead_lag: &LeadLagResult,
        lead_venue_ofi_z: f64,
        spread_bps: f64,
    ) -> ToxicityAssessment {
        // Gate 1: Check feed freshness
        if quote_age_a_ms > self.max_quote_age_ms || quote_age_b_ms > self.max_quote_age_ms {
            return ToxicityAssessment::StalePhantomSpread;
        }

        // Gate 2: If there is a clear leader, check if OFI points against the spread
        // If Venue A leads, and Venue A OFI is strongly negative (aggressive selling),
        // then Venue B's bid is about to collapse! Selling on B and buying on A is toxic.
        if lead_lag.leader == LeadVenue::VenueALeads && lead_venue_ofi_z < -self.ofi_z_threshold {
            // Venue A is crashing down, Venue B will follow in tau* ms
            let expected_drift = lead_venue_ofi_z.abs() * 3.5;
            let hurdle_expansion = expected_drift * 1.5;
            return ToxicityAssessment::ToxicAdverseSelection {
                expected_drift_bps: expected_drift,
                hurdle_expansion_bps: hurdle_expansion,
            };
        }

        if lead_lag.leader == LeadVenue::VenueBLeads && lead_venue_ofi_z > self.ofi_z_threshold {
            // Venue B is rocketing up, Venue A ask is about to get lifted
            let expected_drift = lead_venue_ofi_z.abs() * 3.5;
            let hurdle_expansion = expected_drift * 1.5;
            return ToxicityAssessment::ToxicAdverseSelection {
                expected_drift_bps: expected_drift,
                hurdle_expansion_bps: hurdle_expansion,
            };
        }

        // Gate 3: Spread within reasonable bounds
        if spread_bps <= 0.0 {
            return ToxicityAssessment::StalePhantomSpread;
        }

        ToxicityAssessment::BenignMeanReverting
    }
}

/// Online tracker for multi-level order flow imbalance (OFI), maintaining running
/// Welford statistics to calculate normalized Z-scores: Z_OFI = (OFI - mean) / std.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiLevelOfiTracker {
    pub count: u64,
    pub mean: f64,
    pub m2: f64,
    pub decay_factor: f64,
}

impl Default for MultiLevelOfiTracker {
    fn default() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            decay_factor: 0.95,
        }
    }
}

impl MultiLevelOfiTracker {
    pub fn new(decay_factor: f64) -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            decay_factor: decay_factor.clamp(0.50, 0.999),
        }
    }

    /// Ingests a new raw multi-level OFI sample and returns the current normalized Z-score.
    pub fn ingest_ofi(&mut self, ofi: f64) -> f64 {
        self.count += 1;
        if self.count == 1 {
            self.mean = ofi;
            self.m2 = 1.0;
            return 0.0;
        }

        // Exponentially decaying online variance
        let delta = ofi - self.mean;
        self.mean += (1.0 - self.decay_factor) * delta;
        let delta2 = ofi - self.mean;
        self.m2 = self.decay_factor * self.m2 + (1.0 - self.decay_factor) * delta * delta2;

        let std = self.m2.max(1e-6).sqrt();
        (ofi - self.mean) / std
    }

    /// Returns current standard deviation of OFI.
    pub fn std_dev(&self) -> f64 {
        self.m2.max(1e-6).sqrt()
    }
}

/// Preemption decision returned by OfiLeadLagPreemptor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OfiPreemptionDecision {
    HoldNoSignal,
    SnipeFollower {
        target_side: Side,
        expected_drift_bps: f64,
        total_friction_bps: f64,
    },
}

/// Anticipatory routing engine that exploits cross-venue OFI lead-lag dynamics
/// between high-throughput CEX leaders (e.g. Binance/OKX) and follower venues (Hyperliquid/Lighter).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OfiLeadLagPreemptor {
    pub beta_price_impact: f64,
    pub lead_latency_tau_ms: f64,
    pub ofi_z_threshold: f64,
}

impl Default for OfiLeadLagPreemptor {
    fn default() -> Self {
        Self {
            beta_price_impact: 2.5,
            lead_latency_tau_ms: 35.0,
            ofi_z_threshold: 2.0,
        }
    }
}

impl OfiLeadLagPreemptor {
    pub fn new(beta_impact: f64, lead_latency_tau_ms: f64, ofi_z_threshold: f64) -> Self {
        Self {
            beta_price_impact: beta_impact.max(0.1),
            lead_latency_tau_ms: lead_latency_tau_ms.max(1.0),
            ofi_z_threshold: ofi_z_threshold.max(0.5),
        }
    }

    /// Evaluates if leader OFI warrants anticipatory taker sweep on follower venue:
    /// Expected price drift = beta * |Z_OFI| * (1 - exp(-tau / 50ms))
    /// Dispatch iff Expected price drift >= follower total friction (half-spread + taker fee + slippage).
    pub fn evaluate_preemption(
        &self,
        z_ofi: f64,
        follower_half_spread_bps: f64,
        follower_taker_fee_bps: f64,
        follower_slippage_bps: f64,
    ) -> OfiPreemptionDecision {
        if z_ofi.abs() < self.ofi_z_threshold {
            return OfiPreemptionDecision::HoldNoSignal;
        }

        let total_friction =
            follower_half_spread_bps + follower_taker_fee_bps + follower_slippage_bps;
        let latency_factor = 1.0 - (-self.lead_latency_tau_ms / 50.0).exp();
        let expected_drift = self.beta_price_impact * z_ofi.abs() * latency_factor;

        if expected_drift >= total_friction {
            let target_side = if z_ofi > 0.0 { Side::Buy } else { Side::Sell };
            OfiPreemptionDecision::SnipeFollower {
                target_side,
                expected_drift_bps: expected_drift,
                total_friction_bps: total_friction,
            }
        } else {
            OfiPreemptionDecision::HoldNoSignal
        }
    }
}

/// Cont & de Larrard (2013) Markovian LOB first-passage price move predictor.
///
/// Under the diffusion approximation of limit order book queue dynamics,
/// predicts the probability that the next price change is upward:
/// $P(\tau_{\text{ask}} < \tau_{\text{bid}})$.
///
/// When order flow arrivals between bid and ask are uncorrelated ($\rho \approx 0$):
/// $$p_{\text{up}}(q_b, q_a) = \frac{2}{\pi} \arctan\left(\frac{q_b}{q_a}\right)$$
/// Under correlated arrivals with correlation coefficient $\rho$:
/// $$p_{\text{up}}(q_b, q_a) = \frac{1}{\pi} \arccos\left(\frac{q_b \cos\theta_0 - q_a}{\sqrt{q_b^2 + q_a^2 - 2 q_b q_a \cos\theta_0}}\right)$$
/// where $\theta_0 = \pi - \arccos(-\rho)$.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContDeLarrardPredictor {
    pub order_flow_correlation: f64,
}

impl Default for ContDeLarrardPredictor {
    fn default() -> Self {
        Self {
            order_flow_correlation: 0.0,
        }
    }
}

impl ContDeLarrardPredictor {
    pub fn new(order_flow_correlation: f64) -> Self {
        Self {
            order_flow_correlation: order_flow_correlation.clamp(-0.95, 0.95),
        }
    }

    /// Computes probability that the next price move is upward: $P(\tau_{\text{ask}} < \tau_{\text{bid}})$.
    /// `q_b`: visible depth at best bid; `q_a`: visible depth at best ask.
    pub fn prob_price_increase(&self, q_b: f64, q_a: f64) -> f64 {
        if q_b <= 1e-9 && q_a <= 1e-9 {
            return 0.50;
        }
        if q_a <= 1e-9 {
            return 1.00;
        }
        if q_b <= 1e-9 {
            return 0.00;
        }

        let rho = self.order_flow_correlation.clamp(-0.95, 0.95);
        if rho.abs() < 1e-4 {
            // Uncorrelated queue dynamics: p_up = (2 / pi) * arctan(q_b / q_a)
            (2.0 / std::f64::consts::PI) * (q_b / q_a).atan()
        } else {
            // General correlated diffusion limit
            let theta_0 = std::f64::consts::PI - (-rho).acos();
            let num = q_b * theta_0.cos() - q_a;
            let den = (q_b * q_b + q_a * q_a - 2.0 * q_b * q_a * theta_0.cos()).sqrt();
            if den <= 1e-12 {
                0.50
            } else {
                (1.0 / std::f64::consts::PI) * (num / den).clamp(-1.0, 1.0).acos()
            }
        }
    }
}

/// Hasbrouck (1995) and Gonzalo-Granger (1995) VECM Price Discovery Evaluator.
///
/// Analyzes cointegrated price dynamics between two venues $P_t = [p_{1,t}, p_{2,t}]^T$:
/// $$\Delta P_t = \alpha \beta^T P_{t-1} + \varepsilon_t, \quad \varepsilon_t \sim \mathcal{N}(0, \Omega)$$
/// where $\alpha = [\alpha_1, \alpha_2]^T$ are error-correction speeds and $\beta = [1, -1]^T$.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VecmPriceDiscovery {
    pub alpha_1: f64, // Adjustment speed of Venue 1
    pub alpha_2: f64, // Adjustment speed of Venue 2
    pub sigma_1: f64, // Residual standard deviation Venue 1
    pub sigma_2: f64, // Residual standard deviation Venue 2
    pub rho: f64,     // Innovation correlation
}

impl VecmPriceDiscovery {
    pub fn new(alpha_1: f64, alpha_2: f64, sigma_1: f64, sigma_2: f64, rho: f64) -> Self {
        Self {
            alpha_1,
            alpha_2,
            sigma_1: sigma_1.max(1e-6),
            sigma_2: sigma_2.max(1e-6),
            rho: rho.clamp(-0.99, 0.99),
        }
    }

    /// Gonzalo-Granger (1995) Component Share for Venue 1:
    /// $CS_1 = \frac{|\alpha_2|}{|\alpha_1| + |\alpha_2|}$
    /// The venue with smaller adjustment speed ($|\alpha_i| \approx 0$) dominates permanent price discovery.
    pub fn gonzalo_granger_cs_1(&self) -> f64 {
        let a1 = self.alpha_1.abs();
        let a2 = self.alpha_2.abs();
        if a1 + a2 <= 1e-12 {
            0.50
        } else {
            a2 / (a1 + a2)
        }
    }

    /// Hasbrouck (1995) Information Share bounds $[IS_{1,\min}, IS_{1,\max}]$ for Venue 1.
    pub fn hasbrouck_is_bounds_1(&self) -> (f64, f64) {
        let gamma_1 = -self.alpha_2;
        let gamma_2 = self.alpha_1;
        let var_total = gamma_1 * gamma_1 * self.sigma_1 * self.sigma_1
            + 2.0 * gamma_1 * gamma_2 * self.rho * self.sigma_1 * self.sigma_2
            + gamma_2 * gamma_2 * self.sigma_2 * self.sigma_2;

        if var_total <= 1e-12 {
            return (0.50, 0.50);
        }

        // Ordering 1: Venue 1 first in Cholesky factorization
        let f11_a = self.sigma_1;
        let f21_a = self.rho * self.sigma_2;
        let is_1_ord1 = (gamma_1 * f11_a + gamma_2 * f21_a).powi(2) / var_total;

        // Ordering 2: Venue 2 first in Cholesky factorization
        let f12_b = self.rho * self.sigma_1;
        let f22_b = self.sigma_2;
        let is_2_ord2 = (gamma_1 * f12_b + gamma_2 * f22_b).powi(2) / var_total;
        let is_1_ord2 = 1.0 - is_2_ord2;

        (
            is_1_ord1.min(is_1_ord2).clamp(0.0, 1.0),
            is_1_ord1.max(is_1_ord2).clamp(0.0, 1.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hayashi_yoshida_synthetic_lead() {
        let est = HayashiYoshidaEstimator::new(100, 10);
        let mut ticks_a = Vec::new();
        let mut ticks_b = Vec::new();

        // Venue A has jump innovations at specific intervals; Venue B follows with 30ms latency
        let mut px = 100.0;
        for i in 0..30 {
            let t_a = i * 50;
            let t_b = t_a + 30; // 30ms delay
            if i % 3 == 0 {
                px += 2.0;
            } else if i % 3 == 1 {
                px -= 1.0;
            }

            ticks_a.push(QuoteTick::new(t_a, px));
            ticks_b.push(QuoteTick::new(t_b, px));
        }

        let res = est.evaluate_lead_lag(&ticks_a, &ticks_b);
        assert!(res.optimal_lag_ms >= 20 && res.optimal_lag_ms <= 40);
        assert_eq!(res.leader, LeadVenue::VenueALeads);
        assert!(res.peak_correlation > 0.80);
    }

    #[test]
    fn test_spread_toxicity_stale_detection() {
        let eval = SpreadToxicityEvaluator::default();
        let dummy_lead = LeadLagResult {
            lead_lag_ratio: 1.0,
            optimal_lag_ms: 0,
            peak_correlation: 0.9,
            leader: LeadVenue::Synchronous,
            zero_lag_correlation: 0.9,
        };

        let result = eval.evaluate_toxicity(400, 100, &dummy_lead, 0.0, 10.0);
        assert_eq!(result, ToxicityAssessment::StalePhantomSpread);
    }

    #[test]
    fn test_spread_toxicity_adverse_selection() {
        let eval = SpreadToxicityEvaluator::default();
        let lead_a = LeadLagResult {
            lead_lag_ratio: 2.5,
            optimal_lag_ms: 40,
            peak_correlation: 0.95,
            leader: LeadVenue::VenueALeads,
            zero_lag_correlation: 0.6,
        };

        // Negative OFI on leader A indicates aggressive sell sweep
        let result = eval.evaluate_toxicity(50, 50, &lead_a, -3.0, 15.0);
        match result {
            ToxicityAssessment::ToxicAdverseSelection {
                expected_drift_bps,
                hurdle_expansion_bps,
            } => {
                assert!(expected_drift_bps > 5.0);
                assert!(hurdle_expansion_bps > 5.0);
            }
            _ => panic!("Expected toxic adverse selection"),
        }
    }

    #[test]
    fn test_cont_de_larrard_queue_probabilities() {
        let model = ContDeLarrardPredictor::default();

        // Equal depth -> 50% probability of upward tick
        let p_eq = model.prob_price_increase(10.0, 10.0);
        assert!((p_eq - 0.50).abs() < 1e-4);

        // Heavy bid queue (100 bid vs 1 ask) -> high upward probability
        let p_bid_heavy = model.prob_price_increase(100.0, 1.0);
        assert!(p_bid_heavy > 0.95);

        // Heavy ask queue (1 bid vs 100 ask) -> low upward probability
        let p_ask_heavy = model.prob_price_increase(1.0, 100.0);
        assert!(p_ask_heavy < 0.05);

        // Correlated model: positive correlation shifts probability
        let model_corr = ContDeLarrardPredictor::new(0.5);
        let p_corr = model_corr.prob_price_increase(10.0, 10.0);
        assert!(p_corr > 0.0 && p_corr < 1.0);
    }

    #[test]
    fn test_vecm_price_discovery_shares() {
        // Venue 1 has small adjustment speed (alpha_1 = -0.05), Venue 2 has large adjustment speed (alpha_2 = 0.45)
        // Venue 1 leads price discovery (does not adjust to Venue 2, Venue 2 adjusts to Venue 1)
        let vecm = VecmPriceDiscovery::new(-0.05, 0.45, 1.0, 1.2, 0.4);

        let cs_1 = vecm.gonzalo_granger_cs_1();
        assert!((cs_1 - 0.90).abs() < 1e-4);

        let (is_min, is_max) = vecm.hasbrouck_is_bounds_1();
        assert!(is_min >= 0.70);
        assert!(is_max <= 1.00);
        assert!(is_min <= is_max);
    }

    #[test]
    fn test_multi_level_ofi_tracker_and_preemptor() {
        let mut tracker = MultiLevelOfiTracker::new(0.90);
        // Feed baseline zero-mean samples
        for _ in 0..20 {
            tracker.ingest_ofi(0.0);
        }
        // Sudden massive positive OFI shock
        let z_shock = tracker.ingest_ofi(15.0);
        assert!(
            z_shock > 2.0,
            "Expected Z-score > 2.0 on shock, got {z_shock}"
        );

        let preemptor = OfiLeadLagPreemptor::new(3.0, 40.0, 2.0);
        // Follower friction: half spread 1.0 bps + taker fee 0.5 bps + slippage 0.5 bps = 2.0 bps
        let decision = preemptor.evaluate_preemption(z_shock, 1.0, 0.5, 0.5);
        match decision {
            OfiPreemptionDecision::SnipeFollower {
                target_side,
                expected_drift_bps,
                total_friction_bps,
            } => {
                assert_eq!(target_side, Side::Buy);
                assert!(expected_drift_bps >= total_friction_bps);
            }
            OfiPreemptionDecision::HoldNoSignal => panic!("Expected SnipeFollower decision"),
        }

        // Sub-threshold OFI should hold
        let hold_decision = preemptor.evaluate_preemption(1.0, 1.0, 0.5, 0.5);
        assert_eq!(hold_decision, OfiPreemptionDecision::HoldNoSignal);
    }
}
