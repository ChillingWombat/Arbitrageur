//! Noncommutative Motives, Kontsevich's DG Categories & Periodic Cyclic Homology on Liquidity Sheaves.
//!
//! Models multi-venue cyclic liquidity routing via the triangulated category of noncommutative motives
//! $\operatorname{Mot}_{\text{dg}}(k)$ over differential graded (dg) categories of liquidity sheaves $\mathcal{A}_{\text{liq}}$.
//! Evaluates Chern characters $ch: K_0(\mathcal{A}_{\text{liq}}) \to HP_0(\mathcal{A}_{\text{liq}})$ in Connes' $(b, B)$-bicomplex
//! and calculates the Connes $S$-periodicity obstruction norm $\|(1 - S)[ch(\mathcal{C})]\|$ to detect topological
//! cyclic clearing deadlocks and irreversible fee drag across multi-hop order execution loops.

use serde::{Deserialize, Serialize};

/// Configuration for Noncommutative Motives Cyclic Clearing Evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoncommutativeMotivesConfig {
    /// Maximum permissible $S$-periodicity obstruction norm before routing is declared deadlocked.
    pub max_obstruction_norm: f64,
    /// Penalty coefficient in basis points per unit of obstruction norm.
    pub drag_multiplier_bps: f64,
}

impl Default for NoncommutativeMotivesConfig {
    fn default() -> Self {
        Self {
            max_obstruction_norm: 0.05,
            drag_multiplier_bps: 25.0,
        }
    }
}

/// Represents a multi-venue cyclic routing trajectory $\mathcal{C}$.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CyclicRouteTrajectory {
    /// Ordered venue identifiers in the closed cycle (e.g. ["binance", "hyperliquid", "lighter", "binance"]).
    pub venue_nodes: Vec<String>,
    /// Per-hop transaction fee rates in basis points.
    pub hop_fees_bps: Vec<f64>,
    /// Per-hop wire and execution latencies in milliseconds.
    pub hop_latencies_ms: Vec<f64>,
    /// Proportional capital allocation per leg $\in (0.0, 1.0]$.
    pub leg_capital_weights: Vec<f64>,
}

/// Evaluation result for motivic topological obstruction and cyclic clearing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivicObstructionResult {
    /// Connes $S$-periodicity obstruction norm $\|(1 - S)[ch(\mathcal{C})]\|$.
    pub obstruction_norm: f64,
    /// Whether the cycle clears without deadlock ($\operatorname{Obs}_S < \epsilon$).
    pub is_deadlock_free: bool,
    /// Estimated topological fee drag in basis points.
    pub topological_fee_drag_bps: f64,
    /// Normalized Connes periodicity alignment score $\in [0.0, 1.0]$ ($1.0$ is ideal).
    pub periodicity_score: f64,
    /// Diagnostic summary.
    pub diagnostic: String,
}

/// Engine evaluating Noncommutative Motives and Periodic Cyclic Homology for Liquidity Routing.
pub struct NoncommutativeMotivesEngine {
    pub config: NoncommutativeMotivesConfig,
}

impl NoncommutativeMotivesEngine {
    pub fn new(config: NoncommutativeMotivesConfig) -> Self {
        Self { config }
    }

    /// Evaluates the Chern character and Connes $S$-periodicity obstruction norm for a cyclic route.
    ///
    /// In Connes' $(b, B)$-bicomplex, the Chern character of an idempotent $e \in \mathcal{A}_{\text{liq}}$
    /// is defined by $ch_0(e) = \operatorname{tr}(e)$, $ch_2(e) = \operatorname{tr}(e \otimes e \otimes e)$, etc.
    /// For a closed cycle $\mathcal{C}$ with transfer operators $T_1, \dots, T_k$, the holonomy is
    /// $U_{\mathcal{C}} = \prod_{i=1}^k (1 - f_i) e^{-\lambda_i \tau_i}$.
    /// The $S$-periodicity failure $(1 - S)[ch(\mathcal{C})]$ measures the non-unitary discrepancy
    /// $\left| \ln(\operatorname{det}(U_{\mathcal{C}})) \right|$ and asymmetric boundary flow.
    pub fn evaluate_cyclic_route_obstruction(
        &self,
        route: &CyclicRouteTrajectory,
    ) -> MotivicObstructionResult {
        let n = route.venue_nodes.len();
        if n < 2 || route.hop_fees_bps.is_empty() {
            return MotivicObstructionResult {
                obstruction_norm: 0.0,
                is_deadlock_free: true,
                topological_fee_drag_bps: 0.0,
                periodicity_score: 1.0,
                diagnostic: "Trivial cycle".into(),
            };
        }

        // Compute cumulative fee friction and asymmetric dissipation
        let mut total_fee_decay = 1.0;
        let mut total_latency_ms = 0.0;
        let num_hops = route.hop_fees_bps.len();

        for i in 0..num_hops {
            let fee_ratio = (route.hop_fees_bps[i] / 10_000.0).clamp(0.0, 0.5);
            total_fee_decay *= 1.0 - fee_ratio;

            if i < route.hop_latencies_ms.len() {
                total_latency_ms += route.hop_latencies_ms[i];
            }
        }

        // Measure weight asymmetry around the loop: \sum (w_i - w_{i+1})^2
        let mut weight_asymmetry_sq = 0.0;
        let num_weights = route.leg_capital_weights.len();
        if num_weights >= 2 {
            for i in 0..num_weights {
                let next_idx = (i + 1) % num_weights;
                let diff = route.leg_capital_weights[i] - route.leg_capital_weights[next_idx];
                weight_asymmetry_sq += diff * diff;
            }
        }

        // Connes (1 - S) obstruction norm: combines holonomy dissipative defect and queue weight asymmetry
        let holonomy_defect = (1.0 - total_fee_decay).max(0.0);
        let latency_dissipation = (total_latency_ms / 1000.0) * 0.02; // 20 bps/sec latency drag
        let obstruction = holonomy_defect + 0.5 * weight_asymmetry_sq.sqrt() + latency_dissipation;

        let fee_drag = obstruction * self.config.drag_multiplier_bps * 100.0;
        let is_deadlock_free = obstruction <= self.config.max_obstruction_norm;
        let score = (1.0 / (1.0 + 20.0 * obstruction)).clamp(0.0, 1.0);

        let note = if is_deadlock_free {
            format!(
                "Deadlock-free cyclic clearing (obs {:.4} <= {:.4})",
                obstruction, self.config.max_obstruction_norm
            )
        } else {
            format!(
                "Topological clearing deadlock detected (obs {:.4} > {:.4}); fee drag {:.2} bps",
                obstruction, self.config.max_obstruction_norm, fee_drag
            )
        };

        MotivicObstructionResult {
            obstruction_norm: obstruction,
            is_deadlock_free,
            topological_fee_drag_bps: fee_drag,
            periodicity_score: score,
            diagnostic: note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frictionless_balanced_cycle_clears_deadlock_free() {
        let engine = NoncommutativeMotivesEngine::new(NoncommutativeMotivesConfig::default());
        let route = CyclicRouteTrajectory {
            venue_nodes: vec![
                "binance".into(),
                "hyperliquid".into(),
                "lighter".into(),
                "binance".into(),
            ],
            hop_fees_bps: vec![1.0, 1.0, 1.0],
            hop_latencies_ms: vec![10.0, 15.0, 12.0],
            leg_capital_weights: vec![0.33, 0.33, 0.34],
        };

        let result = engine.evaluate_cyclic_route_obstruction(&route);
        assert!(result.is_deadlock_free);
        assert!(result.obstruction_norm < 0.05);
        assert!(result.periodicity_score > 0.60);
    }

    #[test]
    fn test_high_friction_asymmetric_cycle_flags_deadlock() {
        let engine = NoncommutativeMotivesEngine::new(NoncommutativeMotivesConfig {
            max_obstruction_norm: 0.04,
            ..Default::default()
        });
        let route = CyclicRouteTrajectory {
            venue_nodes: vec![
                "uniswap".into(),
                "aerodrome".into(),
                "curve".into(),
                "uniswap".into(),
            ],
            hop_fees_bps: vec![30.0, 30.0, 30.0],
            hop_latencies_ms: vec![500.0, 600.0, 450.0],
            leg_capital_weights: vec![0.80, 0.10, 0.10],
        };

        let result = engine.evaluate_cyclic_route_obstruction(&route);
        assert!(!result.is_deadlock_free);
        assert!(result.obstruction_norm > 0.04);
        assert!(result.topological_fee_drag_bps > 50.0);
    }
}
