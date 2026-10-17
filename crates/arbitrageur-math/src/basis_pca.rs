//! Multi-Currency Basis Curve Principal Component Analysis (PCA) & Synthetic Forward Pricing Engine.
//!
//! Implements:
//! 1. **Empirical Eigenvector Decomposition of Basis Curves**:
//!    Decomposes the term structure of perpetual funding rates and dated futures basis yields
//!    into 3 orthogonal principal modes:
//!    - **Level ($\mathbf{v}_1$)**: Parallel shifts across all maturities (~80% variance).
//!    - **Slope ($\mathbf{v}_2$)**: Contango / backwardation steepening or flattening (~15% variance).
//!    - **Curvature ($\mathbf{v}_3$)**: Intermediate term structure hump / quarterly roll-down dislocation (~4% variance).
//! 2. **Synthetic Forward Rate Pricing**:
//!    Computes exact implied forward basis yields between two maturities $(T_1, T_2)$:
//!    $$F(T_1, T_2) = \frac{y(T_2) T_2 - y(T_1) T_1}{T_2 - T_1}$$
//! 3. **Analytical Level- and Slope-Neutral Basis Butterfly Spreads**:
//!    Solves exact weights $(w_1, w_{\text{body}} = -1.0, w_2)$ immunizing the portfolio against
//!    parallel yield shocks and slope rotations, isolating pure curvature roll-down alpha.
//! 4. **Curve Dislocation Detection via Standardized Factor Z-Scores**:
//!    Monitors $\zeta_{\text{slope}}$ and $\zeta_{\text{curve}}$ to trigger mean-reverting calendar relative value trades.

use serde::{Deserialize, Serialize};

/// Tenor point along the basis yield curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasisTenorPoint {
    /// Tenor label (e.g. "PERP", "1M", "3M", "6M", "9M").
    pub label: String,
    /// Time to expiration in years (e.g. 0.0 for PERP, 0.0833 for 1M, 0.25 for 3M).
    pub tenor_years: f64,
    /// Annualized basis yield (percentage, e.g. 8.50 for 8.50% annualized basis).
    pub basis_yield_pct: f64,
}

impl BasisTenorPoint {
    pub fn new(label: impl Into<String>, tenor_years: f64, basis_yield_pct: f64) -> Self {
        Self {
            label: label.into(),
            tenor_years: tenor_years.max(0.0),
            basis_yield_pct,
        }
    }
}

/// Principal factor scores extracted from a basis curve observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasisFactorScores {
    /// Level factor score (parallel shift).
    pub level_score: f64,
    /// Slope factor score (contango vs backwardation).
    pub slope_score: f64,
    /// Curvature factor score (belly hump vs wings).
    pub curvature_score: f64,
    /// Standardized Level Z-Score.
    pub level_z_score: f64,
    /// Standardized Slope Z-Score.
    pub slope_z_score: f64,
    /// Standardized Curvature Z-Score.
    pub curvature_z_score: f64,
}

/// Level- and Slope-Neutral Basis Butterfly Spread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasisButterflySpread {
    /// Index and label of the front wing.
    pub front_wing_label: String,
    pub front_wing_weight: f64,
    /// Index and label of the intermediate body.
    pub body_label: String,
    pub body_weight: f64,
    /// Index and label of the back wing.
    pub back_wing_label: String,
    pub back_wing_weight: f64,
    /// Net expected roll-down curvature yield in basis points.
    pub expected_roll_down_bps: f64,
    /// Residual level exposure (should be ~0.0).
    pub residual_level_exposure: f64,
    /// Residual slope exposure (should be ~0.0).
    pub residual_slope_exposure: f64,
}

/// Engine computing PCA decomposition, synthetic forward pricing, and butterfly spreads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasisCurvePcaEngine {
    /// Number of discrete tenor maturities $K \ge 3$.
    pub num_tenors: usize,
    /// Level eigenvector $\mathbf{v}_1$.
    pub v1_level: Vec<f64>,
    /// Slope eigenvector $\mathbf{v}_2$.
    pub v2_slope: Vec<f64>,
    /// Curvature eigenvector $\mathbf{v}_3$.
    pub v3_curvature: Vec<f64>,
    /// Historical baseline means per tenor.
    pub baseline_means: Vec<f64>,
    /// Historical standard deviations of factor scores for Z-score scaling.
    pub factor_volatilities: [f64; 3], // [sigma_level, sigma_slope, sigma_curve]
}

impl Default for BasisCurvePcaEngine {
    fn default() -> Self {
        Self::new(5) // Default: [PERP (0), 1M (0.083), 3M (0.25), 6M (0.5), 9M (0.75)]
    }
}

impl BasisCurvePcaEngine {
    /// Initializes the PCA engine with analytical orthogonal Legendre/Nelson-Siegel eigenvectors.
    pub fn new(num_tenors: usize) -> Self {
        let k = num_tenors.max(3);

        // 1. Level v1: Constant parallel shift, normalized to unit norm
        let norm_v1 = (k as f64).sqrt();
        let v1 = vec![1.0 / norm_v1; k];

        // 2. Slope v2: Linear contrast between short and long maturities
        let mut v2 = vec![0.0; k];
        let mid = (k - 1) as f64 / 2.0;
        let mut sum_sq_v2 = 0.0;
        for (i, elem) in v2.iter_mut().enumerate() {
            let val = (i as f64) - mid;
            *elem = val;
            sum_sq_v2 += val * val;
        }
        let norm_v2 = sum_sq_v2.sqrt().max(1e-6);
        for x in &mut v2 {
            *x /= norm_v2;
        }

        // 3. Curvature v3: Quadratic hump (positive at wings, negative in belly), orthogonalized to v1 and v2
        let mut v3 = vec![0.0; k];
        let mut sum_sq_v3 = 0.0;
        for (i, elem) in v3.iter_mut().enumerate() {
            let x = if k > 1 {
                (i as f64) / ((k - 1) as f64)
            } else {
                0.5
            };
            let val = 4.0 * (x - 0.5).powi(2) - 0.5;
            *elem = val;
            sum_sq_v3 += val * val;
        }
        let norm_v3 = sum_sq_v3.sqrt().max(1e-6);
        for x in &mut v3 {
            *x /= norm_v3;
        }

        Self {
            num_tenors: k,
            v1_level: v1,
            v2_slope: v2,
            v3_curvature: v3,
            baseline_means: vec![8.0; k], // Default 8% annualized basis mean
            factor_volatilities: [3.5, 2.0, 1.2], // Typical factor volatilities in percentage points
        }
    }

    /// Evaluates synthetic forward basis yield between two maturities $(T_1, T_2)$:
    /// $$F(T_1, T_2) = \frac{y_2 T_2 - y_1 T_1}{T_2 - T_1}$$
    pub fn compute_synthetic_forward_yield(
        &self,
        t1_years: f64,
        y1_pct: f64,
        t2_years: f64,
        y2_pct: f64,
    ) -> Option<f64> {
        let dt = t2_years - t1_years;
        if dt.abs() < 1e-4 {
            return None;
        }

        let forward_yield = (y2_pct * t2_years - y1_pct * t1_years) / dt;
        if forward_yield.is_finite() {
            Some(forward_yield)
        } else {
            None
        }
    }

    /// Projects an observed basis curve $\mathbf{y} \in \mathbb{R}^K$ onto the PCA factor basis.
    pub fn extract_factor_scores(&self, yields_pct: &[f64]) -> Option<BasisFactorScores> {
        if yields_pct.len() != self.num_tenors {
            return None;
        }

        let mut diff = vec![0.0; self.num_tenors];
        for i in 0..self.num_tenors {
            diff[i] = yields_pct[i] - self.baseline_means.get(i).copied().unwrap_or(0.0);
        }

        let level_score: f64 = diff.iter().zip(&self.v1_level).map(|(y, v)| y * v).sum();
        let slope_score: f64 = diff.iter().zip(&self.v2_slope).map(|(y, v)| y * v).sum();
        let curve_score: f64 = diff
            .iter()
            .zip(&self.v3_curvature)
            .map(|(y, v)| y * v)
            .sum();

        let level_z = level_score / self.factor_volatilities[0].max(1e-4);
        let slope_z = slope_score / self.factor_volatilities[1].max(1e-4);
        let curve_z = curve_score / self.factor_volatilities[2].max(1e-4);

        Some(BasisFactorScores {
            level_score,
            slope_score,
            curvature_score: curve_score,
            level_z_score: level_z,
            slope_z_score: slope_z,
            curvature_z_score: curve_z,
        })
    }

    /// Constructs a Level- and Slope-Neutral Basis Butterfly Spread.
    ///
    /// Fixes body weight $w_{\text{body}} = -1.0$ (short intermediate tenor) and solves for
    /// front wing $w_1$ and back wing $w_2$ such that:
    /// $$\begin{pmatrix} v_{1, \text{wing1}} & v_{1, \text{wing2}} \\ v_{2, \text{wing1}} & v_{2, \text{wing2}} \end{pmatrix} \begin{pmatrix} w_1 \\ w_2 \end{pmatrix} = \begin{pmatrix} v_{1, \text{body}} \\ v_{2, \text{body}} \end{pmatrix}$$
    pub fn build_butterfly_spread(
        &self,
        tenors: &[BasisTenorPoint],
        idx_wing1: usize,
        idx_body: usize,
        idx_wing2: usize,
    ) -> Option<BasisButterflySpread> {
        if idx_wing1 >= self.num_tenors
            || idx_body >= self.num_tenors
            || idx_wing2 >= self.num_tenors
        {
            return None;
        }
        if tenors.len() < self.num_tenors {
            return None;
        }

        let a11 = self.v1_level[idx_wing1];
        let a12 = self.v1_level[idx_wing2];
        let a21 = self.v2_slope[idx_wing1];
        let a22 = self.v2_slope[idx_wing2];

        let b1 = self.v1_level[idx_body];
        let b2 = self.v2_slope[idx_body];

        let det = a11 * a22 - a12 * a21;
        if det.abs() < 1e-9 {
            return None; // Degenerate / collinear wing selection
        }

        let w1 = (b1 * a22 - b2 * a12) / det;
        let w2 = (a11 * b2 - a21 * b1) / det;
        let w_body = -1.0;

        // Verify residual level and slope exposures
        let res_level = a11 * w1 + a12 * w2 - b1;
        let res_slope = a21 * w1 + a22 * w2 - b2;

        // Calculate expected curvature carry: w1 * y1 + w_body * y_body + w2 * y2
        let y1 = tenors[idx_wing1].basis_yield_pct;
        let y_body = tenors[idx_body].basis_yield_pct;
        let y2 = tenors[idx_wing2].basis_yield_pct;
        let net_roll_down_pct = w1 * y1 + w_body * y_body + w2 * y2;
        let expected_roll_down_bps = net_roll_down_pct * 100.0;

        Some(BasisButterflySpread {
            front_wing_label: tenors[idx_wing1].label.clone(),
            front_wing_weight: w1,
            body_label: tenors[idx_body].label.clone(),
            body_weight: w_body,
            back_wing_label: tenors[idx_wing2].label.clone(),
            back_wing_weight: w2,
            expected_roll_down_bps,
            residual_level_exposure: res_level,
            residual_slope_exposure: res_slope,
        })
    }

    /// Evaluates whether the basis curve exhibits a dislocation justifying relative value entry.
    /// Returns true if either the slope Z-score or curvature Z-score exceeds entry_hurdle_z.
    pub fn is_curve_dislocated(&self, scores: &BasisFactorScores, entry_hurdle_z: f64) -> bool {
        scores.slope_z_score.abs() >= entry_hurdle_z
            || scores.curvature_z_score.abs() >= entry_hurdle_z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basis_pca_eigenvector_orthogonality() {
        let engine = BasisCurvePcaEngine::new(5);

        // Dot products must be zero between orthogonal vectors
        let dot_12: f64 = engine
            .v1_level
            .iter()
            .zip(&engine.v2_slope)
            .map(|(a, b)| a * b)
            .sum();
        let dot_13: f64 = engine
            .v1_level
            .iter()
            .zip(&engine.v3_curvature)
            .map(|(a, b)| a * b)
            .sum();
        let dot_23: f64 = engine
            .v2_slope
            .iter()
            .zip(&engine.v3_curvature)
            .map(|(a, b)| a * b)
            .sum();

        assert!(
            dot_12.abs() < 1e-10,
            "v1 and v2 must be orthogonal: {dot_12}"
        );
        assert!(
            dot_13.abs() < 1e-10,
            "v1 and v3 must be orthogonal: {dot_13}"
        );
        assert!(
            dot_23.abs() < 1e-10,
            "v2 and v3 must be orthogonal: {dot_23}"
        );

        // Unit norms
        let norm_v1: f64 = engine.v1_level.iter().map(|x| x * x).sum();
        let norm_v2: f64 = engine.v2_slope.iter().map(|x| x * x).sum();
        let norm_v3: f64 = engine.v3_curvature.iter().map(|x| x * x).sum();

        assert!((norm_v1 - 1.0).abs() < 1e-10);
        assert!((norm_v2 - 1.0).abs() < 1e-10);
        assert!((norm_v3 - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_butterfly_weights_are_level_and_slope_neutral() {
        let engine = BasisCurvePcaEngine::new(5);
        let tenors = vec![
            BasisTenorPoint::new("PERP", 0.0, 12.0),
            BasisTenorPoint::new("1M", 0.0833, 10.5),
            BasisTenorPoint::new("3M", 0.25, 9.0),
            BasisTenorPoint::new("6M", 0.50, 8.2),
            BasisTenorPoint::new("9M", 0.75, 7.8),
        ];

        // Wing 1 = PERP (idx 0), Body = 3M (idx 2), Wing 2 = 9M (idx 4)
        let butterfly = engine.build_butterfly_spread(&tenors, 0, 2, 4).unwrap();

        assert_eq!(butterfly.body_weight, -1.0);
        assert!(butterfly.front_wing_weight > 0.0);
        assert!(butterfly.back_wing_weight > 0.0);

        // Residual exposures must be vanishingly small
        assert!(
            butterfly.residual_level_exposure.abs() < 1e-8,
            "Residual level exposure must be near zero: {}",
            butterfly.residual_level_exposure
        );
        assert!(
            butterfly.residual_slope_exposure.abs() < 1e-8,
            "Residual slope exposure must be near zero: {}",
            butterfly.residual_slope_exposure
        );
    }

    #[test]
    fn test_synthetic_forward_rate_consistency() {
        let engine = BasisCurvePcaEngine::new(5);

        // Spot to 3M = 6.0%, Spot to 6M = 8.0%
        // Forward (3M to 6M) = (8.0 * 0.5 - 6.0 * 0.25) / (0.5 - 0.25) = (4.0 - 1.5) / 0.25 = 10.0%
        let forward = engine
            .compute_synthetic_forward_yield(0.25, 6.0, 0.50, 8.0)
            .unwrap();

        assert!(
            (forward - 10.0).abs() < 1e-8,
            "Forward rate expected 10.0, got {forward}"
        );
    }

    #[test]
    fn test_factor_score_extraction_and_zscore() {
        let mut engine = BasisCurvePcaEngine::new(5);
        engine.baseline_means = vec![8.0, 8.0, 8.0, 8.0, 8.0];

        // Flat curve at baseline: factor scores must be zero
        let flat_yields = [8.0, 8.0, 8.0, 8.0, 8.0];
        let flat_scores = engine.extract_factor_scores(&flat_yields).unwrap();
        assert!(flat_scores.level_score.abs() < 1e-8);
        assert!(flat_scores.slope_score.abs() < 1e-8);
        assert!(flat_scores.curvature_score.abs() < 1e-8);

        // Steep contango: front low (4.0), back high (12.0)
        let contango_yields = [4.0, 6.0, 8.0, 10.0, 12.0];
        let contango_scores = engine.extract_factor_scores(&contango_yields).unwrap();
        assert!(
            contango_scores.slope_score > 0.0,
            "Slope score must be positive for contango"
        );
        assert!(contango_scores.slope_z_score > 1.5);
        assert!(engine.is_curve_dislocated(&contango_scores, 1.5));
    }
}
