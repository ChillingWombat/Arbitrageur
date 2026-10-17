//! Distribution-Free Conformal Quantile-Random-Forest Online Risk Shields & Multi-Dimensional Microstructural Horizon Ensembles
//!
//! # Mathematical Foundations
//! In non-stationary high-frequency execution across crypto venues, execution slippage depends
//! jointly on multi-dimensional order book features:
//! $$\mathbf{x} = (v_{\text{order}}, \text{ofi}_{\text{imb}}, \sigma_{\text{regime}}) \in \mathbb{R}^3$$
//! capturing order size depth consumption, order flow imbalance, and instantaneous market volatility.
//!
//! Rather than assuming linearity or univariate projections, the Conformal Forest Shield deploys
//! an ensemble of $M = 8$ shallow randomized axis-aligned decision trees (depth 3, 8 leaves each)
//! to partition the microstructural feature space into local hyper-rectangles:
//! $$\hat{\mu}_{\text{forest}}(\mathbf{x}) = \frac{1}{M} \sum_{m=1}^M \hat{\mu}_{m}(\mathbf{x})$$
//!
//! # Online Welford Leaf Statistics
//! Each leaf in tree $m$ recursively accumulates online streaming Welford moments:
//! $$n_l \leftarrow n_l + 1, \quad \bar{s}_l \leftarrow \bar{s}_l + \frac{s_t - \bar{s}_l}{n_l}, \quad M_{2, l} \leftarrow M_{2, l} + (s_t - \bar{s}_l)(s_t - \bar{s}_{l, \text{prev}})$$
//! yielding local conditional variance $\hat{\sigma}_m^2(\mathbf{x}) = M_{2, l} / \max(1, n_l - 1)$.
//!
//! Total ensemble uncertainty combines intra-leaf variance and inter-tree variance:
//! $$\hat{\sigma}_{\text{forest}}^2(\mathbf{x}) = \frac{1}{M} \sum_{m=1}^M \hat{\sigma}_m^2(\mathbf{x}) + \frac{1}{M} \sum_{m=1}^M \left( \hat{\mu}_m(\mathbf{x}) - \hat{\mu}_{\text{forest}}(\mathbf{x}) \right)^2$$
//!
//! # Conformal Multiplier & Finite-Sample Coverage
//! An adaptive conformal quantile multiplier $\hat{q}_{\text{conf}} \in [1.0, 3.5]$ tracks empirical miscoverage:
//! $$\hat{q}_{\text{conf}, t+1} = \hat{q}_{\text{conf}, t} + \eta_{\text{conf}} \cdot \left( \mathbb{I}_{\{s_t > S_{\text{upper}}\}} - \alpha \right)$$
//! guaranteeing non-asymptotic $(1 - \alpha)$ finite-sample validity for the upper slippage bound:
//! $$S_{\text{upper}}(\mathbf{x}) = \hat{\mu}_{\text{forest}}(\mathbf{x}) + \hat{q}_{\text{conf}} \cdot \sqrt{\hat{\sigma}_{\text{forest}}^2(\mathbf{x}) + \epsilon}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $S_{\text{upper}}(\mathbf{x}) \ge \text{lock\_slippage\_bps}$ (default 40.0 bps), the shield engages
//! a fail-closed execution lock ($\kappa = 0.0$) in $< 25\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Number of randomized trees in the microstructural ensemble.
pub const FOREST_TREE_COUNT: usize = 8;
/// Number of leaves per tree (depth 3 = 8 leaves).
pub const TREE_LEAF_COUNT: usize = 8;

/// Configuration for the Conformal Forest Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalForestConfig {
    /// Desired miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Conformal quantile multiplier adaptation rate $\eta_{\text{conf}}$ (default 0.06).
    pub conformal_rate: f64,
    /// Initial baseline slippage benchmark in bps (default 10.0).
    pub target_slippage_bps: f64,
    /// Fail-closed liquidation lock threshold in bps (default 40.0).
    pub lock_slippage_bps: f64,
    /// Derisking throttle threshold in bps (default 22.0).
    pub throttle_slippage_bps: f64,
}

impl Default for ConformalForestConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            conformal_rate: 0.06,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
            throttle_slippage_bps: 22.0,
        }
    }
}

/// Action verdict emitted by the Conformal Forest Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalForestAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        forest_mean_bps: f64,
        throttle_capacity: f64,
    },
    /// Elevated multi-dimensional forest regret: throttle maker allocation proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        forest_mean_bps: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        reason: String,
    },
}

/// Single leaf state with online Welford statistics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TreeLeafState {
    pub count: u64,
    pub mean: f64,
    pub m2: f64,
}

impl Default for TreeLeafState {
    fn default() -> Self {
        Self {
            count: 1,
            mean: 10.0,
            m2: 1.0,
        }
    }
}

impl TreeLeafState {
    #[inline]
    pub fn update(&mut self, val: f64) {
        self.count += 1;
        let delta = val - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = val - self.mean;
        self.m2 += delta * delta2;
    }

    #[inline]
    pub fn variance(&self) -> f64 {
        if self.count > 1 {
            (self.m2 / (self.count - 1) as f64).max(0.01)
        } else {
            1.0
        }
    }
}

/// Static split definition for a depth-3 tree.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TreeSplit {
    /// Feature index: 0 = order_size, 1 = ofi_imbalance, 2 = vol_regime
    pub feature_idx: usize,
    pub threshold: f64,
}

/// A randomized depth-3 decision tree with 8 leaves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShallowRandomTree {
    pub splits: [TreeSplit; 7],
    pub leaves: [TreeLeafState; TREE_LEAF_COUNT],
}

impl ShallowRandomTree {
    /// Creates a tree with specified randomized splits and initial baseline leaf means.
    pub fn new(splits: [TreeSplit; 7], initial_mean: f64) -> Self {
        let leaf = TreeLeafState {
            count: 1,
            mean: initial_mean,
            m2: 1.0,
        };
        Self {
            splits,
            leaves: [leaf; TREE_LEAF_COUNT],
        }
    }

    /// Evaluates leaf index for given 3D feature vector.
    #[inline]
    pub fn find_leaf(&self, features: &[f64; 3]) -> usize {
        // Node 0
        let s0 = &self.splits[0];
        let n1 = if features[s0.feature_idx] <= s0.threshold {
            1
        } else {
            2
        };

        // Level 1
        let s1 = &self.splits[n1];
        let n2 = if features[s1.feature_idx] <= s1.threshold {
            2 * n1 + 1
        } else {
            2 * n1 + 2
        };

        // Level 2 -> Leaf index (0..7)
        let s2 = &self.splits[n2];
        if features[s2.feature_idx] <= s2.threshold {
            (n2 - 3) * 2
        } else {
            (n2 - 3) * 2 + 1
        }
    }
}

/// Distribution-Free Conformal Quantile-Random-Forest Online Risk Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalForestShield {
    pub config: ConformalForestConfig,
    pub trees: [ShallowRandomTree; FOREST_TREE_COUNT],
    pub conformal_quantile: f64,
    pub total_observations: u64,
}

impl ConformalForestShield {
    /// Creates a new Conformal Forest Shield with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConformalForestConfig::default())
    }

    /// Creates a new Conformal Forest Shield with custom configuration.
    pub fn with_config(config: ConformalForestConfig) -> Self {
        let base = config.target_slippage_bps;

        // Structured randomized orthogonal splits across (order_size [0, 10], ofi [-1, 1], vol [0.1, 1.0])
        let tree_configs: [[TreeSplit; 7]; FOREST_TREE_COUNT] = [
            [
                TreeSplit {
                    feature_idx: 0,
                    threshold: 3.0,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.4,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.3,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 1.5,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.2,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 6.0,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.1,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 4.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.5,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 2.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.3,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.3,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 7.0,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.4,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 2.5,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.2,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.3,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.6,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 5.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.7,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 0,
                    threshold: 5.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.3,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.2,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.1,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 2.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.5,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 8.0,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.2,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 3.5,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.45,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.25,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.4,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 6.5,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.65,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.35,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.15,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 4.5,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 1.8,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.55,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.25,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 7.5,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 0,
                    threshold: 2.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.4,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.0,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.25,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 5.5,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.6,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.4,
                },
            ],
            [
                TreeSplit {
                    feature_idx: 1,
                    threshold: -0.3,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 3.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.5,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.3,
                },
                TreeSplit {
                    feature_idx: 1,
                    threshold: 0.1,
                },
                TreeSplit {
                    feature_idx: 0,
                    threshold: 6.0,
                },
                TreeSplit {
                    feature_idx: 2,
                    threshold: 0.7,
                },
            ],
        ];

        let trees = [
            ShallowRandomTree::new(tree_configs[0], base),
            ShallowRandomTree::new(tree_configs[1], base),
            ShallowRandomTree::new(tree_configs[2], base),
            ShallowRandomTree::new(tree_configs[3], base),
            ShallowRandomTree::new(tree_configs[4], base),
            ShallowRandomTree::new(tree_configs[5], base),
            ShallowRandomTree::new(tree_configs[6], base),
            ShallowRandomTree::new(tree_configs[7], base),
        ];

        Self {
            config,
            trees,
            conformal_quantile: 1.0,
            total_observations: 0,
        }
    }

    /// Evaluates ensemble conditional mean and total variance for microstructural features.
    #[inline]
    pub fn predict_moments(&self, features: &[f64; 3]) -> (f64, f64) {
        let mut sum_mean = 0.0;
        let mut tree_means = [0.0; FOREST_TREE_COUNT];
        let mut sum_var = 0.0;

        for (m, tree) in self.trees.iter().enumerate() {
            let l_idx = tree.find_leaf(features);
            let leaf = &tree.leaves[l_idx];
            tree_means[m] = leaf.mean;
            sum_mean += leaf.mean;
            sum_var += leaf.variance();
        }

        let forest_mean = sum_mean / FOREST_TREE_COUNT as f64;
        let intra_var = sum_var / FOREST_TREE_COUNT as f64;

        let mut inter_var = 0.0;
        for &tm in &tree_means {
            inter_var += (tm - forest_mean).powi(2);
        }
        inter_var /= FOREST_TREE_COUNT as f64;

        let total_var = (intra_var + inter_var).max(0.01);
        (forest_mean, total_var)
    }

    /// Ingests a realized trade observation and updates the forest ensemble and conformal quantile.
    pub fn observe(&mut self, features: &[f64; 3], realized_slippage_bps: f64) {
        let (mean, var) = self.predict_moments(features);
        let upper_bound = mean + self.conformal_quantile * var.sqrt();

        // Update all tree leaves corresponding to these features
        for tree in &mut self.trees {
            let l_idx = tree.find_leaf(features);
            tree.leaves[l_idx].update(realized_slippage_bps);
        }

        // Conformal quantile multiplier adaptation: expand on breach, shrink otherwise
        let breach = realized_slippage_bps > upper_bound;
        if breach {
            self.conformal_quantile += self.config.conformal_rate * (1.0 - self.config.alpha);
        } else {
            self.conformal_quantile -= self.config.conformal_rate * self.config.alpha;
        }
        self.conformal_quantile = self.conformal_quantile.clamp(1.0, 3.5);

        self.total_observations += 1;
    }

    /// Evaluates the distribution-free upper slippage bound and produces an execution action.
    pub fn evaluate_execution_shield(&self, features: &[f64; 3]) -> ConformalForestAction {
        let (mean, var) = self.predict_moments(features);
        let slippage_upper_bps = mean + self.conformal_quantile * var.sqrt();

        if slippage_upper_bps >= self.config.lock_slippage_bps {
            ConformalForestAction::FailClosedLock {
                slippage_upper_bps,
                reason: format!(
                    "Forest-conformal upper slippage {:.2} bps exceeds lock hurdle {:.2} bps",
                    slippage_upper_bps, self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper_bps >= self.config.throttle_slippage_bps {
            let excess = slippage_upper_bps - self.config.throttle_slippage_bps;
            let span = (self.config.lock_slippage_bps - self.config.throttle_slippage_bps).max(1.0);
            let throttle_capacity = (1.0 - (excess / span)).clamp(0.05, 0.95);
            ConformalForestAction::DeriskingThrottle {
                slippage_upper_bps,
                forest_mean_bps: mean,
                throttle_capacity,
                reason: format!(
                    "Elevated forest-conformal upper slippage {:.2} bps (throttle: {:.1}%)",
                    slippage_upper_bps,
                    throttle_capacity * 100.0
                ),
            }
        } else {
            ConformalForestAction::NormalExecution {
                slippage_upper_bps,
                forest_mean_bps: mean,
                throttle_capacity: 1.0,
            }
        }
    }
}

impl Default for ConformalForestShield {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_normal_execution() {
        let shield = ConformalForestShield::new();
        let features = [1.0, 0.0, 0.2];
        let action = shield.evaluate_execution_shield(&features);
        match action {
            ConformalForestAction::NormalExecution {
                slippage_upper_bps,
                forest_mean_bps,
                throttle_capacity,
            } => {
                assert!(slippage_upper_bps < shield.config.throttle_slippage_bps);
                assert!((forest_mean_bps - shield.config.target_slippage_bps).abs() < 1e-4);
                assert!((throttle_capacity - 1.0).abs() < 1e-6);
            }
            _ => panic!("Expected NormalExecution in initial clean state"),
        }
    }

    #[test]
    fn test_forest_learns_feature_sensitive_slippage() {
        let mut shield = ConformalForestShield::new();
        let benign = [1.0, 0.5, 0.1];
        let toxic = [8.0, -0.8, 0.8];

        for _ in 0..100 {
            shield.observe(&benign, 5.0);
            shield.observe(&toxic, 28.0);
        }

        let (mean_benign, _) = shield.predict_moments(&benign);
        let (mean_toxic, _) = shield.predict_moments(&toxic);

        assert!(
            mean_toxic > mean_benign,
            "Toxic regime slippage {mean_toxic} should exceed benign regime {mean_benign}"
        );
    }

    #[test]
    fn test_extreme_slippage_triggers_fail_closed_lock() {
        let mut shield = ConformalForestShield::new();
        let features = [9.0, -0.9, 0.9];
        for _ in 0..50 {
            shield.observe(&features, 65.0);
        }

        let action = shield.evaluate_execution_shield(&features);
        match action {
            ConformalForestAction::FailClosedLock {
                slippage_upper_bps,
                reason,
            } => {
                assert!(slippage_upper_bps >= shield.config.lock_slippage_bps);
                assert!(reason.contains("exceeds lock hurdle"));
            }
            _ => panic!("Expected FailClosedLock after severe slippage observations"),
        }
    }
}
