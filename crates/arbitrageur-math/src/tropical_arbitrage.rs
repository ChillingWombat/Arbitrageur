//! Tropical Geometry, Max-Plus Algebra & Karp's Minimum Mean Cycle Arbitrage (Tranche 75).
//!
//! Formulates multi-hop cross-exchange / cross-AMM cyclic arbitrage over the Tropical
//! Max-Plus semiring $(\mathbb{R} \cup \{-\infty\}, \oplus, \otimes)$ where:
//! - Tropical addition: $a \oplus b = \max(a, b)$
//! - Tropical multiplication: $a \otimes b = a + b$
//!
//! Given directed conversion quotes $R_{uv} (1 - \phi_{uv})$ between assets/venues $(u, v)$,
//! define logarithmic edge weights $w_{uv} = \ln\left(R_{uv}(1 - \phi_{uv})\right)$.
//! In tropical matrix powers $W^{\otimes k}$, the entry $(W^{\otimes k})_{uu}$ is the maximum
//! log-return of any $k$-hop closed cycle starting and ending at $u$.
//!
//! Implements Karp's $O(V \cdot E)$ theorem for the **Tropical Spectral Radius** (maximum mean
//! cycle weight $\lambda^*$):
//! $$\lambda^* = \max_{v \in V} \min_{0 \le k < n} \frac{D_n(v) - D_k(v)}{n - k}$$
//! where $D_k(v)$ is the maximum weight of an edge progression of length $k$ to vertex $v$.
//! Strictly positive $\lambda^* > 0$ certifies a globally optimal arbitrage cycle maximizing
//! logarithmic yield per hop (minimizing execution leg count and atomic failure surface).

use serde::{Deserialize, Serialize};

/// Tropical $-\infty$ additive identity.
pub const TROPICAL_NEG_INF: f64 = f64::NEG_INFINITY;

/// Tropical Max-Plus Semiring scalar wrapper.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TropicalScalar(pub f64);

impl TropicalScalar {
    /// Tropical zero $\mathbb{O} = -\infty$ (identity for $\oplus = \max$).
    #[inline(always)]
    pub fn zero() -> Self {
        Self(TROPICAL_NEG_INF)
    }

    /// Tropical unit $\mathbb{I} = 0.0$ (identity for $\otimes = +$).
    #[inline(always)]
    pub fn one() -> Self {
        Self(0.0)
    }

    /// Tropical addition $a \oplus b = \max(a, b)$.
    #[inline(always)]
    pub fn oplus(self, rhs: Self) -> Self {
        Self(self.0.max(rhs.0))
    }

    /// Tropical multiplication $a \otimes b = a + b$.
    #[inline(always)]
    pub fn otimes(self, rhs: Self) -> Self {
        if self.0 == TROPICAL_NEG_INF || rhs.0 == TROPICAL_NEG_INF {
            Self(TROPICAL_NEG_INF)
        } else {
            Self(self.0 + rhs.0)
        }
    }
}

/// Directed liquidity conversion edge in the tropical market graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TropicalMarketEdge {
    pub from_node: usize,
    pub to_node: usize,
    pub venue: String,
    pub pair_symbol: String,
    /// Raw multiplicative exchange rate before fee.
    pub raw_rate: f64,
    /// Proportional fee in basis points.
    pub fee_bps: f64,
    /// Available executable depth in USD.
    pub max_capacity_usd: f64,
}

impl TropicalMarketEdge {
    /// Tropical log-yield weight $w_{uv} = \ln\left(R_{uv} (1 - \text{fee\_bps} / 10^4)\right)$.
    #[inline]
    pub fn tropical_weight(&self) -> f64 {
        let net_mult = self.raw_rate * (1.0 - self.fee_bps / 10_000.0);
        if net_mult > 0.0 && net_mult.is_finite() {
            net_mult.ln()
        } else {
            TROPICAL_NEG_INF
        }
    }
}

/// Result of Tropical Spectral Radius & Maximum Mean Cycle extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TropicalCycleArbitrageResult {
    /// Tropical eigenvalue $\lambda^*$ (maximum mean log-yield per hop).
    pub spectral_radius_per_hop: f64,
    /// Mean yield per hop in basis points ($10^4 \cdot \lambda^*$).
    pub mean_yield_per_hop_bps: f64,
    /// Total cumulative cycle yield in basis points ($10^4 \cdot \sum_{e \in C} w_e$).
    pub total_cycle_yield_bps: f64,
    /// Ordered sequence of vertex indices forming the optimal simple cycle.
    pub cycle_nodes: Vec<usize>,
    /// Ordered sequence of edge indices forming the optimal simple cycle.
    pub cycle_edge_indices: Vec<usize>,
    /// Bottleneck executable notional capacity across the cycle in USD.
    pub bottleneck_capacity_usd: f64,
    /// Whether the cycle beats the minimum per-hop net yield hurdle.
    pub is_profitable: bool,
}

/// Square matrix over the Tropical Max-Plus Semiring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TropicalMatrix {
    pub dim: usize,
    pub data: Vec<f64>,
}

impl TropicalMatrix {
    /// Creates a $n \times n$ tropical zero matrix (all entries $-\infty$).
    pub fn new_zero(dim: usize) -> Self {
        Self {
            dim,
            data: vec![TROPICAL_NEG_INF; dim * dim],
        }
    }

    #[inline(always)]
    pub fn get(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.dim + c]
    }

    #[inline(always)]
    pub fn set(&mut self, r: usize, c: usize, val: f64) {
        let idx = r * self.dim + c;
        if val > self.data[idx] {
            self.data[idx] = val;
        }
    }

    /// Tropical matrix multiplication $(A \otimes B)_{ij} = \bigoplus_{k=0}^{n-1} (A_{ik} \otimes B_{kj}) = \max_k (A_{ik} + B_{kj})$.
    pub fn tropical_matmul(&self, rhs: &Self) -> Self {
        assert_eq!(self.dim, rhs.dim);
        let n = self.dim;
        let mut out = Self::new_zero(n);
        for i in 0..n {
            for k in 0..n {
                let a_ik = self.get(i, k);
                if a_ik == TROPICAL_NEG_INF {
                    continue;
                }
                for j in 0..n {
                    let b_kj = rhs.get(k, j);
                    if b_kj != TROPICAL_NEG_INF {
                        let prod = a_ik + b_kj;
                        let idx = i * n + j;
                        if prod > out.data[idx] {
                            out.data[idx] = prod;
                        }
                    }
                }
            }
        }
        out
    }

    /// Tropical trace $\operatorname{tr}_{\oplus}(A) = \bigoplus_{i=0}^{n-1} A_{ii} = \max_i A_{ii}$.
    pub fn tropical_trace(&self) -> f64 {
        (0..self.dim)
            .map(|i| self.get(i, i))
            .fold(TROPICAL_NEG_INF, f64::max)
    }
}

/// Karp's Maximum Mean Cycle & Tropical Eigenvalue Solver.
pub struct TropicalArbitrageEngine {
    pub num_nodes: usize,
    pub edges: Vec<TropicalMarketEdge>,
    pub min_per_hop_hurdle_bps: f64,
}

impl TropicalArbitrageEngine {
    pub fn new(
        num_nodes: usize,
        edges: Vec<TropicalMarketEdge>,
        min_per_hop_hurdle_bps: f64,
    ) -> Self {
        Self {
            num_nodes,
            edges,
            min_per_hop_hurdle_bps: min_per_hop_hurdle_bps.max(0.0),
        }
    }

    /// Builds the Tropical adjacency matrix $W_{uv} = \max_{e: u \to v} w_e$.
    pub fn build_adjacency_matrix(&self) -> TropicalMatrix {
        let mut mat = TropicalMatrix::new_zero(self.num_nodes);
        for edge in &self.edges {
            if edge.from_node < self.num_nodes && edge.to_node < self.num_nodes {
                mat.set(edge.from_node, edge.to_node, edge.tropical_weight());
            }
        }
        mat
    }

    /// Scores a three-leg cycle in the max-plus semiring. The result is the
    /// cumulative log yield, using the best executable edge per leg.
    pub fn evaluate_3hop_cycle(&self, u: usize, v: usize, w: usize) -> Option<f64> {
        if u >= self.num_nodes
            || v >= self.num_nodes
            || w >= self.num_nodes
            || u == v
            || v == w
            || w == u
        {
            return None;
        }
        let legs = [(u, v), (v, w), (w, u)];
        let mut total = 0.0;
        for (from, to) in legs {
            let best = self
                .edges
                .iter()
                .filter(|edge| {
                    edge.from_node == from && edge.to_node == to && edge.max_capacity_usd > 0.0
                })
                .map(TropicalMarketEdge::tropical_weight)
                .filter(|weight| weight.is_finite())
                .reduce(f64::max)?;
            total += best;
        }
        total.is_finite().then_some(total)
    }

    /// Executes Karp's $O(V \cdot E)$ algorithm to compute the exact Tropical Spectral Radius $\lambda^*$
    /// and extract the highest-efficiency simple arbitrage cycle.
    pub fn solve_maximum_mean_cycle(&self) -> Option<TropicalCycleArbitrageResult> {
        let n = self.num_nodes;
        if n == 0 || self.edges.is_empty() {
            return None;
        }

        // D[k][v] = max tropical weight of a k-step progression ending at v.
        // Initialize D[0][v] = 0.0 for all v (virtual super-source with 0-weight edges to all vertices).
        let mut d = vec![vec![TROPICAL_NEG_INF; n]; n + 1];
        let mut pred_edge: Vec<Vec<Option<usize>>> = vec![vec![None; n]; n + 1];

        d[0].fill(0.0);

        for k in 1..=n {
            for (e_idx, edge) in self.edges.iter().enumerate() {
                if edge.from_node >= n || edge.to_node >= n || edge.max_capacity_usd <= 0.0 {
                    continue;
                }
                let prev_w = d[k - 1][edge.from_node];
                if prev_w == TROPICAL_NEG_INF {
                    continue;
                }
                let w = edge.tropical_weight();
                if w == TROPICAL_NEG_INF {
                    continue;
                }
                let cand = prev_w + w;
                if cand > d[k][edge.to_node] {
                    d[k][edge.to_node] = cand;
                    pred_edge[k][edge.to_node] = Some(e_idx);
                }
            }
        }

        // Karp's formula: \lambda^* = \max_{v} \min_{0 \le k < n} (D[n][v] - D[k][v]) / (n - k)
        let mut lambda_star = TROPICAL_NEG_INF;
        let mut best_v = None;

        for (v, &d_n_v) in d[n].iter().enumerate() {
            if d_n_v == TROPICAL_NEG_INF {
                continue;
            }
            let mut min_ratio = f64::INFINITY;
            for (k, d_k) in d.iter().enumerate().take(n) {
                if d_k[v] != TROPICAL_NEG_INF {
                    let ratio = (d_n_v - d_k[v]) / ((n - k) as f64);
                    if ratio < min_ratio {
                        min_ratio = ratio;
                    }
                }
            }
            if min_ratio.is_finite() && min_ratio > lambda_star {
                lambda_star = min_ratio;
                best_v = Some(v);
            }
        }

        let end_v = best_v?;
        if !lambda_star.is_finite() {
            return None;
        }

        // Backtrack from (n, end_v) to extract the simple cycle
        let mut curr_v = end_v;
        let mut path_edges = Vec::with_capacity(n);
        for k in (1..=n).rev() {
            let e_idx = pred_edge[k][curr_v]?;
            path_edges.push(e_idx);
            curr_v = self.edges[e_idx].from_node;
        }
        path_edges.reverse();

        // Find the first repeated vertex along path_edges to isolate the simple cycle
        let mut visited_at: Vec<Option<usize>> = vec![None; n];
        let mut cycle_edge_slice = path_edges.clone();
        for (step, &e_idx) in path_edges.iter().enumerate() {
            let u = self.edges[e_idx].from_node;
            if let Some(first_step) = visited_at[u] {
                cycle_edge_slice = path_edges[first_step..step].to_vec();
                break;
            }
            visited_at[u] = Some(step);
            let v_next = self.edges[e_idx].to_node;
            if let Some(first_step) = visited_at[v_next] {
                cycle_edge_slice = path_edges[first_step..=step].to_vec();
                break;
            }
        }

        if cycle_edge_slice.is_empty() {
            return None;
        }

        let mut total_log_w = 0.0;
        let mut bottleneck_usd = f64::INFINITY;
        let mut cycle_nodes = Vec::with_capacity(cycle_edge_slice.len() + 1);
        cycle_nodes.push(self.edges[cycle_edge_slice[0]].from_node);

        for &e_idx in &cycle_edge_slice {
            let e = &self.edges[e_idx];
            total_log_w += e.tropical_weight();
            bottleneck_usd = bottleneck_usd.min(e.max_capacity_usd);
            cycle_nodes.push(e.to_node);
        }

        let hops = cycle_edge_slice.len() as f64;
        let realized_mean_log = total_log_w / hops;
        let mean_yield_per_hop_bps = realized_mean_log * 10_000.0;
        let total_cycle_yield_bps = total_log_w * 10_000.0;

        Some(TropicalCycleArbitrageResult {
            spectral_radius_per_hop: realized_mean_log,
            mean_yield_per_hop_bps,
            total_cycle_yield_bps,
            cycle_nodes,
            cycle_edge_indices: cycle_edge_slice,
            bottleneck_capacity_usd: bottleneck_usd,
            is_profitable: mean_yield_per_hop_bps >= self.min_per_hop_hurdle_bps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tropical_semiring_axioms_and_matrix_powers() {
        let a = TropicalScalar(2.5);
        let b = TropicalScalar(4.0);
        let c = TropicalScalar(-1.0);

        // Associativity and distributivity: a \otimes (b \oplus c) == (a \otimes b) \oplus (a \otimes c)
        let lhs = a.otimes(b.oplus(c));
        let rhs = (a.otimes(b)).oplus(a.otimes(c));
        assert!((lhs.0 - rhs.0).abs() < 1e-12);
        assert!((lhs.0 - 6.5).abs() < 1e-12);
    }

    #[test]
    fn test_karps_maximum_mean_cycle_prefers_higher_per_hop_yield() {
        // Construct a 4-node graph with two cycles:
        // Cycle 1 (2 hops: 0 -> 1 -> 0): total yield = +30 bps => +15 bps/hop
        // Cycle 2 (3 hops: 1 -> 2 -> 3 -> 1): total yield = +36 bps => +12 bps/hop
        // Karp's algorithm must select Cycle 1 because +15 bps/hop > +12 bps/hop!
        let edges = vec![
            TropicalMarketEdge {
                from_node: 0,
                to_node: 1,
                venue: "hyperliquid".into(),
                pair_symbol: "BTC/USDC".into(),
                raw_rate: 1.0020,
                fee_bps: 2.0, // net ~ +18 bps
                max_capacity_usd: 50_000.0,
            },
            TropicalMarketEdge {
                from_node: 1,
                to_node: 0,
                venue: "binance".into(),
                pair_symbol: "USDC/BTC".into(),
                raw_rate: 1.0014,
                fee_bps: 2.0, // net ~ +12 bps (Cycle 1 sum ~ 30 bps / 2 = 15 bps/hop)
                max_capacity_usd: 40_000.0,
            },
            TropicalMarketEdge {
                from_node: 1,
                to_node: 2,
                venue: "okx".into(),
                pair_symbol: "ETH/BTC".into(),
                raw_rate: 1.0014,
                fee_bps: 2.0, // net ~ +12 bps
                max_capacity_usd: 30_000.0,
            },
            TropicalMarketEdge {
                from_node: 2,
                to_node: 3,
                venue: "deribit".into(),
                pair_symbol: "SOL/ETH".into(),
                raw_rate: 1.0014,
                fee_bps: 2.0, // net ~ +12 bps
                max_capacity_usd: 25_000.0,
            },
            TropicalMarketEdge {
                from_node: 3,
                to_node: 1,
                venue: "lighter".into(),
                pair_symbol: "BTC/SOL".into(),
                raw_rate: 1.0014,
                fee_bps: 2.0, // net ~ +12 bps (Cycle 2 sum ~ 36 bps / 3 = 12 bps/hop)
                max_capacity_usd: 20_000.0,
            },
        ];

        let engine = TropicalArbitrageEngine::new(4, edges, 5.0);
        let res = engine
            .solve_maximum_mean_cycle()
            .expect("Must detect optimal cycle");

        assert!(res.is_profitable);
        assert_eq!(res.cycle_edge_indices.len(), 2);
        assert!((res.mean_yield_per_hop_bps - 15.0).abs() < 0.2);
        assert!((res.total_cycle_yield_bps - 30.0).abs() < 0.4);
        assert_eq!(res.bottleneck_capacity_usd, 40_000.0);
    }

    #[test]
    fn test_evaluate_3hop_cycle_returns_max_plus_log_yield() {
        let engine = TropicalArbitrageEngine::new(
            3,
            vec![
                TropicalMarketEdge {
                    from_node: 0,
                    to_node: 1,
                    venue: "a".into(),
                    pair_symbol: "BTC/USDC".into(),
                    raw_rate: 1.01,
                    fee_bps: 10.0,
                    max_capacity_usd: 10_000.0,
                },
                TropicalMarketEdge {
                    from_node: 1,
                    to_node: 2,
                    venue: "b".into(),
                    pair_symbol: "ETH/BTC".into(),
                    raw_rate: 1.02,
                    fee_bps: 20.0,
                    max_capacity_usd: 10_000.0,
                },
                TropicalMarketEdge {
                    from_node: 2,
                    to_node: 0,
                    venue: "c".into(),
                    pair_symbol: "USDC/ETH".into(),
                    raw_rate: 1.03,
                    fee_bps: 30.0,
                    max_capacity_usd: 10_000.0,
                },
            ],
            0.0,
        );

        let expected = (1.01_f64 * 0.999).ln() + (1.02_f64 * 0.998).ln() + (1.03_f64 * 0.997).ln();
        let score = engine
            .evaluate_3hop_cycle(0, 1, 2)
            .expect("complete triangle should evaluate");
        assert!((score - expected).abs() < 1e-12);
        assert!(engine.evaluate_3hop_cycle(0, 2, 1).is_none());
    }
}
