//! Non-Linear Optimal Multi-Asset Order Routing with Market Impact Propagators & Real-Time Slippage Prediction.
//!
//! Provides production mathematical formulations for:
//! 1. **Power-Law Non-Linear Market Impact**:
//!    The empirical instantaneous price impact for trade size $v$ is concave:
//!    $$I(v) = \eta \cdot \text{sgn}(v) \cdot |v|^\alpha, \quad \alpha \in [0.5, 0.8]$$
//!    capturing the empirical square-root law of market impact ($\alpha = 0.5$) rather than naive linear models ($\alpha = 1.0$).
//! 2. **Multi-Asset Cross-Impact Propagator Matrix**:
//!    Models cross-asset liquidity spillover where aggressive flow in asset $j$ shifts prices in correlated asset $i$:
//!    $$D_i(t) = \sum_{j=1}^K \Psi_{ij} \sum_{k} G_j(t - t_{j,k}) \cdot \text{sgn}(q_{j,k}) |q_{j,k}|^{\alpha_j}$$
//!    where $\mathbf{\Psi} \in \mathbb{R}^{K \times K}$ is the cross-impact elasticity matrix and $G_j(\tau) = \Gamma_{0,j} (\tau + \tau_{0,j})^{-\gamma_j}$ is the Bouchaud power-law decay kernel.
//! 3. **Non-Linear Multi-Venue Smart Order Router (SOR)**:
//!    Solves optimal order splitting across heterogeneous venues subject to non-linear power-law impact, maker/taker fee tiers, and capacity constraints via projected gradient descent.
//! 4. **Composite Depth & Transient Slippage Predictor**:
//!    Integrates discrete order book level-2 queue depletion with continuous power-law transient impact.

use serde::{Deserialize, Serialize};

/// Power-law non-linear market impact model parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerLawImpactModel {
    /// Instantaneous impact scaling coefficient \eta > 0.
    pub eta: f64,
    /// Concavity exponent \alpha \in (0.1, 1.0], typically 0.5 (square-root law).
    pub alpha: f64,
}

impl Default for PowerLawImpactModel {
    fn default() -> Self {
        Self {
            eta: 0.05,
            alpha: 0.50, // Square-root law of market impact
        }
    }
}

impl PowerLawImpactModel {
    pub fn new(eta: f64, alpha: f64) -> Self {
        Self {
            eta: eta.max(1e-6),
            alpha: alpha.clamp(0.10, 1.0),
        }
    }

    /// Evaluates instantaneous impact: I(v) = \eta \cdot \text{sgn}(v) \cdot |v|^\alpha.
    pub fn instantaneous_impact(&self, volume: f64) -> f64 {
        if volume.abs() < 1e-12 {
            return 0.0;
        }
        self.eta * volume.signum() * volume.abs().powf(self.alpha)
    }

    /// Evaluates marginal price impact (derivative dI/dv):
    /// dI/dv = \eta \cdot \alpha \cdot |v|^{\alpha - 1}.
    pub fn marginal_impact(&self, volume: f64) -> f64 {
        let abs_v = volume.abs();
        if abs_v < 1e-6 {
            // Regularize near zero to avoid singular derivative when \alpha < 1
            return self.eta * self.alpha * (1e-6_f64).powf(self.alpha - 1.0);
        }
        self.eta * self.alpha * abs_v.powf(self.alpha - 1.0)
    }

    /// Computes total execution price impact cost:
    /// \int_0^v I(u) du = \frac{\eta}{\alpha + 1} |v|^{\alpha + 1}.
    pub fn total_execution_cost(&self, volume: f64) -> f64 {
        let abs_v = volume.abs();
        (self.eta / (self.alpha + 1.0)) * abs_v.powf(self.alpha + 1.0)
    }
}

/// Bouchaud power-law kernel parameters for decay of transient impact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImpactDecayKernel {
    pub gamma_decay: f64,
    pub gamma_0: f64,
    pub tau_0: f64,
}

impl Default for ImpactDecayKernel {
    fn default() -> Self {
        Self {
            gamma_decay: 0.50,
            gamma_0: 0.05,
            tau_0: 0.10,
        }
    }
}

impl ImpactDecayKernel {
    pub fn new(gamma_decay: f64, gamma_0: f64, tau_0: f64) -> Self {
        Self {
            gamma_decay: gamma_decay.clamp(0.05, 0.95),
            gamma_0: gamma_0.max(1e-6),
            tau_0: tau_0.max(1e-4),
        }
    }

    pub fn kernel_value(&self, tau_sec: f64) -> f64 {
        let t = tau_sec.max(0.0) + self.tau_0;
        self.gamma_0 * t.powf(-self.gamma_decay)
    }
}

/// Multi-asset cross-impact propagator modeling liquidity spillover across correlated markets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiAssetCrossImpactPropagator {
    /// Number of assets in the universe.
    pub num_assets: usize,
    /// Asset symbol labels.
    pub asset_symbols: Vec<String>,
    /// Cross-impact elasticity matrix \mathbf{\Psi} \in \mathbb{R}^{K \times K}.
    /// Element \Psi_{ij} specifies the impact of asset j flow on asset i price.
    pub cross_impact_matrix: Vec<Vec<f64>>,
    /// Individual asset impact models.
    pub impact_models: Vec<PowerLawImpactModel>,
    /// Individual asset decay kernels.
    pub decay_kernels: Vec<ImpactDecayKernel>,
}

impl MultiAssetCrossImpactPropagator {
    pub fn new(
        asset_symbols: Vec<String>,
        cross_impact_matrix: Vec<Vec<f64>>,
        impact_models: Vec<PowerLawImpactModel>,
        decay_kernels: Vec<ImpactDecayKernel>,
    ) -> Option<Self> {
        let k = asset_symbols.len();
        if k == 0
            || cross_impact_matrix.len() != k
            || impact_models.len() != k
            || decay_kernels.len() != k
        {
            return None;
        }
        for row in &cross_impact_matrix {
            if row.len() != k {
                return None;
            }
        }
        Some(Self {
            num_assets: k,
            asset_symbols,
            cross_impact_matrix,
            impact_models,
            decay_kernels,
        })
    }

    /// Creates a default diagonal multi-asset propagator where cross-impact is zero.
    pub fn default_diagonal(asset_symbols: Vec<String>) -> Self {
        let k = asset_symbols.len();
        let mut matrix = vec![vec![0.0; k]; k];
        for (i, row) in matrix.iter_mut().enumerate().take(k) {
            row[i] = 1.0;
        }
        let impact_models = vec![PowerLawImpactModel::default(); k];
        let decay_kernels = vec![ImpactDecayKernel::default(); k];
        Self {
            num_assets: k,
            asset_symbols,
            cross_impact_matrix: matrix,
            impact_models,
            decay_kernels,
        }
    }

    /// Computes cumulative price displacement vector \mathbf{D}(t) across all assets:
    /// D_i(t) = \sum_{j=1}^K \Psi_{ij} \sum_{k} G_j(t - t_{j,k}) \cdot \text{sgn}(q_{j,k}) |q_{j,k}|^{\alpha_j}.
    pub fn compute_cumulative_displacement(
        &self,
        past_trades_by_asset: &[Vec<(f64, f64)>], // (timestamp_sec, signed_quantity)
        current_time_sec: f64,
    ) -> Vec<f64> {
        if past_trades_by_asset.len() != self.num_assets {
            return vec![0.0; self.num_assets];
        }

        // First calculate single-asset transient impact for each asset j
        let mut single_impacts = vec![0.0; self.num_assets];
        for (j, trades) in past_trades_by_asset.iter().enumerate() {
            let kernel = &self.decay_kernels[j];
            let model = &self.impact_models[j];
            let mut asset_j_impact = 0.0;
            for &(t_k, q_k) in trades {
                if current_time_sec >= t_k {
                    let tau = current_time_sec - t_k;
                    let instantaneous = model.instantaneous_impact(q_k);
                    asset_j_impact += kernel.kernel_value(tau) * instantaneous;
                }
            }
            single_impacts[j] = asset_j_impact;
        }

        // Multiply by cross-impact matrix: D_i = \sum_j \Psi_{ij} I_j
        let mut displacements = vec![0.0; self.num_assets];
        for (i, disp) in displacements.iter_mut().enumerate().take(self.num_assets) {
            let mut sum_i = 0.0;
            for (j, &impact) in single_impacts.iter().enumerate().take(self.num_assets) {
                sum_i += self.cross_impact_matrix[i][j] * impact;
            }
            *disp = sum_i;
        }

        displacements
    }
}

/// Venue execution specification for non-linear smart order routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NonLinearVenueSpec {
    pub venue_id: String,
    pub impact_model: PowerLawImpactModel,
    pub fee_bps: f64,
    pub half_spread_bps: f64,
    pub max_capacity: f64,
}

impl NonLinearVenueSpec {
    pub fn new(
        venue_id: impl Into<String>,
        eta: f64,
        alpha: f64,
        fee_bps: f64,
        half_spread_bps: f64,
        max_capacity: f64,
    ) -> Self {
        Self {
            venue_id: venue_id.into(),
            impact_model: PowerLawImpactModel::new(eta, alpha),
            fee_bps: fee_bps.max(0.0),
            half_spread_bps: half_spread_bps.max(0.0),
            max_capacity: max_capacity.max(0.0),
        }
    }
}

/// Solves optimal multi-venue parent order allocation under non-linear power-law impact.
#[derive(Debug, Clone, Default)]
pub struct NonLinearMultiVenueSor;

impl NonLinearMultiVenueSor {
    /// Computes optimal order allocation vector \mathbf{q}^* = [q_1^*, \dots, q_M^*]
    /// minimizing total expected execution cost:
    /// \mathcal{L}(\mathbf{q}) = \sum_{m=1}^M \left[ \frac{\eta_m}{\alpha_m + 1} q_m^{\alpha_m + 1} + \left(\frac{\text{fee}_m + s_m}{10000}\right) q_m \right]
    /// subject to \sum_m q_m = Q_{\text{total}}, 0 \le q_m \le \text{cap}_m.
    pub fn split_order(total_quantity: f64, venues: &[NonLinearVenueSpec]) -> Vec<(String, f64)> {
        if venues.is_empty() || total_quantity <= 0.0 {
            return Vec::new();
        }
        let m = venues.len();
        if m == 1 {
            let alloc = total_quantity.min(venues[0].max_capacity);
            return vec![(venues[0].venue_id.clone(), alloc)];
        }

        // Initialize with equal or capacity-proportional split
        let total_cap: f64 = venues.iter().map(|v| v.max_capacity).sum();
        let mut q: Vec<f64> = venues
            .iter()
            .map(|v| {
                if total_cap > 1e-9 {
                    (v.max_capacity / total_cap * total_quantity).min(v.max_capacity)
                } else {
                    (total_quantity / m as f64).min(v.max_capacity)
                }
            })
            .collect();

        // Project onto simplex \sum q_m = Q
        Self::project_onto_bounded_simplex(&mut q, total_quantity, venues);

        // Projected gradient descent with Barzilai-Borwein / line search step
        let max_iters = 50;
        let tolerance = 1e-6;

        for _ in 0..max_iters {
            // Compute gradient g_m = \eta_m q_m^{\alpha_m} + \frac{\text{fee}_m + s_m}{10000}
            let mut grad = vec![0.0; m];
            for i in 0..m {
                let v = &venues[i];
                let friction = (v.fee_bps + v.half_spread_bps) * 1e-4;
                grad[i] = v.impact_model.instantaneous_impact(q[i]) + friction;
            }

            // Find direction of steepest descent on simplex:
            // Find coordinate with maximum gradient and coordinate with minimum gradient
            let mut max_idx = 0;
            let mut min_idx = 0;
            for i in 1..m {
                if grad[i] > grad[max_idx] && q[i] > 1e-6 {
                    max_idx = i;
                }
                if grad[i] < grad[min_idx] && q[i] < venues[i].max_capacity - 1e-6 {
                    min_idx = i;
                }
            }

            let grad_diff = grad[max_idx] - grad[min_idx];
            if grad_diff < tolerance {
                break; // Converged
            }

            // Transfer amount from max_idx to min_idx
            let max_transfer = (q[max_idx]).min(venues[min_idx].max_capacity - q[min_idx]);
            if max_transfer <= 1e-9 {
                break;
            }

            // Step size using second derivative (Hessian approximation)
            let h_max = venues[max_idx].impact_model.marginal_impact(q[max_idx]);
            let h_min = venues[min_idx].impact_model.marginal_impact(q[min_idx]);
            let hessian_sum = (h_max + h_min).max(1e-6);

            let step = (grad_diff / hessian_sum).clamp(0.0, max_transfer);
            q[max_idx] -= step;
            q[min_idx] += step;
        }

        venues
            .iter()
            .enumerate()
            .map(|(i, v)| (v.venue_id.clone(), q[i]))
            .collect()
    }

    /// Projects vector \mathbf{q} onto bounded simplex \sum_m q_m = Q, 0 \le q_m \le \text{cap}_m.
    fn project_onto_bounded_simplex(q: &mut [f64], total: f64, venues: &[NonLinearVenueSpec]) {
        let m = q.len();
        let mut current_sum: f64 = q.iter().sum();
        if (current_sum - total).abs() < 1e-9 {
            return;
        }

        // Iterative clipping adjustment
        for _ in 0..20 {
            let diff = total - current_sum;
            if diff.abs() < 1e-9 {
                break;
            }

            let step = diff / m as f64;
            for (i, val) in q.iter_mut().enumerate() {
                *val = (*val + step).clamp(0.0, venues[i].max_capacity);
            }
            current_sum = q.iter().sum();
        }
    }
}

/// Composite slippage report combining book depth walk and non-linear transient impact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositeSlippageReport {
    /// Nominal order quantity.
    pub order_qty: f64,
    /// Discrete book swept average fill price.
    pub book_avg_price: f64,
    /// Discrete book slippage in basis points relative to mid.
    pub book_slippage_bps: f64,
    /// Power-law non-linear transient impact in basis points.
    pub transient_impact_bps: f64,
    /// Total expected slippage in basis points.
    pub total_expected_slippage_bps: f64,
    /// Recommended protective limit price.
    pub protective_limit_price: f64,
}

/// Real-time depth and transient slippage predictor.
#[derive(Debug, Clone, Default)]
pub struct PredictedDepthSlippage;

impl PredictedDepthSlippage {
    /// Computes composite slippage from L2 order book levels and power-law impact:
    /// - Discrete book walk: integrates available liquidity levels [(price, size)].
    /// - Continuous power-law impact: I(Q) = \eta Q^\alpha.
    /// - Protective limit: Mid \cdot (1 \pm \text{Slippage}_{\text{total}}).
    pub fn compute_composite_slippage(
        order_qty: f64,
        is_buy: bool,
        book_levels: &[(f64, f64)], // (price, size), sorted best to worst
        impact_model: &PowerLawImpactModel,
        mid_price: f64,
    ) -> Option<CompositeSlippageReport> {
        if order_qty <= 0.0 || book_levels.is_empty() || mid_price <= 0.0 {
            return None;
        }

        let mut remaining = order_qty;
        let mut total_cost = 0.0;
        let mut filled_qty = 0.0;

        for &(price, size) in book_levels {
            if remaining <= 0.0 {
                break;
            }
            let fill = remaining.min(size);
            total_cost += fill * price;
            filled_qty += fill;
            remaining -= fill;
        }

        if filled_qty < 1e-9 {
            return None;
        }

        let book_avg_price = total_cost / filled_qty;
        let book_slippage_bps = if is_buy {
            ((book_avg_price - mid_price) / mid_price).max(0.0) * 10_000.0
        } else {
            ((mid_price - book_avg_price) / mid_price).max(0.0) * 10_000.0
        };

        // Power-law transient impact in bps
        let raw_impact = impact_model.instantaneous_impact(order_qty);
        let transient_impact_bps = (raw_impact / mid_price).abs() * 10_000.0;

        let total_expected_slippage_bps = book_slippage_bps + transient_impact_bps;

        let protective_limit_price = if is_buy {
            mid_price * (1.0 + total_expected_slippage_bps * 1e-4 * 1.25)
        } else {
            mid_price * (1.0 - total_expected_slippage_bps * 1e-4 * 1.25)
        };

        Some(CompositeSlippageReport {
            order_qty,
            book_avg_price,
            book_slippage_bps,
            transient_impact_bps,
            total_expected_slippage_bps,
            protective_limit_price,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_law_impact_concavity() {
        let model = PowerLawImpactModel::new(0.04, 0.50); // Square root law
        let i1 = model.instantaneous_impact(1.0);
        let i4 = model.instantaneous_impact(4.0);
        let i9 = model.instantaneous_impact(9.0);

        // In square root law: I(4) = 2 * I(1), I(9) = 3 * I(1)
        assert!((i4 - 2.0 * i1).abs() < 1e-6);
        assert!((i9 - 3.0 * i1).abs() < 1e-6);

        // Derivative dI/dv should strictly decrease with volume (concavity)
        let m1 = model.marginal_impact(1.0);
        let m4 = model.marginal_impact(4.0);
        assert!(m1 > m4);

        // Total cost integral
        let cost = model.total_execution_cost(4.0);
        // \int_0^4 0.04 * u^0.5 du = 0.04 * (2/3) * 4^1.5 = 0.04 * (2/3) * 8 = 0.213333
        assert!((cost - (0.04 / 1.5) * 8.0).abs() < 1e-6);
    }

    #[test]
    fn test_multi_asset_cross_impact() {
        let symbols = vec!["BTC".to_string(), "ETH".to_string()];
        // BTC impact spills over to ETH by 0.35, ETH spills to BTC by 0.15
        let matrix = vec![vec![1.0, 0.15], vec![0.35, 1.0]];
        let impact_models = vec![
            PowerLawImpactModel::new(0.05, 0.5),
            PowerLawImpactModel::new(0.08, 0.5),
        ];
        let decay_kernels = vec![
            ImpactDecayKernel::new(0.5, 0.1, 0.05),
            ImpactDecayKernel::new(0.5, 0.1, 0.05),
        ];

        let propagator =
            MultiAssetCrossImpactPropagator::new(symbols, matrix, impact_models, decay_kernels)
                .expect("propagator");

        // Trade 100 BTC at t=1.0, 0 ETH trades
        let past_trades = vec![
            vec![(1.0, 100.0)], // BTC trades
            vec![],             // ETH trades
        ];

        let disp = propagator.compute_cumulative_displacement(&past_trades, 2.0);
        assert_eq!(disp.len(), 2);
        // BTC displacement > 0
        assert!(disp[0] > 0.0);
        // ETH displacement > 0 due to cross-impact spillover
        assert!(disp[1] > 0.0);
        // ETH displacement should be 0.35 * BTC direct displacement
        assert!((disp[1] - 0.35 * disp[0]).abs() < 1e-6);
    }

    #[test]
    fn test_non_linear_sor_split() {
        let venues = vec![
            NonLinearVenueSpec::new("binance", 0.02, 0.5, 2.0, 1.0, 100.0),
            NonLinearVenueSpec::new("hyperliquid", 0.03, 0.5, 1.0, 1.5, 100.0),
            NonLinearVenueSpec::new("lighter", 0.04, 0.5, 0.5, 2.0, 100.0),
        ];

        let splits = NonLinearMultiVenueSor::split_order(60.0, &venues);
        assert_eq!(splits.len(), 3);
        let total_split: f64 = splits.iter().map(|(_, q)| q).sum();
        assert!((total_split - 60.0).abs() < 1e-4);

        // Venue with lower impact coefficient and fees should get substantial allocation
        for (v, q) in splits {
            assert!(q > 0.0, "Venue {} should receive positive split", v);
            assert!(q <= 100.0, "Venue {} should not breach capacity", v);
        }
    }

    #[test]
    fn test_composite_depth_slippage() {
        let levels = vec![(100.0, 10.0), (100.2, 20.0), (100.5, 30.0), (101.0, 50.0)];
        let model = PowerLawImpactModel::new(0.1, 0.5);
        let mid = 100.0;

        let report = PredictedDepthSlippage::compute_composite_slippage(
            25.0, true, // Buy
            &levels, &model, mid,
        )
        .expect("report");

        assert_eq!(report.order_qty, 25.0);
        assert!(report.book_avg_price > 100.0);
        assert!(report.book_slippage_bps > 0.0);
        assert!(report.transient_impact_bps > 0.0);
        assert!(report.total_expected_slippage_bps > report.book_slippage_bps);
        assert!(report.protective_limit_price > report.book_avg_price);
    }
}
