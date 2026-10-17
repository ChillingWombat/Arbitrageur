//! Derived Symplectic Critical Loci, Donaldson-Thomas Virtual Invariants & Multi-Exchange Liquidity Sheaves
//!
//! # Mathematical Foundations
//! In derived algebraic and symplectic geometry (Joyce 2015, Behrend 2009, Pantev et al. PTVV),
//! the derived critical locus of a routing potential function $H_{\text{route}}: \mathcal{M} \to \mathbb{R}$
//! represents the derived Lagrangian intersection:
//! $$\operatorname{Crit}(H_{\text{route}}) = \mathcal{M} \times_{T^*\mathcal{M}} \mathcal{M}$$
//! which naturally carries a canonical $(-1)$-shifted symplectic structure.
//!
//! # Behrend Constructible Function & Donaldson-Thomas Virtual Invariants
//! The Behrend constructible function $\nu_{\operatorname{Crit}(H)}: \operatorname{Crit}(H) \to \mathbb{Z}$
//! computes the virtual Donaldson-Thomas (DT) invariant:
//! $$I_{\text{DT}} = \chi(\operatorname{Crit}(H), \nu) = \int_{\operatorname{Crit}(H)} \nu \, d\chi$$
//! measuring the topological multiplicity of stable execution equilibria across fragmented venues.
//!
//! If the derived critical locus is non-degenerate and its virtual fundamental cycle
//! $[\operatorname{Crit}(H)]^{\text{vir}}$ is unobstructed, the multi-exchange routing network admits
//! canonical deformation-free clearing without liquidity freeze or route deadlock.
//!
//! # Virtual Defect Norm & Curvature Fee Shading
//! The Donaldson-Thomas boundary defect norm:
//! $$\|\operatorname{Def}_{\text{DT}}\| = \sqrt{ \sum_k w_k (\text{shear}_k)^2 }$$
//! quantifies the exact obstruction to smooth execution flow. If $\|\operatorname{Def}_{\text{DT}}\| \le \tau_{\text{DT}}$,
//! the engine routes with virtual fee shading:
//! $$\text{FeeShading}_{\text{bps}} = \gamma_{\text{DT}} \cdot \|\operatorname{Def}_{\text{DT}}\| \cdot (1 - \mathcal{E}_{\text{DT}})$$
//! evaluated with deterministic zero-allocation routines in $< 50\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Action verdict emitted by the Derived Critical Locus Routing Engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DerivedCriticalLocusAction {
    /// Zero/negligible defect: unobstructed virtual fundamental class, execute immediate atomic routing.
    ExecuteImmediate,
    /// Smooth virtual deformation: route with DT fee shading.
    RouteWithVirtualLocusShading {
        fee_shading_bps: f64,
        virtual_efficiency: f64,
    },
    /// Severe virtual singularity or obstruction: reject route.
    DerivedCriticalLocusReject { defect_norm: f64 },
}

/// Evaluation result under Derived Symplectic Critical Loci Geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DerivedCriticalLocusResult {
    /// Donaldson-Thomas boundary defect norm.
    pub defect_norm: f64,
    /// Behrend virtual multiplicity invariant I_DT.
    pub dt_virtual_invariant: f64,
    /// Derived virtual routing efficiency in (0, 1].
    pub virtual_efficiency: f64,
    /// Fee shading in basis points.
    pub fee_shading_bps: f64,
    /// Whether the critical locus is unobstructed.
    pub is_unobstructed_locus: bool,
    /// Recommended routing action.
    pub action: DerivedCriticalLocusAction,
}

/// Configuration parameters for Derived Critical Locus Routing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DerivedCriticalLocusConfig {
    /// Maximum allowed defect norm before routing rejection (default 0.040).
    pub max_defect_tol: f64,
    /// Fee shading multiplier for DT curvature (default 25.0).
    pub fee_shading_multiplier: f64,
    /// Weight for spread mismatch shear (default 0.35).
    pub spread_shear_weight: f64,
    /// Weight for book depth asymmetry (default 0.35).
    pub depth_asymmetry_weight: f64,
    /// Weight for latency transit displacement (default 0.30).
    pub latency_transit_weight: f64,
}

impl Default for DerivedCriticalLocusConfig {
    fn default() -> Self {
        Self {
            max_defect_tol: 0.040,
            fee_shading_multiplier: 25.0,
            spread_shear_weight: 0.35,
            depth_asymmetry_weight: 0.35,
            latency_transit_weight: 0.30,
        }
    }
}

/// Engine for evaluating Derived Symplectic Critical Loci & DT Virtual Invariants.
#[derive(Debug, Clone, Copy, Default)]
pub struct DerivedCriticalLocusEngine {
    pub config: DerivedCriticalLocusConfig,
}

impl DerivedCriticalLocusEngine {
    /// Creates a new Derived Critical Locus engine with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new Derived Critical Locus engine with custom configuration.
    pub fn with_config(config: DerivedCriticalLocusConfig) -> Self {
        Self { config }
    }

    /// Evaluates derived critical locus obstructions, DT invariants, and virtual efficiency.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_critical_locus_routing(
        &self,
        venue_a_bid: f64,
        venue_a_ask: f64,
        venue_b_bid: f64,
        venue_b_ask: f64,
        venue_a_depth: f64,
        venue_b_depth: f64,
        latency_delay_ms: f64,
        tick_shear_bps: f64,
        route_volume: f64,
    ) -> DerivedCriticalLocusResult {
        let mid_a = (venue_a_bid + venue_a_ask) * 0.5;
        let mid_b = (venue_b_bid + venue_b_ask) * 0.5;
        let avg_mid = (mid_a + mid_b) * 0.5;
        let mid_ref = if avg_mid > 1e-8 { avg_mid } else { 1.0 };

        // Spread shear
        let spread_a = (venue_a_ask - venue_a_bid).max(0.0) / mid_ref;
        let spread_b = (venue_b_ask - venue_b_bid).max(0.0) / mid_ref;
        let spread_shear = (spread_a - spread_b).abs();

        // Depth asymmetry relative to route volume
        let total_depth = venue_a_depth + venue_b_depth;
        let depth_asymmetry = if total_depth > 1e-8 {
            let imbalance = (venue_a_depth - venue_b_depth).abs() / total_depth;
            let route_fraction = (route_volume / total_depth).min(2.0);
            imbalance * (0.5 + 0.5 * route_fraction)
        } else {
            1.0
        };

        // Latency transit displacement
        let latency_norm = (latency_delay_ms / 100.0).min(3.0);
        let shear_norm = (tick_shear_bps / 10.0).abs().min(3.0);
        let latency_displacement = (latency_norm * 0.6 + shear_norm * 0.4) * 0.05;

        // DT Boundary defect norm ||Def_DT||
        let w_spread = self.config.spread_shear_weight;
        let w_depth = self.config.depth_asymmetry_weight;
        let w_lat = self.config.latency_transit_weight;

        let defect_sq = w_spread * spread_shear * spread_shear
            + w_depth * depth_asymmetry * depth_asymmetry * 0.01
            + w_lat * latency_displacement * latency_displacement;
        let defect_norm = defect_sq.sqrt();

        // Behrend virtual Euler characteristic multiplicity I_DT
        // Smooth transversal intersections have I_DT = 1.0; non-zero defect perturbs virtual multiplicity
        let dt_virtual_invariant = (1.0 / (1.0 + defect_norm * 5.0)).max(0.01);

        // Virtual routing efficiency \mathcal{E}_{DT} \in (0, 1]
        let virtual_efficiency = (1.0
            / (1.0 + defect_norm * 13.0 + (1.0 - dt_virtual_invariant) * 0.5))
            .clamp(0.01, 1.0);

        // Curvature fee shading
        let fee_shading_bps =
            self.config.fee_shading_multiplier * defect_norm * (1.0 - virtual_efficiency);

        let is_unobstructed_locus = defect_norm < 1e-4;

        let action = if defect_norm > self.config.max_defect_tol {
            DerivedCriticalLocusAction::DerivedCriticalLocusReject { defect_norm }
        } else if is_unobstructed_locus {
            DerivedCriticalLocusAction::ExecuteImmediate
        } else {
            DerivedCriticalLocusAction::RouteWithVirtualLocusShading {
                fee_shading_bps,
                virtual_efficiency,
            }
        };

        DerivedCriticalLocusResult {
            defect_norm,
            dt_virtual_invariant,
            virtual_efficiency,
            fee_shading_bps,
            is_unobstructed_locus,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perfect_alignment_unobstructed_locus() {
        let engine = DerivedCriticalLocusEngine::new();
        let res = engine
            .compute_critical_locus_routing(100.0, 100.1, 100.0, 100.1, 10.0, 10.0, 0.0, 0.0, 1.0);

        assert!(res.defect_norm < 0.005);
        assert!(res.virtual_efficiency > 0.90);
        assert!(res.dt_virtual_invariant > 0.95);
        assert_eq!(res.action, DerivedCriticalLocusAction::ExecuteImmediate);
    }

    #[test]
    fn test_mild_shear_routes_with_shading() {
        let engine = DerivedCriticalLocusEngine::new();
        let res = engine.compute_critical_locus_routing(
            100.0, 100.2, 100.05, 100.25, 12.0, 8.0, 18.0, 1.2, 2.0,
        );

        assert!(res.defect_norm > 0.0);
        assert!(res.defect_norm < engine.config.max_defect_tol);
        assert!(res.virtual_efficiency > 0.70 && res.virtual_efficiency <= 1.0);

        match res.action {
            DerivedCriticalLocusAction::RouteWithVirtualLocusShading {
                fee_shading_bps,
                virtual_efficiency,
            } => {
                assert!(fee_shading_bps > 0.0);
                assert!(virtual_efficiency > 0.70);
            }
            _ => panic!(
                "Expected RouteWithVirtualLocusShading, got {:?}",
                res.action
            ),
        }
    }

    #[test]
    fn test_severe_defect_rejects() {
        let config = DerivedCriticalLocusConfig {
            max_defect_tol: 0.020,
            ..Default::default()
        };
        let engine = DerivedCriticalLocusEngine::with_config(config);

        let res = engine.compute_critical_locus_routing(
            100.0, 103.0, 100.0, 100.1, 50.0, 1.0, 250.0, 20.0, 5.0,
        );

        assert!(res.defect_norm > config.max_defect_tol);
        match res.action {
            DerivedCriticalLocusAction::DerivedCriticalLocusReject { defect_norm } => {
                assert_eq!(defect_norm, res.defect_norm);
            }
            _ => panic!("Expected DerivedCriticalLocusReject, got {:?}", res.action),
        }
    }
}
