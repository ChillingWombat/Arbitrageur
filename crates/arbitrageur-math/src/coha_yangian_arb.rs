//! Cohomological Hall Algebras (CoHA), BPS Lie Algebras & Yangian Symmetry on Multi-Exchange Order Flow Tensors.
//!
//! Models multi-venue order flow networks as quiver representations $Q = (V, A)$ and evaluates
//! quantum group Yangian $Y(\mathfrak{gl}_n)$ symmetries via rational R-matrices and Bethe transfer matrices.
//! Detects toxic flow and front-running dislocations via Yangian symmetry breaking $\Delta_{\text{Yangian}}$.

use serde::{Deserialize, Serialize};

/// Directed arrow in the liquidity quiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuiverArrow {
    pub arrow_id: usize,
    pub source_vertex: usize,
    pub target_vertex: usize,
    pub capacity_usd: f64,
    pub latency_ms: f64,
    pub fee_rate: f64,
}

/// Trading venue node in the liquidity quiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuiverNode {
    pub node_id: usize,
    pub name: String,
    pub dimension: usize,
    pub total_liquidity_usd: f64,
}

/// Representation of a multi-exchange order flow quiver $Q = (V, A)$.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderFlowQuiver {
    pub nodes: Vec<QuiverNode>,
    pub arrows: Vec<QuiverArrow>,
}

impl OrderFlowQuiver {
    pub fn new(nodes: Vec<QuiverNode>, arrows: Vec<QuiverArrow>) -> Self {
        Self { nodes, arrows }
    }

    /// Evaluates the Euler-Ringel bilinear form $\langle \mathbf{d}, \mathbf{d}' \rangle = \sum_{v \in V} d_v d_v' - \sum_{a: u \to v} d_u d_v'$.
    pub fn euler_ringel_form(&self, d1: &[usize], d2: &[usize]) -> i64 {
        let n = self.nodes.len();
        if d1.len() != n || d2.len() != n {
            return 0;
        }

        let mut sum_v: i64 = 0;
        for i in 0..n {
            sum_v += (d1[i] * d2[i]) as i64;
        }

        let mut sum_a: i64 = 0;
        for a in &self.arrows {
            if a.source_vertex < n && a.target_vertex < n {
                sum_a += (d1[a.source_vertex] * d2[a.target_vertex]) as i64;
            }
        }

        sum_v - sum_a
    }

    /// Symmetric Euler form $( \mathbf{d}, \mathbf{d}' ) = \langle \mathbf{d}, \mathbf{d}' \rangle + \langle \mathbf{d}', \mathbf{d} \rangle$.
    pub fn symmetric_euler_form(&self, d1: &[usize], d2: &[usize]) -> i64 {
        self.euler_ringel_form(d1, d2) + self.euler_ringel_form(d2, d1)
    }
}

/// Rational R-matrix for $Y(\mathfrak{gl}_n)$: $R(u) = u \mathbf{I}_{n^2} + \hbar \mathbf{P}$.
#[derive(Debug, Clone)]
pub struct YangianRMatrix {
    pub dim: usize,
    pub hbar: f64,
}

impl YangianRMatrix {
    pub fn new(dim: usize, hbar: f64) -> Self {
        Self {
            dim: dim.max(1),
            hbar,
        }
    }

    /// Evaluates $R(u) \in \mathbb{R}^{n^2 \times n^2}$.
    /// Index mapping for tensor product $(i, j) \in [0, n) \times [0, n)$ is $i \cdot n + j$.
    pub fn eval(&self, u: f64) -> Vec<Vec<f64>> {
        let n = self.dim;
        let n2 = n * n;
        let mut mat = vec![vec![0.0; n2]; n2];

        for i in 0..n {
            for j in 0..n {
                let row = i * n + j;
                // Identity part: u * I
                mat[row][row] += u;
                // Permutation operator P: swaps tensor factors, so (i, j) -> (j, i)
                let col = j * n + i;
                mat[row][col] += self.hbar;
            }
        }

        mat
    }
}

/// Evaluates Yang-Baxter equation error: $\| R_{12}(u - v) R_{13}(u) R_{23}(v) - R_{23}(v) R_{13}(u) R_{12}(u - v) \|_F$.
pub fn check_yang_baxter_residual(dim: usize, hbar: f64, u: f64, v: f64) -> f64 {
    let r = YangianRMatrix::new(dim, hbar);
    let n = dim;
    let n3 = n * n * n;

    let r12 = embed_r12(&r.eval(u - v), n);
    let r13 = embed_r13(&r.eval(u), n);
    let r23 = embed_r23(&r.eval(v), n);

    let lhs = mat_mul_3(&mat_mul_3(&r12, &r13, n3), &r23, n3);
    let rhs = mat_mul_3(&mat_mul_3(&r23, &r13, n3), &r12, n3);

    let mut frobenius_sq = 0.0;
    for i in 0..n3 {
        for j in 0..n3 {
            let diff = lhs[i][j] - rhs[i][j];
            frobenius_sq += diff * diff;
        }
    }
    frobenius_sq.sqrt()
}

fn embed_r12(r: &[Vec<f64>], n: usize) -> Vec<Vec<f64>> {
    let n3 = n * n * n;
    let mut out = vec![vec![0.0; n3]; n3];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let row = (i * n + j) * n + k;
                for ip in 0..n {
                    for jp in 0..n {
                        let col = (ip * n + jp) * n + k;
                        let r_row = i * n + j;
                        let r_col = ip * n + jp;
                        out[row][col] = r[r_row][r_col];
                    }
                }
            }
        }
    }
    out
}

fn embed_r23(r: &[Vec<f64>], n: usize) -> Vec<Vec<f64>> {
    let n3 = n * n * n;
    let mut out = vec![vec![0.0; n3]; n3];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let row = (i * n + j) * n + k;
                for jp in 0..n {
                    for kp in 0..n {
                        let col = (i * n + jp) * n + kp;
                        let r_row = j * n + k;
                        let r_col = jp * n + kp;
                        out[row][col] = r[r_row][r_col];
                    }
                }
            }
        }
    }
    out
}

fn embed_r13(r: &[Vec<f64>], n: usize) -> Vec<Vec<f64>> {
    let n3 = n * n * n;
    let mut out = vec![vec![0.0; n3]; n3];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let row = (i * n + j) * n + k;
                for ip in 0..n {
                    for kp in 0..n {
                        let col = (ip * n + j) * n + kp;
                        let r_row = i * n + k;
                        let r_col = ip * n + kp;
                        out[row][col] = r[r_row][r_col];
                    }
                }
            }
        }
    }
    out
}

fn mat_mul_3(a: &[Vec<f64>], b: &[Vec<f64>], sz: usize) -> Vec<Vec<f64>> {
    let mut c = vec![vec![0.0; sz]; sz];
    for i in 0..sz {
        for k in 0..sz {
            let aik = a[i][k];
            if aik != 0.0 {
                for j in 0..sz {
                    c[i][j] += aik * b[k][j];
                }
            }
        }
    }
    c
}

/// Empirical Monodromy Transfer Matrix $T(u) = \mathbf{I} + \frac{\hbar}{u} T^{(1)}$.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonodromyTransferMatrix {
    pub dim: usize,
    pub hbar: f64,
    /// Generator components $T^{(1)}_{ij}$
    pub t1: Vec<Vec<f64>>,
}

impl MonodromyTransferMatrix {
    pub fn new(dim: usize, hbar: f64, t1: Vec<Vec<f64>>) -> Self {
        Self { dim, hbar, t1 }
    }

    /// Evaluates $T(u) \in \mathbb{R}^{n \times n}$ at spectral parameter $u$.
    #[allow(clippy::needless_range_loop)]
    pub fn eval(&self, u: f64) -> Vec<Vec<f64>> {
        let n = self.dim;
        let mut mat = vec![vec![0.0; n]; n];
        let inv_u = if u.abs() < 1e-12 { 1e12 } else { 1.0 / u };

        for i in 0..n {
            for j in 0..n {
                let mut val = self.hbar * self.t1[i][j] * inv_u;
                if i == j {
                    val += 1.0;
                }
                mat[i][j] = val;
            }
        }
        mat
    }

    /// Quantum determinant $\operatorname{qdet} T(u) = T_{11}(u) T_{22}(u - \hbar) - T_{12}(u) T_{21}(u - \hbar)$ for $n=2$.
    pub fn quantum_determinant_2x2(&self, u: f64) -> f64 {
        if self.dim != 2 {
            return 1.0;
        }
        let tu = self.eval(u);
        let tu_shift = self.eval(u - self.hbar);

        tu[0][0] * tu_shift[1][1] - tu[0][1] * tu_shift[1][0]
    }

    /// Bethe transfer trace $\tau(u) = \operatorname{tr} T(u)$.
    #[allow(clippy::needless_range_loop)]
    pub fn bethe_trace(&self, u: f64) -> f64 {
        let tu = self.eval(u);
        let mut tr = 0.0;
        for i in 0..self.dim {
            tr += tu[i][i];
        }
        tr
    }
}

/// Diagnostic result from CoHA and Yangian symmetry evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohaYangianResult {
    pub bethe_trace_at_u: f64,
    pub quantum_determinant: f64,
    pub yangian_symmetry_residual: f64,
    pub is_toxic_flow_detected: bool,
    pub bps_coha_euler_characteristic: i64,
}

/// Cohomological Hall Algebra & Yangian Order Flow Analyzer.
pub struct CohaYangianEngine {
    pub quiver: OrderFlowQuiver,
    pub r_matrix: YangianRMatrix,
    pub toxic_residual_threshold: f64,
}

impl CohaYangianEngine {
    pub fn new(quiver: OrderFlowQuiver, hbar: f64, toxic_threshold: f64) -> Self {
        let dim = quiver.nodes.len().max(2);
        Self {
            quiver,
            r_matrix: YangianRMatrix::new(dim, hbar),
            toxic_residual_threshold: toxic_threshold.max(1e-6),
        }
    }

    /// Analyzes an empirical order flow state against Yangian intertwining relations:
    /// $R(u - v) (T(u) \otimes 1) (1 \otimes T(v)) = (1 \otimes T(v)) (T(u) \otimes 1) R(u - v)$.
    #[allow(clippy::needless_range_loop)]
    pub fn analyze_order_flow(
        &self,
        monodromy: &MonodromyTransferMatrix,
        u: f64,
        v: f64,
    ) -> CohaYangianResult {
        let n = monodromy.dim;
        let n2 = n * n;
        let r_uv = self.r_matrix.eval(u - v);

        let tu = monodromy.eval(u);
        let tv = monodromy.eval(v);

        // Compute (T(u) \otimes I)
        let mut tu_x_i = vec![vec![0.0; n2]; n2];
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    let row = i * n + k;
                    let col = j * n + k;
                    tu_x_i[row][col] = tu[i][j];
                }
            }
        }

        // Compute (I \otimes T(v))
        let mut i_x_tv = vec![vec![0.0; n2]; n2];
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    let row = k * n + i;
                    let col = k * n + j;
                    i_x_tv[row][col] = tv[i][j];
                }
            }
        }

        // LHS = R(u - v) * (T(u) \otimes I) * (I \otimes T(v))
        let step1_lhs = mat_mul_2(&tu_x_i, &i_x_tv, n2);
        let lhs = mat_mul_2(&r_uv, &step1_lhs, n2);

        // RHS = (I \otimes T(v)) * (T(u) \otimes I) * R(u - v)
        let step1_rhs = mat_mul_2(&i_x_tv, &tu_x_i, n2);
        let rhs = mat_mul_2(&step1_rhs, &r_uv, n2);

        let mut frob_sq = 0.0;
        for i in 0..n2 {
            for j in 0..n2 {
                let diff = lhs[i][j] - rhs[i][j];
                frob_sq += diff * diff;
            }
        }
        let res = frob_sq.sqrt();

        let bethe_tr = monodromy.bethe_trace(u);
        let qdet = if n == 2 {
            monodromy.quantum_determinant_2x2(u)
        } else {
            1.0
        };

        let dim_vec = vec![1; self.quiver.nodes.len()];
        let euler_chi = self.quiver.euler_ringel_form(&dim_vec, &dim_vec);

        CohaYangianResult {
            bethe_trace_at_u: bethe_tr,
            quantum_determinant: qdet,
            yangian_symmetry_residual: res,
            is_toxic_flow_detected: res > self.toxic_residual_threshold,
            bps_coha_euler_characteristic: euler_chi,
        }
    }
}

fn mat_mul_2(a: &[Vec<f64>], b: &[Vec<f64>], sz: usize) -> Vec<Vec<f64>> {
    let mut c = vec![vec![0.0; sz]; sz];
    for i in 0..sz {
        for k in 0..sz {
            let aik = a[i][k];
            if aik != 0.0 {
                for j in 0..sz {
                    c[i][j] += aik * b[k][j];
                }
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yang_baxter_equation_satisfaction() {
        // Dimension n = 2, hbar = 0.5, u = 3.0, v = 1.0
        let residual = check_yang_baxter_residual(2, 0.5, 3.0, 1.0);
        assert!(
            residual < 1e-10,
            "Yang-Baxter residual must vanish identically, got: {}",
            residual
        );
    }

    #[test]
    fn test_coha_quiver_euler_form_and_symmetry_breaking() {
        let nodes = vec![
            QuiverNode {
                node_id: 0,
                name: "binance".into(),
                dimension: 2,
                total_liquidity_usd: 1_000_000.0,
            },
            QuiverNode {
                node_id: 1,
                name: "hyperliquid".into(),
                dimension: 2,
                total_liquidity_usd: 800_000.0,
            },
        ];

        let arrows = vec![QuiverArrow {
            arrow_id: 0,
            source_vertex: 0,
            target_vertex: 1,
            capacity_usd: 500_000.0,
            latency_ms: 12.0,
            fee_rate: 0.0004,
        }];

        let quiver = OrderFlowQuiver::new(nodes, arrows);
        let d = vec![1, 1];
        // Euler-Ringel form: sum(1*1 + 1*1) - 1*(1*1) = 2 - 1 = 1
        assert_eq!(quiver.euler_ringel_form(&d, &d), 1);

        let engine = CohaYangianEngine::new(quiver, 0.1, 0.05);

        // Symmetric balanced flow: T^(1) is proportional to identity
        let balanced_t1 = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let balanced_mono = MonodromyTransferMatrix::new(2, 0.1, balanced_t1);
        let res_bal = engine.analyze_order_flow(&balanced_mono, 2.0, 1.0);
        assert!(!res_bal.is_toxic_flow_detected);
        assert!(res_bal.yangian_symmetry_residual < 0.02);

        // Highly asymmetric toxic front-running flow
        let toxic_t1 = vec![vec![0.0, 50.0], vec![0.0, 0.0]];
        let toxic_mono = MonodromyTransferMatrix::new(2, 0.1, toxic_t1);
        let res_toxic = engine.analyze_order_flow(&toxic_mono, 2.0, 1.0);
        assert!(res_toxic.is_toxic_flow_detected);
        assert!(res_toxic.yangian_symmetry_residual > 0.1);
    }
}
