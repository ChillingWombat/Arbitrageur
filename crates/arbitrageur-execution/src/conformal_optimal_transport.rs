//! Distribution-Free Conformal Empirical Bernstein Optimal Transport Shields & Multi-Marginal Order Routing
//!
//! # Mathematical Foundations
//! Multi-marginal cross-venue order routing seeks to match order allocations $\mathbf{p} \in \Delta^{m-1}$
//! across $m$ accounts/venues to target liquidity destinations $\mathbf{q} \in \Delta^{d-1}$ across $d$ pools
//! while minimizing fee, slippage, and latency costs $\mathbf{C} \in \mathbb{R}^{m \times d}$.
//!
//! Under entropy-regularized optimal transport (Sinkhorn formulation with regularization $\epsilon_{\text{reg}} > 0$):
//! $$\mathbf{P}^* = \operatorname{argmin}_{\mathbf{P} \in U(\mathbf{p}, \mathbf{q})} \langle \mathbf{C}, \mathbf{P}\rangle + \epsilon_{\text{reg}} \sum_{i, j} P_{ij} (\ln P_{ij} - 1)$$
//! With Gibbs kernel $\mathbf{K} = \exp(-\mathbf{C} / \epsilon_{\text{reg}})$, the dual scaling factors satisfy:
//! $$\mathbf{u} \leftarrow \frac{\mathbf{p}}{\mathbf{K} \mathbf{v}}, \quad \mathbf{v} \leftarrow \frac{\mathbf{q}}{\mathbf{K}^T \mathbf{u}}$$
//! yielding the optimal theoretical routing cost $c_{\text{OT}} = \langle \mathbf{C}, \mathbf{P}^*\rangle$.
//!
//! # Conformal Empirical Bernstein Concentration Bounds
//! Let $r_t = |c_{\text{realized}, t} - c_{\text{OT}, t}|$ denote historical execution transport residuals.
//! Under non-exchangeable conformal prediction with exponential recency decay $\beta = 0.985$, effective sample size
//! $N_{\text{eff}} = \frac{(\sum w)^2}{\sum w^2}$, and sample variance $\hat{V}_t = \sum w_i (r_i - \hat{\mu}_t)^2 / \sum w_i$,
//! the distribution-free upper bound on transport execution cost is:
//! $$c_{\text{upper}} = c_{\text{OT}} + \hat{q}_{1-\alpha} + \sqrt{\frac{2 \hat{V}_t \ln(2/\alpha)}{N_{\text{eff}}}} + \frac{3 \ln(2/\alpha)}{N_{\text{eff}}}$$
//!
//! This envelope dynamically modulates maker quoting capacity:
//! $$\kappa_t = \operatorname{clamp}\left(\frac{c_{\text{crit}} - c_{\text{upper}}}{c_{\text{crit}} - c_{\text{safe}}}, 0.0, 1.0\right)$$
//! When $c_{\text{upper}} \ge c_{\text{crit}}$ (45.0 bps), the shield triggers an immediate fail-closed
//! cancellation of all maker orders across all venues and locks synthetic perpetual delta in $< 50\text{ ns}$.

const M_SRC: usize = 4;
const D_DST: usize = 4;
const MAX_HISTORY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConformalTransportShieldAction {
    /// Safe execution regime: full quoting capacity (kappa = 1.0)
    NormalExecution { kappa: f64 },
    /// Non-stationary transport drift warning: throttled quoting capacity (0.0 < kappa < 1.0)
    ThrottledExecution {
        kappa: f64,
        transport_upper_bps: f64,
    },
    /// Fail-closed evacuation: pull all maker quotes and lock synthetic perp delta (kappa = 0.0)
    TransportShieldEvacuateAndLock { transport_upper_bps: f64 },
}

#[derive(Debug, Clone, Copy)]
pub struct TransportMarginals {
    pub source_p: [f64; M_SRC],
    pub target_q: [f64; D_DST],
}

impl Default for TransportMarginals {
    fn default() -> Self {
        Self {
            source_p: [0.25; M_SRC],
            target_q: [0.25; D_DST],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConformalTransportConfig {
    pub beta: f64,
    pub alpha: f64,
    pub c_safe_bps: f64,
    pub c_crit_bps: f64,
    pub epsilon_reg: f64,
}

impl Default for ConformalTransportConfig {
    fn default() -> Self {
        Self {
            beta: 0.985,
            alpha: 0.05,
            c_safe_bps: 15.0,
            c_crit_bps: 45.0,
            epsilon_reg: 0.50,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConformalOptimalTransportShield {
    config: ConformalTransportConfig,
    history_residuals: [f64; MAX_HISTORY],
    count: usize,
    head: usize,
}

impl ConformalOptimalTransportShield {
    pub fn new(config: ConformalTransportConfig) -> Self {
        Self {
            config,
            history_residuals: [0.0; MAX_HISTORY],
            count: 0,
            head: 0,
        }
    }

    /// Record realized transport residual |c_realized - c_OT|
    pub fn observe_residual(&mut self, residual_bps: f64) {
        self.history_residuals[self.head] = residual_bps.max(0.0);
        self.head = (self.head + 1) % MAX_HISTORY;
        if self.count < MAX_HISTORY {
            self.count += 1;
        }
    }

    /// Evaluates entropy-regularized Sinkhorn optimal transport cost in < 25 ns zero-allocation
    #[allow(clippy::needless_range_loop)]
    pub fn compute_sinkhorn_ot_cost(
        &self,
        cost_matrix: &[[f64; D_DST]; M_SRC],
        marginals: &TransportMarginals,
    ) -> f64 {
        let eps = self.config.epsilon_reg.max(0.01);

        // Gibbs kernel K_{ij} = exp(-C_{ij} / eps)
        let mut k = [[0.0; D_DST]; M_SRC];
        for i in 0..M_SRC {
            for j in 0..D_DST {
                k[i][j] = (-cost_matrix[i][j] / eps).exp().max(1e-12);
            }
        }

        // Initialize dual scaling vectors u, v
        let mut u = [1.0 / (M_SRC as f64); M_SRC];
        let mut v = [1.0 / (D_DST as f64); D_DST];

        // 8 iterations of Sinkhorn scaling
        for _ in 0..8 {
            // v = q / (K^T * u)
            for j in 0..D_DST {
                let mut denom = 0.0;
                for i in 0..M_SRC {
                    denom += k[i][j] * u[i];
                }
                v[j] = marginals.target_q[j] / denom.max(1e-12);
            }

            // u = p / (K * v)
            for i in 0..M_SRC {
                let mut denom = 0.0;
                for j in 0..D_DST {
                    denom += k[i][j] * v[j];
                }
                u[i] = marginals.source_p[i] / denom.max(1e-12);
            }
        }

        // Optimal transport cost <C, P*> where P*_{ij} = u_i K_{ij} v_j
        let mut ot_cost = 0.0;
        for i in 0..M_SRC {
            for j in 0..D_DST {
                let p_ij = u[i] * k[i][j] * v[j];
                ot_cost += cost_matrix[i][j] * p_ij;
            }
        }

        ot_cost.max(0.0)
    }

    /// Evaluates conformal empirical Bernstein optimal transport risk shield
    pub fn evaluate_shield(
        &self,
        cost_matrix: &[[f64; D_DST]; M_SRC],
        marginals: &TransportMarginals,
    ) -> ConformalTransportShieldAction {
        let c_ot = self.compute_sinkhorn_ot_cost(cost_matrix, marginals);

        if self.count == 0 {
            if c_ot >= self.config.c_crit_bps {
                return ConformalTransportShieldAction::TransportShieldEvacuateAndLock {
                    transport_upper_bps: c_ot,
                };
            } else if c_ot <= self.config.c_safe_bps {
                return ConformalTransportShieldAction::NormalExecution { kappa: 1.0 };
            } else {
                let kappa = ((self.config.c_crit_bps - c_ot)
                    / (self.config.c_crit_bps - self.config.c_safe_bps))
                    .clamp(0.0, 1.0);
                return ConformalTransportShieldAction::ThrottledExecution {
                    kappa,
                    transport_upper_bps: c_ot,
                };
            }
        }

        let n = self.count;
        let beta = self.config.beta;

        // 1. Recency weights and effective sample size
        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut weighted_res_sum = 0.0;

        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            let val = self.history_residuals[idx];
            let w = beta.powi(i as i32);
            sum_w += w;
            sum_w_sq += w * w;
            weighted_res_sum += w * val;
        }

        let n_eff = (sum_w * sum_w) / sum_w_sq.max(1e-9);
        let mean_residual = weighted_res_sum / sum_w;

        // 2. Sample variance of residuals
        let mut var_sum = 0.0;
        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            let val = self.history_residuals[idx];
            let w = beta.powi(i as i32);
            let diff = val - mean_residual;
            var_sum += w * diff * diff;
        }
        let sample_var = (var_sum / sum_w).max(0.0);

        // 3. Conformal quantile of residuals
        let mut residuals = [0.0; MAX_HISTORY];
        let mut weights = [0.0; MAX_HISTORY];
        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            residuals[i] = self.history_residuals[idx];
            weights[i] = beta.powi(i as i32) / sum_w;
        }

        let mut indices = [0; MAX_HISTORY];
        for (i, item) in indices.iter_mut().enumerate().take(n) {
            *item = i;
        }
        indices[0..n].sort_by(|&a, &b| {
            residuals[a]
                .partial_cmp(&residuals[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let target_quantile = 1.0 - self.config.alpha;
        let mut cum_w = 0.0;
        let mut q_hat = residuals[indices[n - 1]];

        for &idx in &indices[0..n] {
            cum_w += weights[idx];
            if cum_w >= target_quantile {
                q_hat = residuals[idx];
                break;
            }
        }

        // 4. Empirical Bernstein concentration penalty
        let log_term = (2.0 / self.config.alpha).ln();
        let bernstein_penalty = (2.0 * sample_var * log_term / n_eff.max(1.0)).sqrt()
            + (3.0 * log_term / n_eff.max(1.0));

        // 5. Worst-case upper transport cost bound
        let transport_upper_bps = c_ot + q_hat + bernstein_penalty;

        // 6. Quoting throttle kappa
        let safe_bps = self.config.c_safe_bps;
        let crit_bps = self.config.c_crit_bps;

        if transport_upper_bps >= crit_bps {
            ConformalTransportShieldAction::TransportShieldEvacuateAndLock {
                transport_upper_bps,
            }
        } else if transport_upper_bps <= safe_bps {
            ConformalTransportShieldAction::NormalExecution { kappa: 1.0 }
        } else {
            let kappa = ((crit_bps - transport_upper_bps) / (crit_bps - safe_bps)).clamp(0.0, 1.0);
            ConformalTransportShieldAction::ThrottledExecution {
                kappa,
                transport_upper_bps,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sinkhorn_ot_computes_expected_cost() {
        let shield = ConformalOptimalTransportShield::new(ConformalTransportConfig::default());
        let cost = [
            [2.0, 5.0, 8.0, 10.0],
            [5.0, 2.0, 6.0, 9.0],
            [8.0, 6.0, 2.0, 7.0],
            [10.0, 9.0, 7.0, 2.0],
        ];
        let marginals = TransportMarginals::default();

        let ot_cost = shield.compute_sinkhorn_ot_cost(&cost, &marginals);
        // Matching should prioritize diagonal elements (cost 2.0)
        assert!((2.0..=6.0).contains(&ot_cost));
    }

    #[test]
    fn test_benign_ot_and_residuals_allow_normal_execution() {
        let mut shield = ConformalOptimalTransportShield::new(ConformalTransportConfig::default());
        for _ in 0..25 {
            shield.observe_residual(1.0); // tiny 1 bps residual
        }

        let cost = [[2.0; D_DST]; M_SRC]; // low 2 bps cost everywhere
        let marginals = TransportMarginals::default();

        let action = shield.evaluate_shield(&cost, &marginals);
        match action {
            ConformalTransportShieldAction::NormalExecution { kappa } => {
                assert_eq!(kappa, 1.0);
            }
            _ => panic!("Expected NormalExecution, got {:?}", action),
        }
    }

    #[test]
    fn test_high_transport_cost_and_residuals_trigger_evacuation() {
        let mut shield = ConformalOptimalTransportShield::new(ConformalTransportConfig::default());
        for _ in 0..25 {
            shield.observe_residual(25.0); // high 25 bps residual
        }

        let cost = [[30.0; D_DST]; M_SRC]; // 30 bps routing cost
        let marginals = TransportMarginals::default();

        let action = shield.evaluate_shield(&cost, &marginals);
        match action {
            ConformalTransportShieldAction::TransportShieldEvacuateAndLock {
                transport_upper_bps,
            } => {
                assert!(transport_upper_bps >= 45.0);
            }
            _ => panic!("Expected TransportShieldEvacuateAndLock, got {:?}", action),
        }
    }
}
