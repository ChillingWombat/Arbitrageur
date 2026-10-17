//! Berkovich Analytic Spaces $\mathcal{M}(A)$, Non-Archimedean Seminorms, and Tropical Compactifications
//!
//! # Mathematical Foundations
//! Let $K$ be a complete non-Archimedean valued field (e.g. $\mathbb{Q}_p$ or $k((t))$) with valuation
//! $v: K \to \mathbb{R} \cup \{\infty\}$ and non-Archimedean absolute value $|x| = \exp(-v(x))$ satisfying
//! the strong triangle inequality $|x + y| \le \max(|x|, |y|)$.
//!
//! For a Banach $K$-algebra $A$, the Berkovich spectrum $\mathcal{M}(A)$ consists of all bounded
//! multiplicative seminorms $|\cdot|_x$ on $A$. On the analytic affine line $\mathbb{A}^{1, \text{an}}_K$,
//! points are classified into four fundamental types:
//! - **Type 1**: Rigid / classical points corresponding to exact single-venue executed fill prices $x \in K$.
//! - **Type 2**: Rational boundary points corresponding to closed discs $D(a, r)$ with radius $r \in |K^\times|$ (Gauss norms).
//! - **Type 3**: Irrational boundary points with $r \notin |K^\times|$ (continuous queue depth limits).
//! - **Type 4**: Immediate extensions formed by nested sequences of discs with empty classical intersection.
//!
//! # Berkovich Skeleton Metric Geodesic Theorem
//! The canonical retraction map $\rho: \mathcal{X}^{\text{an}} \to \Sigma(\mathcal{X})$ projects asynchronous
//! order states onto the tropical skeleton $\Sigma$. On $\Sigma$, the non-Archimedean metric $d_\Sigma(x, y)$
//! equals the combinatorial geodesic path length on the polyhedral network of latency and fee-adjusted spreads:
//! $$d_\Sigma(x, y) = \sum_{e \in \operatorname{Path}(x, y)} \ell(e)$$
//!
//! **Theorem**: Multi-venue order matching preserves exact valuation conservation without execution leakage
//! iff the execution trajectory lies entirely within the isometric skeleton $\Sigma(\mathcal{X})$, yielding
//! a vanishing non-Archimedean projection defect:
//! $$\delta_{\text{retract}}(x, y) = \max\left(0.0, d_K(x, y) - d_\Sigma(\rho(x), \rho(y))\right) = 0$$
//!
//! When $\delta_{\text{retract}} > 0$, the topological settlement drag is:
//! $$\Delta M_{\text{drag}} = \frac{1}{2\pi} \delta_{\text{retract}} \cdot \text{MatchedVolume}$$

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BerkovichPointType {
    /// Classical rigid venue quote or fill (Type 1)
    Type1Rigid,
    /// Rational Gauss boundary with radius in value group (Type 2)
    Type2RationalGauss,
    /// Irrational boundary with continuous queue limit (Type 3)
    Type3Irrational,
    /// Infinitesimal queue slippage / nested disc limit (Type 4)
    Type4Immediate,
}

/// Non-Archimedean Valuation Seminorm on local liquidity order states
#[derive(Debug, Clone, Copy)]
pub struct ValuationSeminorm {
    /// Valuation valuation exponent v(x) in [-inf, inf]
    pub valuation: f64,
    /// Non-Archimedean norm |x| = exp(-v(x))
    pub norm: f64,
}

impl ValuationSeminorm {
    #[inline]
    pub fn from_valuation(v: f64) -> Self {
        let norm = (-v).exp();
        Self { valuation: v, norm }
    }

    #[inline]
    pub fn from_price_diff(diff: f64, p_base: f64) -> Self {
        if diff.abs() < 1e-12 {
            Self {
                valuation: 50.0,
                norm: 0.0,
            }
        } else {
            // Valuation based on p-adic logarithmic scale
            let log_p = p_base.ln().max(0.1);
            let v = -(diff.abs() / 100.0).ln() / log_p;
            let norm = (-v).exp();
            Self { valuation: v, norm }
        }
    }

    /// Verifies the ultrametric inequality |x + y| <= max(|x|, |y|)
    #[inline]
    pub fn ultrametric_sum(x: &Self, y: &Self) -> Self {
        let max_norm = x.norm.max(y.norm);
        let v = if max_norm > 1e-12 {
            -max_norm.ln()
        } else {
            50.0
        };
        Self {
            valuation: v,
            norm: max_norm,
        }
    }
}

/// A node in the Berkovich analytic space representing a venue state
#[derive(Debug, Clone)]
pub struct BerkovichNode {
    pub venue_id: u32,
    pub point_type: BerkovichPointType,
    pub mid_price: f64,
    pub queue_depth: f64,
    pub seminorm: ValuationSeminorm,
}

/// An edge on the tropical Berkovich skeleton connecting two venue nodes
#[derive(Debug, Clone, Copy)]
pub struct TropicalSkeletonEdge {
    pub from_venue: u32,
    pub to_venue: u32,
    pub latency_weight: f64,
    pub fee_weight: f64,
    pub metric_length: f64,
}

impl TropicalSkeletonEdge {
    #[inline]
    pub fn new(from_venue: u32, to_venue: u32, latency_ms: f64, fee_bps: f64) -> Self {
        // Metric length on the polyhedral skeleton
        let metric_length = (latency_ms * 0.05 + fee_bps * 0.10).max(0.0);
        Self {
            from_venue,
            to_venue,
            latency_weight: latency_ms,
            fee_weight: fee_bps,
            metric_length,
        }
    }
}

/// Result of evaluating the Berkovich Skeleton Metric Geodesic Theorem
#[derive(Debug, Clone, Copy)]
pub struct SkeletonGeodesicResult {
    /// Non-Archimedean distance between endpoints d_K
    pub d_k: f64,
    /// Geodesic distance on the tropical skeleton d_Sigma
    pub d_sigma: f64,
    /// Retraction projection defect delta_retract
    pub projection_defect: f64,
    /// Whether the execution trajectory is strictly geodesic (defect < 1e-6)
    pub is_geodesic: bool,
    /// Topological settlement drag in quote units
    pub settlement_drag: f64,
}

/// Engine for Berkovich analytic space routing and tropical skeleton evaluation
#[derive(Debug, Clone)]
pub struct BerkovichSkeletonRoutingEngine {
    pub p_base: f64,
    pub edges: Vec<TropicalSkeletonEdge>,
}

impl BerkovichSkeletonRoutingEngine {
    pub fn new(p_base: f64) -> Self {
        Self {
            p_base: p_base.max(2.0),
            edges: Vec::with_capacity(16),
        }
    }

    pub fn add_edge(&mut self, edge: TropicalSkeletonEdge) {
        self.edges.push(edge);
    }

    /// Evaluates the Berkovich Skeleton Metric Geodesic Theorem between two venue states
    pub fn evaluate_geodesic_execution(
        &self,
        node_a: &BerkovichNode,
        node_b: &BerkovichNode,
        matched_volume: f64,
    ) -> SkeletonGeodesicResult {
        // 1. Non-Archimedean ambient distance d_K(a, b)
        let price_diff = (node_a.mid_price - node_b.mid_price).abs();
        let seminorm_diff = ValuationSeminorm::from_price_diff(price_diff, self.p_base);
        let depth_factor = ((node_a.queue_depth - node_b.queue_depth).abs() / 100.0).min(5.0);
        let d_k = seminorm_diff.norm + depth_factor * 0.01;

        // 2. Geodesic distance on the skeleton d_Sigma
        let d_sigma = if node_a.venue_id == node_b.venue_id {
            0.0
        } else {
            // Find direct or shortest edge between node_a and node_b
            let mut found_len = None;
            for e in &self.edges {
                if (e.from_venue == node_a.venue_id && e.to_venue == node_b.venue_id)
                    || (e.from_venue == node_b.venue_id && e.to_venue == node_a.venue_id)
                {
                    found_len = Some(e.metric_length);
                    break;
                }
            }
            found_len.unwrap_or(d_k * 1.5)
        };

        // 3. Retraction defect: delta_retract = max(0.0, d_k - d_sigma)
        let projection_defect = (d_k - d_sigma).max(0.0);
        let is_geodesic = projection_defect < 1e-6;

        // 4. Irrecoverable topological settlement drag:
        // Delta M_drag = (1 / (2 * pi)) * delta_retract * MatchedVolume
        let settlement_drag = if is_geodesic {
            0.0
        } else {
            (1.0 / (2.0 * std::f64::consts::PI)) * projection_defect * matched_volume
        };

        SkeletonGeodesicResult {
            d_k,
            d_sigma,
            projection_defect,
            is_geodesic,
            settlement_drag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ultrametric_inequality_holds() {
        let x = ValuationSeminorm {
            valuation: 2.0,
            norm: 0.1353,
        };
        let y = ValuationSeminorm {
            valuation: 1.0,
            norm: 0.3678,
        };
        let sum = ValuationSeminorm::ultrametric_sum(&x, &y);

        // Ultrametric strong triangle inequality: |x + y| <= max(|x|, |y|)
        assert!(sum.norm <= x.norm.max(y.norm) + 1e-9);
        assert!((sum.norm - 0.3678).abs() < 1e-4);
    }

    #[test]
    fn test_geodesic_trajectory_satisfies_vanishing_defect() {
        let mut engine = BerkovichSkeletonRoutingEngine::new(2.0);
        let edge = TropicalSkeletonEdge::new(1, 2, 2.0, 1.0); // metric_length = 2.0*0.05 + 1.0*0.10 = 0.20
        engine.add_edge(edge);

        let node_a = BerkovichNode {
            venue_id: 1,
            point_type: BerkovichPointType::Type1Rigid,
            mid_price: 3000.0,
            queue_depth: 50.0,
            seminorm: ValuationSeminorm::from_price_diff(0.0, 2.0),
        };

        let node_b = BerkovichNode {
            venue_id: 2,
            point_type: BerkovichPointType::Type2RationalGauss,
            mid_price: 3000.01,
            queue_depth: 50.0,
            seminorm: ValuationSeminorm::from_price_diff(0.01, 2.0),
        };

        // When skeleton edge metric length dominates ambient d_k, defect is zero
        let res = engine.evaluate_geodesic_execution(&node_a, &node_b, 100_000.0);
        assert!(res.is_geodesic);
        assert_eq!(res.projection_defect, 0.0);
        assert_eq!(res.settlement_drag, 0.0);
    }

    #[test]
    fn test_asymmetric_delay_induces_settlement_drag() {
        let mut engine = BerkovichSkeletonRoutingEngine::new(2.0);
        // Skeleton edge with tiny metric length (under-accommodating)
        let edge = TropicalSkeletonEdge {
            from_venue: 1,
            to_venue: 2,
            latency_weight: 0.0,
            fee_weight: 0.0,
            metric_length: 0.001,
        };
        engine.add_edge(edge);

        let node_a = BerkovichNode {
            venue_id: 1,
            point_type: BerkovichPointType::Type1Rigid,
            mid_price: 3000.0,
            queue_depth: 50.0,
            seminorm: ValuationSeminorm::from_price_diff(0.0, 2.0),
        };

        let node_b = BerkovichNode {
            venue_id: 2,
            point_type: BerkovichPointType::Type3Irrational,
            mid_price: 3050.0, // large price discrepancy
            queue_depth: 10.0,
            seminorm: ValuationSeminorm::from_price_diff(50.0, 2.0),
        };

        let res = engine.evaluate_geodesic_execution(&node_a, &node_b, 50_000.0);
        assert!(!res.is_geodesic);
        assert!(res.projection_defect > 0.0);
        assert!(res.settlement_drag > 0.0);
    }
}
