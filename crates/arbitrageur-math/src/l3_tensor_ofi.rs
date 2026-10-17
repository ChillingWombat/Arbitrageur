//! Level-3 Order Flow Imbalance (OFI) Tensor Field and Microstructural Queue Evacuation.
//!
//! Models order book microstructure as an L3 tensor field T_OFI(t) in R^{10 x 4 x 2} across 10 depth levels,
//! 4 order actions (Add, Cancel, Modify, Sweep), and 2 sides (Bid, Ask) with spatial exponential dampening.
//! Computes the Microstructural Queue Depletion Indicator (MQDI) and continuous VPIN to trigger sub-2.5ms
//! maker quote evacuation ahead of toxic adverse selection sweeps.

use serde::{Deserialize, Serialize};

/// Order actions in Level-3 MBO streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum L3OrderAction {
    Add = 0,
    Cancel = 1,
    Modify = 2,
    Sweep = 3,
}

/// Side of the order book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum L3Side {
    Bid = 0,
    Ask = 1,
}

/// Configuration for Level-3 OFI Tensor & Maker Quote Evacuation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3TensorOfiConfig {
    /// Spatial depth exponential decay rate kappa (default: 0.25).
    pub kappa_dampening: f64,
    /// Critical Microstructural Queue Depletion Indicator threshold (default: 1.0).
    pub mqdi_critical_threshold: f64,
    /// Critical VPIN toxicity threshold (default: 0.72).
    pub vpin_critical_threshold: f64,
    /// Number of depth levels tracked (default: 10).
    pub depth_levels: usize,
    /// Quote evacuation withdrawal offset in basis points (default: 30.0).
    pub evacuation_withdrawal_bps: f64,
}

impl Default for L3TensorOfiConfig {
    fn default() -> Self {
        Self {
            kappa_dampening: 0.25,
            mqdi_critical_threshold: 1.0,
            vpin_critical_threshold: 0.72,
            depth_levels: 10,
            evacuation_withdrawal_bps: 30.0,
        }
    }
}

/// Level-3 OFI Tensor Field: R^{10 x 4 x 2}.
/// Dimensions: [depth_level (0..10)][action (0..4)][side (0..2)].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3OfiTensor {
    /// Raw volume flows in tensor: [level][action][side].
    pub tensor: [[[f64; 2]; 4]; 10],
    /// Level spatial weights w_l = exp(-kappa * (l - 1)).
    pub weights: [f64; 10],
}

impl L3OfiTensor {
    pub fn new(kappa_dampening: f64) -> Self {
        let mut weights = [0.0; 10];
        for (l, w) in weights.iter_mut().enumerate() {
            *w = (-kappa_dampening * (l as f64)).exp();
        }
        Self {
            tensor: [[[0.0; 2]; 4]; 10],
            weights,
        }
    }

    /// Records order volume into tensor cell.
    pub fn record(&mut self, level: usize, action: L3OrderAction, side: L3Side, volume: f64) {
        if level < 10 {
            let a_idx = action as usize;
            let s_idx = side as usize;
            self.tensor[level][a_idx][s_idx] += volume;
        }
    }

    /// Resets tensor volume accumulators.
    pub fn reset(&mut self) {
        self.tensor = [[[0.0; 2]; 4]; 10];
    }
}

/// Recommendation for maker quote evacuation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MakerQuoteEvacuationDecision {
    /// True if resting quotes must be immediately evacuated/withdrawn.
    pub should_evacuate: bool,
    /// Specific evacuation trigger reason.
    pub reason: String,
    /// Recommended price withdrawal in basis points to escape adverse fill.
    pub withdrawal_bps: f64,
    /// Microstructural Queue Depletion Indicator on the affected side.
    pub mqdi: f64,
    /// Current Volume-Synchronized Probability of Toxicity (VPIN).
    pub current_vpin: f64,
}

/// Engine for Level-3 OFI Tensor Field tracking and sub-2.5ms maker quote evacuation.
#[derive(Debug, Clone)]
pub struct L3TensorOfiEngine {
    config: L3TensorOfiConfig,
    tensor: L3OfiTensor,
    // Rolling top-of-book state for MQDI computation
    top_bid_volume: f64,
    top_ask_volume: f64,
    // Rolling VPIN volume bucket trackers
    bucket_volume_buy: f64,
    bucket_volume_sell: f64,
    bucket_size: f64,
    vpin_history: Vec<f64>,
}

impl L3TensorOfiEngine {
    pub fn new(config: L3TensorOfiConfig, bucket_size: f64) -> Self {
        let tensor = L3OfiTensor::new(config.kappa_dampening);
        Self {
            config,
            tensor,
            top_bid_volume: 1.0,
            top_ask_volume: 1.0,
            bucket_volume_buy: 0.0,
            bucket_volume_sell: 0.0,
            bucket_size: bucket_size.max(1.0),
            vpin_history: Vec::with_capacity(50),
        }
    }

    /// Updates top-of-book prevailing queue depths.
    pub fn update_top_of_book(&mut self, top_bid_vol: f64, top_ask_vol: f64) {
        self.top_bid_volume = top_bid_vol.max(1e-4);
        self.top_ask_volume = top_ask_vol.max(1e-4);
    }

    /// Ingests an L3 order event into the tensor and rolling VPIN buckets.
    pub fn ingest_event(&mut self, level: usize, action: L3OrderAction, side: L3Side, volume: f64) {
        self.tensor.record(level, action, side, volume);

        // VPIN trade bucket accounting
        if action == L3OrderAction::Sweep {
            match side {
                L3Side::Ask => {
                    // Taker buy sweeping asks
                    self.bucket_volume_buy += volume;
                }
                L3Side::Bid => {
                    // Taker sell sweeping bids
                    self.bucket_volume_sell += volume;
                }
            }

            let total_bucket = self.bucket_volume_buy + self.bucket_volume_sell;
            if total_bucket >= self.bucket_size {
                let imbalance = (self.bucket_volume_buy - self.bucket_volume_sell).abs();
                let vpin_sample = imbalance / total_bucket;
                if self.vpin_history.len() >= 50 {
                    self.vpin_history.remove(0);
                }
                self.vpin_history.push(vpin_sample);
                self.bucket_volume_buy = 0.0;
                self.bucket_volume_sell = 0.0;
            }
        }
    }

    /// Computes Level-specific Order Flow Imbalance OFI_l(t).
    /// OFI_l = Delta V_bid,l - Delta V_ask,l
    /// where Delta V = V_add - V_cancel - V_sweep + Delta V_modify
    pub fn compute_level_ofi(&self, level: usize) -> f64 {
        if level >= 10 {
            return 0.0;
        }

        // Bids: Add (+) Cancel (-) Sweep (-) Modify (+)
        let bid_add = self.tensor.tensor[level][L3OrderAction::Add as usize][L3Side::Bid as usize];
        let bid_cancel =
            self.tensor.tensor[level][L3OrderAction::Cancel as usize][L3Side::Bid as usize];
        let bid_sweep =
            self.tensor.tensor[level][L3OrderAction::Sweep as usize][L3Side::Bid as usize];
        let bid_modify =
            self.tensor.tensor[level][L3OrderAction::Modify as usize][L3Side::Bid as usize];
        let delta_v_bid = bid_add - bid_cancel - bid_sweep + bid_modify;

        // Asks: Add (+) Cancel (-) Sweep (-) Modify (+)
        let ask_add = self.tensor.tensor[level][L3OrderAction::Add as usize][L3Side::Ask as usize];
        let ask_cancel =
            self.tensor.tensor[level][L3OrderAction::Cancel as usize][L3Side::Ask as usize];
        let ask_sweep =
            self.tensor.tensor[level][L3OrderAction::Sweep as usize][L3Side::Ask as usize];
        let ask_modify =
            self.tensor.tensor[level][L3OrderAction::Modify as usize][L3Side::Ask as usize];
        let delta_v_ask = ask_add - ask_cancel - ask_sweep + ask_modify;

        delta_v_bid - delta_v_ask
    }

    /// Computes the integrated spatially-dampened OFI across all 10 depth levels:
    /// OFI_tensor(t) = sum_{l=1}^{10} w_l * OFI_l(t)
    pub fn compute_integrated_tensor_ofi(&self) -> f64 {
        let mut total_ofi = 0.0;
        for l in 0..10 {
            let level_ofi = self.compute_level_ofi(l);
            total_ofi += self.tensor.weights[l] * level_ofi;
        }
        total_ofi
    }

    /// Computes Microstructural Queue Depletion Indicator (MQDI) for a given side.
    /// MQDI_bid = (v_sweep + v_cancel - v_add) / V_bid,1
    /// When MQDI >= 1.0, net queue drain exceeds replenishment and queue depletion is imminent.
    pub fn compute_mqdi(&self, side: L3Side) -> f64 {
        let sweep = self.tensor.tensor[0][L3OrderAction::Sweep as usize][side as usize];
        let cancel = self.tensor.tensor[0][L3OrderAction::Cancel as usize][side as usize];
        let add = self.tensor.tensor[0][L3OrderAction::Add as usize][side as usize];

        let net_depletion = sweep + cancel - add;
        let prevailing_vol = match side {
            L3Side::Bid => self.top_bid_volume,
            L3Side::Ask => self.top_ask_volume,
        };

        net_depletion / prevailing_vol
    }

    /// Computes current rolling VPIN toxicity estimate.
    pub fn current_vpin(&self) -> f64 {
        if self.vpin_history.is_empty() {
            0.0
        } else {
            let sum: f64 = self.vpin_history.iter().sum();
            sum / (self.vpin_history.len() as f64)
        }
    }

    /// Evaluates resting maker quotes for immediate evacuation.
    /// Triggered sub-2.5ms when MQDI >= 1.0 or VPIN >= 0.72.
    pub fn evaluate_evacuation(&self, side: L3Side) -> MakerQuoteEvacuationDecision {
        let mqdi = self.compute_mqdi(side);
        let vpin = self.current_vpin();

        let mqdi_breached = mqdi >= self.config.mqdi_critical_threshold;
        let vpin_breached = vpin >= self.config.vpin_critical_threshold;

        if mqdi_breached && vpin_breached {
            MakerQuoteEvacuationDecision {
                should_evacuate: true,
                reason: format!(
                    "CRITICAL_COMPOUND_TOXICITY: MQDI={:.2} >= {:.2} AND VPIN={:.2} >= {:.2}",
                    mqdi,
                    self.config.mqdi_critical_threshold,
                    vpin,
                    self.config.vpin_critical_threshold
                ),
                withdrawal_bps: self.config.evacuation_withdrawal_bps * 1.5,
                mqdi,
                current_vpin: vpin,
            }
        } else if mqdi_breached {
            MakerQuoteEvacuationDecision {
                should_evacuate: true,
                reason: format!(
                    "MQDI_QUEUE_DEPLETION_SURGE: MQDI={:.2} >= {:.2}",
                    mqdi, self.config.mqdi_critical_threshold
                ),
                withdrawal_bps: self.config.evacuation_withdrawal_bps,
                mqdi,
                current_vpin: vpin,
            }
        } else if vpin_breached {
            MakerQuoteEvacuationDecision {
                should_evacuate: true,
                reason: format!(
                    "VPIN_TOXIC_FLOW_SPIKE: VPIN={:.2} >= {:.2}",
                    vpin, self.config.vpin_critical_threshold
                ),
                withdrawal_bps: self.config.evacuation_withdrawal_bps,
                mqdi,
                current_vpin: vpin,
            }
        } else {
            MakerQuoteEvacuationDecision {
                should_evacuate: false,
                reason: "NORMAL_MICROSTRUCTURE_FLOW".into(),
                withdrawal_bps: 0.0,
                mqdi,
                current_vpin: vpin,
            }
        }
    }

    /// Resets the tensor interval (e.g. at periodic snapshot boundaries).
    pub fn reset_tensor(&mut self) {
        self.tensor.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tensor_spatial_weights() {
        let tensor = L3OfiTensor::new(0.25);
        assert_eq!(tensor.weights[0], 1.0);
        assert!((tensor.weights[1] - (-0.25f64).exp()).abs() < 1e-6);
        assert!(tensor.weights[9] < tensor.weights[0]);
    }

    #[test]
    fn test_integrated_tensor_ofi() {
        let mut engine = L3TensorOfiEngine::new(L3TensorOfiConfig::default(), 100.0);
        // Level 0: 10 BTC added to bid
        engine.ingest_event(0, L3OrderAction::Add, L3Side::Bid, 10.0);
        // Level 1: 5 BTC added to ask
        engine.ingest_event(1, L3OrderAction::Add, L3Side::Ask, 5.0);

        let ofi_0 = engine.compute_level_ofi(0);
        assert_eq!(ofi_0, 10.0);
        let ofi_1 = engine.compute_level_ofi(1);
        assert_eq!(ofi_1, -5.0);

        let integrated = engine.compute_integrated_tensor_ofi();
        // integrated = 1.0 * 10.0 + exp(-0.25) * (-5.0)
        let expected = 10.0 - 5.0 * (-0.25f64).exp();
        assert!((integrated - expected).abs() < 1e-4);
    }

    #[test]
    fn test_mqdi_queue_evacuation() {
        let mut engine = L3TensorOfiEngine::new(L3TensorOfiConfig::default(), 50.0);
        engine.update_top_of_book(5.0, 5.0);

        // Prevailing bid is 5.0 BTC.
        // A sweep of 4.0 BTC arrives and 2.0 BTC cancels: net depletion = 6.0 > 5.0 -> MQDI = 1.20 >= 1.0
        engine.ingest_event(0, L3OrderAction::Sweep, L3Side::Bid, 4.0);
        engine.ingest_event(0, L3OrderAction::Cancel, L3Side::Bid, 2.0);

        let decision = engine.evaluate_evacuation(L3Side::Bid);
        assert!(decision.should_evacuate);
        assert!(decision.mqdi >= 1.0);
        assert!(decision.withdrawal_bps >= 30.0);
    }

    #[test]
    fn test_vpin_toxicity_evacuation() {
        let mut engine = L3TensorOfiEngine::new(L3TensorOfiConfig::default(), 10.0);
        engine.update_top_of_book(10.0, 10.0);

        // Fill buckets with heavily one-sided toxic sweeps (e.g. 10.0 buy, 0.0 sell)
        for _ in 0..5 {
            engine.ingest_event(0, L3OrderAction::Sweep, L3Side::Ask, 10.0);
        }

        let decision = engine.evaluate_evacuation(L3Side::Ask);
        assert!(decision.should_evacuate);
        assert!(decision.current_vpin >= 0.72);
    }
}
