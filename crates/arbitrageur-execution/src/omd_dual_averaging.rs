//! Online Mirror Descent (OMD) & Regularized Dual Averaging for Fast Dynamic Hedging (Tranche 76).
//!
//! Classical gradient descent for delta and vega hedging assumes a Euclidean geometry on the
//! action space $w \in \Delta_K = \{ w \in \mathbb{R}_+^K \mid \sum_{i=1}^K w_i = 1 \}$ (the probability simplex
//! of candidate hedging venues / instruments), leading to slow $\mathcal{O}(1/\sqrt{T})$ convergence
//! and projection boundary oscillations under sharp market jumps.
//!
//! Implements **Online Mirror Descent (OMD)** (Nemirovski-Yudin, Beck-Teboulle) with entropic
//! mirror map (negative Shannon entropy):
//! $$\Phi(w) = \sum_{i=1}^K w_i \ln w_i$$
//! producing the **Hedge / Multiplicative Weights Update Algorithm** with Bregman divergence
//! $D_\Phi(w, w_t) = \operatorname{KL}(w \| w_t) = \sum_{i=1}^K w_i \ln(w_i / w_{t, i})$:
//! $$w_{t+1, i} = \frac{w_{t, i} \exp(-\eta_t g_{t, i})}{\sum_{j=1}^K w_{t, j} \exp(-\eta_t g_{t, j})}$$
//!
//! Also implements Xiao's **Regularized Dual Averaging (RDA)** with $\ell_1$-regularization
//! $\lambda_{\text{turnover}} \|w - w_{\text{target}}\|_1$ to penalize portfolio turnover and transaction
//! fee drag, enforcing exact sparsity (zero-rebalance thresholding when marginal tracking gain
//! is smaller than exchange maker/taker fees):
//! $$w_{t+1} = \arg\min_{w \in \Delta_K} \left\{ \langle \bar{g}_t, w \rangle + \lambda \|w - w_0\|_1 + \frac{\beta_t}{t} \Phi(w) \right\}$$
//!
//! Guarantees optimal $\mathcal{O}(\sqrt{\ln K / T})$ regret bound across $K$ cross-venue hedging instruments.

use serde::{Deserialize, Serialize};

/// Online Mirror Descent Hedging Engine over the $K$-dimensional simplex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineMirrorDescentHedger {
    pub dimension: usize,
    /// Current normalized portfolio weights $w_t \in \Delta_K$.
    pub weights: Vec<f64>,
    /// Base learning rate $\eta_0 > 0$.
    pub base_learning_rate: f64,
    /// Step counter $t \ge 1$.
    pub step_count: u64,
}

impl OnlineMirrorDescentHedger {
    /// Initializes uniform weights $w_0 = [1/K, \dots, 1/K]$.
    pub fn new(dimension: usize, base_learning_rate: f64) -> Self {
        let dim = dimension.max(1);
        let uniform = 1.0 / (dim as f64);
        Self {
            dimension: dim,
            weights: vec![uniform; dim],
            base_learning_rate: base_learning_rate.max(1e-4),
            step_count: 0,
        }
    }

    /// Computes adaptive step size $\eta_t = \frac{\eta_0}{\sqrt{t}}$ guaranteeing $\mathcal{O}(\sqrt{\ln K / T})$ regret.
    #[inline(always)]
    pub fn step_size(&self) -> f64 {
        let t = (self.step_count + 1) as f64;
        self.base_learning_rate / t.sqrt()
    }

    /// Updates weights via Entropic Mirror Descent given loss gradient $g_t \in \mathbb{R}^K$:
    /// $$w_{t+1, i} = \frac{w_{t, i} \exp(-\eta_t g_{t, i})}{\sum_{j=1}^K w_{t, j} \exp(-\eta_t g_{t, j})}$$
    pub fn update_entropic(&mut self, loss_gradient: &[f64]) {
        if loss_gradient.len() != self.dimension {
            return;
        }
        self.step_count += 1;
        let eta = self.step_size();

        // Numerically stable softmax using max-shift
        let mut scaled_log = Vec::with_capacity(self.dimension);
        let mut max_val = f64::NEG_INFINITY;
        for (w, &g) in self.weights.iter().zip(loss_gradient.iter()) {
            let exponent = -eta * g;
            let log_weight = (w.max(1e-12)).ln() + exponent;
            if log_weight > max_val {
                max_val = log_weight;
            }
            scaled_log.push(log_weight);
        }

        let mut sum_exp = 0.0;
        for val in &mut scaled_log {
            let exp_val = (*val - max_val).exp();
            *val = exp_val;
            sum_exp += exp_val;
        }

        if sum_exp > 0.0 {
            for (w, &val) in self.weights.iter_mut().zip(scaled_log.iter()) {
                *w = val / sum_exp;
            }
        }
    }

    /// Computes the Kullback-Leibler (Bregman) divergence $D_{\operatorname{KL}}(u \| v) = \sum_i u_i \ln(u_i / v_i)$.
    pub fn kl_divergence(&self, target_weights: &[f64]) -> f64 {
        if target_weights.len() != self.dimension {
            return 0.0;
        }
        let mut kl = 0.0;
        for (&u, &v) in target_weights.iter().zip(self.weights.iter()) {
            if u > 1e-12 && v > 1e-12 {
                kl += u * (u / v).ln();
            }
        }
        kl.max(0.0)
    }
}

/// Regularized Dual Averaging (RDA) Hedger with $\ell_1$-Turnover Penalty and Soft-Thresholding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegularizedDualAveragingHedger {
    pub dimension: usize,
    /// Accumulated dual gradient sum $\bar{G}_t = \sum_{s=1}^t g_s$.
    pub dual_gradient_sum: Vec<f64>,
    /// Anchor portfolio target $w_0$ (e.g. baseline delta-neutral basket).
    pub anchor_target: Vec<f64>,
    /// Turnover penalty parameter $\lambda_{\text{turnover}} \ge 0$ (e.g. bid-ask fee in bps).
    pub lambda_turnover: f64,
    /// Regularization parameter $\gamma_0 > 0$.
    pub gamma: f64,
    pub step_count: u64,
    /// Current primal weights $w_t \in \Delta_K$.
    pub current_weights: Vec<f64>,
}

impl RegularizedDualAveragingHedger {
    pub fn new(
        dimension: usize,
        anchor_target: Vec<f64>,
        lambda_turnover: f64,
        gamma: f64,
    ) -> Self {
        let dim = dimension.max(1);
        let mut anchor = anchor_target;
        if anchor.len() != dim {
            anchor = vec![1.0 / (dim as f64); dim];
        }
        let current_weights = anchor.clone();
        Self {
            dimension: dim,
            dual_gradient_sum: vec![0.0; dim],
            anchor_target: anchor,
            lambda_turnover: lambda_turnover.max(0.0),
            gamma: gamma.max(1e-4),
            step_count: 0,
            current_weights,
        }
    }

    /// Updates the dual average $\bar{g}_t = \frac{1}{t} \sum_{s=1}^t g_s$ and computes the
    /// $\ell_1$-soft-thresholded primal solution on the simplex:
    /// $$w_{t+1, i} \propto \operatorname{soft\_threshold}\left( w_{0, i} - \frac{\bar{G}_{t, i}}{\gamma \sqrt{t}}, \, \frac{\lambda}{\gamma \sqrt{t}} \right)$$
    pub fn update(&mut self, loss_gradient: &[f64]) {
        if loss_gradient.len() != self.dimension {
            return;
        }
        self.step_count += 1;
        let t = self.step_count as f64;
        let scale = self.gamma * t.sqrt();
        let threshold = self.lambda_turnover / scale;

        for (g_sum, &g) in self.dual_gradient_sum.iter_mut().zip(loss_gradient.iter()) {
            *g_sum += g;
        }

        let mut unnormalized = Vec::with_capacity(self.dimension);
        let mut sum_pos = 0.0;

        for (i, &w0) in self.anchor_target.iter().enumerate() {
            let unpenalized = w0 - self.dual_gradient_sum[i] / scale;
            let diff = unpenalized - w0;

            // Soft-thresholding around anchor point w_0:
            let shrunk_diff = if diff > threshold {
                diff - threshold
            } else if diff < -threshold {
                diff + threshold
            } else {
                0.0 // Zero turnover: stay exactly at anchor weight!
            };

            let val = (w0 + shrunk_diff).max(0.0);
            sum_pos += val;
            unnormalized.push(val);
        }

        if sum_pos > 1e-12 {
            for (w, val) in self.current_weights.iter_mut().zip(unnormalized) {
                *w = val / sum_pos;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_omd_converges_to_minimum_loss_arm() {
        // 3 hedging instruments: Instrument 1 has consistently lower tracking error (loss = 0.1)
        // Instruments 0 and 2 have higher tracking errors (loss = 0.8)
        let mut hedger = OnlineMirrorDescentHedger::new(3, 0.5);

        for _ in 0..100 {
            hedger.update_entropic(&[0.8, 0.1, 0.8]);
        }

        // Instrument 1 should receive > 95% of the portfolio weight
        assert!(
            hedger.weights[1] > 0.95,
            "OMD should concentrate on lowest-loss arm, got w={:?}",
            hedger.weights
        );
        let sum: f64 = hedger.weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_rda_suppresses_churn_below_turnover_threshold() {
        // High turnover penalty lambda = 5.0: small noise gradients should be soft-thresholded to 0
        let anchor = vec![0.5, 0.5];
        let mut rda = RegularizedDualAveragingHedger::new(2, anchor.clone(), 5.0, 1.0);

        // Small gradient oscillation (+0.02, -0.02)
        for _ in 0..10 {
            rda.update(&[0.02, -0.02]);
        }

        // Weights must remain exactly at anchor [0.5, 0.5] due to soft-thresholding
        assert!((rda.current_weights[0] - 0.5).abs() < 1e-9);
        assert!((rda.current_weights[1] - 0.5).abs() < 1e-9);
    }
}
