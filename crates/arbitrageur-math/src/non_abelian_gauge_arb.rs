//! Non-Abelian Gauge Fields & Holonomy Invariant Arbitrage on Curved Liquidity Manifolds.
//!
//! Models multi-asset conversion matrices (spot, perpetuals, dated futures, and collateral
//! margin haircuts) across fragmented venues as parallel transport operators $U_{uv} \in \text{GL}(k, \mathbb{R})$
//! on a principal fiber bundle over the market graph $G = (V, E)$:
//!
//! $$U_{uv} = \mathcal{P} \exp\left( \int_u^v \mathbf{A}_\mu dx^\mu \right)$$
//!
//! Unlike classical scalar (Abelian $\mathbb{R}^+$) exchange rate graphs where order of execution
//! commutes, multi-instrument conversions involving tiered margin haircuts, funding basis, and
//! option vega-delta cross-couplings are governed by non-Abelian matrix connections $\mathbf{A}_\mu \in \mathfrak{gl}(k, \mathbb{R})$.
//!
//! Computes:
//! 1. Non-Abelian Yang-Mills field strength (curvature commutator):
//!    $$F_{\mu\nu} = A_\mu A_\nu - A_\nu A_\mu$$
//! 2. Gauge-invariant Wilson loop trace $W(C) = \frac{1}{k} \operatorname{Tr}\left(\prod_{e \in C} U_e\right)$.
//! 3. Dominant holonomy eigenvalue yield $\mathcal{Y}(C) = \ln \lambda_{\max}\left(\prod_{e \in C} U_e\right)$,
//!    strictly invariant under local numéraire gauge transformations $U_{uv} \mapsto g_u U_{uv} g_v^{-1}$.

use serde::{Deserialize, Serialize};

/// A $2 \times 2$ non-Abelian parallel transport operator $U_{uv} \in \text{GL}(2, \mathbb{R})$
/// acting on a 2-component state vector $[\text{Spot Capital}, \text{Perp Margin Collateral}]^T$.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TransportMatrix2x2 {
    pub m00: f64,
    pub m01: f64,
    pub m10: f64,
    pub m11: f64,
}

impl TransportMatrix2x2 {
    /// Identity parallel transport matrix $\mathbf{I}_2$.
    pub fn identity() -> Self {
        Self {
            m00: 1.0,
            m01: 0.0,
            m10: 0.0,
            m11: 1.0,
        }
    }

    /// Constructs a parallel transport matrix from spot exchange rate $r_{\text{spot}}$,
    /// perp basis conversion rate $r_{\text{perp}}$, cross-collateral transfer coupling $\epsilon_{01}, \epsilon_{10}$,
    /// and transaction fee rate in basis points.
    pub fn from_market_transition(
        spot_rate: f64,
        perp_rate: f64,
        spot_to_perp_coupling: f64,
        perp_to_spot_coupling: f64,
        fee_bps: f64,
    ) -> Self {
        let fee_factor = (1.0 - fee_bps / 10_000.0).max(0.0);
        Self {
            m00: spot_rate * fee_factor,
            m01: perp_to_spot_coupling * fee_factor,
            m10: spot_to_perp_coupling * fee_factor,
            m11: perp_rate * fee_factor,
        }
    }

    /// Matrix multiplication $A \cdot B$ (non-commutative parallel transport composition).
    #[inline(always)]
    pub fn matmul(&self, rhs: &Self) -> Self {
        Self {
            m00: self.m00 * rhs.m00 + self.m01 * rhs.m10,
            m01: self.m00 * rhs.m01 + self.m01 * rhs.m11,
            m10: self.m10 * rhs.m00 + self.m11 * rhs.m10,
            m11: self.m10 * rhs.m01 + self.m11 * rhs.m11,
        }
    }

    /// Lie bracket (commutator) $[A, B] = A B - B A$ measuring non-Abelian curvature.
    #[inline(always)]
    pub fn commutator(&self, other: &Self) -> Self {
        let ab = self.matmul(other);
        let ba = other.matmul(self);
        Self {
            m00: ab.m00 - ba.m00,
            m01: ab.m01 - ba.m01,
            m10: ab.m10 - ba.m10,
            m11: ab.m11 - ba.m11,
        }
    }

    /// Frobenius norm $\|M\|_F = \sqrt{\sum_{i,j} M_{ij}^2}$.
    #[inline(always)]
    pub fn frobenius_norm(&self) -> f64 {
        (self.m00 * self.m00 + self.m01 * self.m01 + self.m10 * self.m10 + self.m11 * self.m11)
            .sqrt()
    }

    /// Matrix trace $\operatorname{Tr}(M) = M_{00} + M_{11}$.
    #[inline(always)]
    pub fn trace(&self) -> f64 {
        self.m00 + self.m11
    }

    /// Matrix determinant $\det(M) = M_{00} M_{11} - M_{01} M_{10}$.
    #[inline(always)]
    pub fn determinant(&self) -> f64 {
        self.m00 * self.m11 - self.m01 * self.m10
    }

    /// Matrix inverse $M^{-1}$ for gauge transformations $g^{-1}$.
    pub fn inverse(&self) -> Option<Self> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Self {
            m00: self.m11 * inv_det,
            m01: -self.m01 * inv_det,
            m10: -self.m10 * inv_det,
            m11: self.m00 * inv_det,
        })
    }

    /// Computes the dominant real eigenvalue magnitude $\lambda_{\max}(M)$.
    pub fn dominant_eigenvalue(&self) -> f64 {
        let tr = self.trace();
        let det = self.determinant();
        let disc = tr * tr - 4.0 * det;
        if disc >= 0.0 {
            let sqrt_disc = disc.sqrt();
            let l1 = 0.5 * (tr + sqrt_disc);
            let l2 = 0.5 * (tr - sqrt_disc);
            l1.max(l2)
        } else {
            // Complex conjugate eigenvalues: modulus is sqrt(det)
            det.abs().sqrt()
        }
    }

    /// Computes the normalized dominant eigenvector $[v_0, v_1]^T$ representing the optimal
    /// Spot vs Perp capital split entering the holonomy cycle.
    pub fn dominant_eigenvector(&self) -> [f64; 2] {
        let lambda = self.dominant_eigenvalue();
        // (M00 - lambda) * v0 + M01 * v1 = 0 => v0 = M01, v1 = lambda - M00
        let v0_raw = self.m01.abs();
        let v1_raw = (lambda - self.m00).abs();
        let sum = v0_raw + v1_raw;
        if sum < 1e-9 {
            [1.0, 0.0]
        } else {
            [v0_raw / sum, v1_raw / sum]
        }
    }
}

/// Directed gauge link connecting two venue/asset nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeLinkEdge {
    pub edge_id: String,
    pub from_node: usize,
    pub to_node: usize,
    pub transport: TransportMatrix2x2,
}

/// Evaluation of a closed holonomy plaquette loop $C$.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolonomyPlaquetteResult {
    pub cycle_nodes: Vec<usize>,
    /// Holonomy product matrix $H(C) = \prod_{e \in C} U_e$.
    pub holonomy_matrix: TransportMatrix2x2,
    /// Normalized Wilson loop trace $W(C) = \frac{1}{2} \operatorname{Tr}(H(C))$.
    pub wilson_loop_trace: f64,
    /// Dominant eigenvalue $\lambda_{\max}(H(C))$.
    pub dominant_eigenvalue: f64,
    /// Gauge-invariant net logarithmic arbitrage yield in basis points: $10^4 \ln(\lambda_{\max})$.
    pub net_yield_bps: f64,
    /// Optimal initial capital split $[w_{\text{spot}}, w_{\text{perp}}]$.
    pub optimal_capital_weights: [f64; 2],
    /// Non-Abelian curvature norm $\|[U_1, U_2]\|_F$ measuring order-of-execution sensitivity.
    pub non_abelian_curvature_norm: f64,
    /// True if profitable above hurdle threshold.
    pub is_profitable_arbitrage: bool,
}

/// Non-Abelian Gauge Field Arbitrage Engine.
pub struct NonAbelianGaugeArbitrageEngine {
    pub num_nodes: usize,
    pub links: Vec<GaugeLinkEdge>,
    pub min_yield_hurdle_bps: f64,
}

impl NonAbelianGaugeArbitrageEngine {
    pub fn new(num_nodes: usize, links: Vec<GaugeLinkEdge>, min_yield_hurdle_bps: f64) -> Self {
        Self {
            num_nodes,
            links,
            min_yield_hurdle_bps: min_yield_hurdle_bps.max(0.1),
        }
    }

    /// Evaluates the holonomy around a specified sequence of link indices forming a closed loop.
    pub fn evaluate_cycle(
        &self,
        link_indices: &[usize],
    ) -> Result<HolonomyPlaquetteResult, String> {
        if link_indices.is_empty() {
            return Err("Cycle cannot be empty".into());
        }

        let first_link = self
            .links
            .get(link_indices[0])
            .ok_or_else(|| "Invalid link index".to_string())?;
        let start_node = first_link.from_node;
        let mut curr_node = start_node;
        let mut cycle_nodes = vec![start_node];
        let mut holonomy = TransportMatrix2x2::identity();

        for &idx in link_indices {
            let link = self
                .links
                .get(idx)
                .ok_or_else(|| format!("Link index {} out of bounds", idx))?;
            if link.from_node != curr_node {
                return Err(format!(
                    "Discontinuous path: expected from_node {}, got {}",
                    curr_node, link.from_node
                ));
            }
            holonomy = holonomy.matmul(&link.transport);
            curr_node = link.to_node;
            cycle_nodes.push(curr_node);
        }

        if curr_node != start_node {
            return Err(format!(
                "Path is not a closed loop: started at {}, ended at {}",
                start_node, curr_node
            ));
        }

        let wilson_trace = 0.5 * holonomy.trace();
        let dom_eig = holonomy.dominant_eigenvalue();
        let net_yield_bps = if dom_eig > 0.0 {
            dom_eig.ln() * 10_000.0
        } else {
            -10_000.0
        };
        let weights = holonomy.dominant_eigenvector();

        // Compute non-Abelian curvature norm from first two links in the loop
        let curvature_norm = if link_indices.len() >= 2 {
            let u1 = &self.links[link_indices[0]].transport;
            let u2 = &self.links[link_indices[1]].transport;
            u1.commutator(u2).frobenius_norm()
        } else {
            0.0
        };

        Ok(HolonomyPlaquetteResult {
            cycle_nodes,
            holonomy_matrix: holonomy,
            wilson_loop_trace: wilson_trace,
            dominant_eigenvalue: dom_eig,
            net_yield_bps,
            optimal_capital_weights: weights,
            non_abelian_curvature_norm: curvature_norm,
            is_profitable_arbitrage: net_yield_bps >= self.min_yield_hurdle_bps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_abelian_commutator_and_curvature() {
        let u1 = TransportMatrix2x2::from_market_transition(1.01, 1.00, 0.05, 0.01, 2.0);
        let u2 = TransportMatrix2x2::from_market_transition(1.00, 1.02, 0.01, 0.06, 2.0);

        let comm = u1.commutator(&u2);
        // Because off-diagonal couplings and diagonal rates are asymmetric, [U1, U2] != 0
        assert!(comm.frobenius_norm() > 1e-4);
    }

    #[test]
    fn test_gauge_invariance_of_holonomy_eigenvalue_and_wilson_trace() {
        // Construct a 3-node closed triangular plaquette: 0 -> 1 -> 2 -> 0
        let u01 = TransportMatrix2x2::from_market_transition(1.005, 1.002, 0.02, 0.01, 1.0);
        let u12 = TransportMatrix2x2::from_market_transition(1.004, 1.006, 0.01, 0.03, 1.0);
        let u20 = TransportMatrix2x2::from_market_transition(0.998, 0.999, 0.02, 0.02, 1.0);

        let links = vec![
            GaugeLinkEdge {
                edge_id: "0->1".into(),
                from_node: 0,
                to_node: 1,
                transport: u01,
            },
            GaugeLinkEdge {
                edge_id: "1->2".into(),
                from_node: 1,
                to_node: 2,
                transport: u12,
            },
            GaugeLinkEdge {
                edge_id: "2->0".into(),
                from_node: 2,
                to_node: 0,
                transport: u20,
            },
        ];

        let engine = NonAbelianGaugeArbitrageEngine::new(3, links, 5.0);
        let res_orig = engine
            .evaluate_cycle(&[0, 1, 2])
            .expect("Cycle eval failed");

        // Apply arbitrary local GL(2, R) gauge transformations g_0, g_1, g_2 at each node:
        // U_{uv}' = g_u * U_{uv} * g_v^{-1}
        let g0 = TransportMatrix2x2 {
            m00: 2.5,
            m01: 0.4,
            m10: -0.3,
            m11: 1.8,
        };
        let g1 = TransportMatrix2x2 {
            m00: 0.8,
            m01: -0.2,
            m10: 0.5,
            m11: 1.4,
        };
        let g2 = TransportMatrix2x2 {
            m00: 1.7,
            m01: 0.9,
            m10: 0.1,
            m11: 1.1,
        };

        let g0_inv = g0.inverse().unwrap();
        let g1_inv = g1.inverse().unwrap();
        let g2_inv = g2.inverse().unwrap();

        let u01_gauged = g0.matmul(&u01).matmul(&g1_inv);
        let u12_gauged = g1.matmul(&u12).matmul(&g2_inv);
        let u20_gauged = g2.matmul(&u20).matmul(&g0_inv);

        let links_gauged = vec![
            GaugeLinkEdge {
                edge_id: "0->1_g".into(),
                from_node: 0,
                to_node: 1,
                transport: u01_gauged,
            },
            GaugeLinkEdge {
                edge_id: "1->2_g".into(),
                from_node: 1,
                to_node: 2,
                transport: u12_gauged,
            },
            GaugeLinkEdge {
                edge_id: "2->0_g".into(),
                from_node: 2,
                to_node: 0,
                transport: u20_gauged,
            },
        ];

        let engine_gauged = NonAbelianGaugeArbitrageEngine::new(3, links_gauged, 5.0);
        let res_gauged = engine_gauged
            .evaluate_cycle(&[0, 1, 2])
            .expect("Gauged cycle eval failed");

        // Wilson loop trace and dominant eigenvalue yield must be strictly gauge-invariant!
        assert!((res_orig.wilson_loop_trace - res_gauged.wilson_loop_trace).abs() < 1e-9);
        assert!((res_orig.dominant_eigenvalue - res_gauged.dominant_eigenvalue).abs() < 1e-9);
        assert!((res_orig.net_yield_bps - res_gauged.net_yield_bps).abs() < 1e-6);
        assert!(res_orig.is_profitable_arbitrage);
    }
}
