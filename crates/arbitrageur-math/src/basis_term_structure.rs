//! Nelson-Siegel 3-Factor Basis Term Structure Calibrator
//! and Duration/Convexity-Matched Butterfly Construction Engine.
//!
//! Provides:
//! - Nelson-Siegel 3-factor yield/basis curve parameterization connecting instantaneous
//!   perpetual funding rates ($m = 0$) with dated quarterly futures (1M, 3M, 6M):
//!   $$y(m) = \beta_0 + \beta_1 \left( \frac{1 - e^{-m/\tau}}{m/\tau} \right) + \beta_2 \left( \frac{1 - e^{-m/\tau}}{m/\tau} - e^{-m/\tau} \right)$$
//! - Factor duration vector $[\mathcal{D}_{\beta_0}, \mathcal{D}_{\beta_1}, \mathcal{D}_{\beta_2}]$ and
//!   analytical roll-down yield rate $-m \frac{\partial y(m)}{\partial m}$.
//! - Weighted Least Squares (WLS) calibration via closed-form 3x3 linear system solver.
//! - Duration- and slope-matched butterfly spread construction ($\beta_0$-neutral, $\beta_1$-neutral)
//!   extracting pure curvature ($\beta_2$) roll-down yield.

use serde::{Deserialize, Serialize};

/// Nelson-Siegel 3-Factor Basis Term Structure Curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NelsonSiegelCurve {
    /// Level parameter beta_0 (asymptotic long-term basis yield as m -> inf).
    pub beta0: f64,
    /// Slope parameter beta_1 (spread between perpetual rate y(0) and long-term yield).
    pub beta1: f64,
    /// Curvature parameter beta_2 (intermediate hump or dip).
    pub beta2: f64,
    /// Scale parameter tau (maturity horizon of curvature peak).
    pub tau: f64,
}

impl NelsonSiegelCurve {
    pub fn new(beta0: f64, beta1: f64, beta2: f64, tau: f64) -> Self {
        Self {
            beta0,
            beta1,
            beta2,
            tau: tau.max(0.01),
        }
    }

    /// Basis yield y(m) at maturity m (in years).
    pub fn yield_at(&self, m: f64) -> f64 {
        if m <= 1e-6 {
            return self.beta0 + self.beta1; // Instantaneous perpetual rate: g1(0)=1, g2(0)=0
        }
        let m_tau = m / self.tau;
        let exp_neg = (-m_tau).exp();
        let g1 = (1.0 - exp_neg) / m_tau;
        let g2 = g1 - exp_neg;
        self.beta0 + self.beta1 * g1 + self.beta2 * g2
    }

    /// Nelson-Siegel factor durations [D_beta0, D_beta1, D_beta2].
    pub fn factor_durations(&self, m: f64) -> [f64; 3] {
        if m <= 1e-6 {
            return [0.0, 0.0, 0.0];
        }
        let m_tau = m / self.tau;
        let exp_neg = (-m_tau).exp();
        let d_beta0 = m;
        let d_beta1 = self.tau * (1.0 - exp_neg);
        let d_beta2 = self.tau * (1.0 - exp_neg) - m * exp_neg;
        [d_beta0, d_beta1, d_beta2]
    }

    /// Roll-down yield rate: -m * dy(m)/dm.
    ///
    /// Reflects the instantaneous rate of basis appreciation as the contract rolls toward expiry.
    pub fn roll_down_rate(&self, m: f64) -> f64 {
        if m <= 1e-6 {
            return 0.0;
        }
        let m_tau = m / self.tau;
        let exp_neg = (-m_tau).exp();
        let g1 = (1.0 - exp_neg) / m_tau;

        let dy_dm_beta1 = (exp_neg - g1) / m;
        let dy_dm_beta2 = (exp_neg - g1) / m + (m_tau / self.tau) * exp_neg;

        let dy_dm = self.beta1 * dy_dm_beta1 + self.beta2 * dy_dm_beta2;
        -m * dy_dm
    }
}

/// Calibrates Nelson-Siegel curve using Weighted Least Squares (WLS).
pub struct NelsonSiegelCalibrator;

impl NelsonSiegelCalibrator {
    /// Calibrates [beta0, beta1, beta2] given maturities m_i, basis yields y_i, and weights w_i.
    pub fn calibrate(
        maturities: &[f64],
        yields: &[f64],
        weights: &[f64],
        tau: f64,
    ) -> Option<NelsonSiegelCurve> {
        let p = maturities.len();
        if p < 3 || yields.len() != p || weights.len() != p {
            return None;
        }

        // Construct 3x3 normal equation: (X^T W X) beta = X^T W y
        let mut xtwx = [[0.0; 3]; 3];
        let mut xtwy = [0.0; 3];

        for i in 0..p {
            let m = maturities[i];
            let w = weights[i].max(1e-4);
            let y = yields[i];

            let (g1, g2) = if m <= 1e-6 {
                (1.0, 0.0)
            } else {
                let m_tau = m / tau;
                let exp_neg = (-m_tau).exp();
                let g1 = (1.0 - exp_neg) / m_tau;
                (g1, g1 - exp_neg)
            };
            let row = [1.0, g1, g2];

            for r in 0..3 {
                xtwy[r] += row[r] * w * y;
                for c in 0..3 {
                    xtwx[r][c] += row[r] * w * row[c];
                }
            }
        }

        // Solve 3x3 system via Cramer's rule
        let det = Self::det3(&xtwx);
        if det.abs() < 1e-12 {
            return None;
        }

        let beta0 = Self::det3_replace_col(&xtwx, &xtwy, 0) / det;
        let beta1 = Self::det3_replace_col(&xtwx, &xtwy, 1) / det;
        let beta2 = Self::det3_replace_col(&xtwx, &xtwy, 2) / det;

        Some(NelsonSiegelCurve::new(beta0, beta1, beta2, tau))
    }

    #[inline]
    fn det3(m: &[[f64; 3]; 3]) -> f64 {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    #[inline]
    fn det3_replace_col(m: &[[f64; 3]; 3], col: &[f64; 3], c_idx: usize) -> f64 {
        let mut temp = *m;
        for r in 0..3 {
            temp[r][c_idx] = col[r];
        }
        Self::det3(&temp)
    }
}

/// Constructs a Duration- and Slope-Matched Basis Butterfly Spread.
pub struct BasisButterflyBuilder;

impl BasisButterflyBuilder {
    /// Computes weights [w1, w2, w3] for maturities [m1, m2, m3]
    /// such that Level Duration and Slope Duration are 0, with w2 = -1.0 (short body).
    pub fn build_butterfly(
        curve: &NelsonSiegelCurve,
        m1: f64,
        m2: f64,
        m3: f64,
    ) -> Option<[f64; 3]> {
        let d1 = curve.factor_durations(m1);
        let d2 = curve.factor_durations(m2);
        let d3 = curve.factor_durations(m3);

        // System:
        // d1[0] * w1 + d3[0] * w3 = -d2[0] * (-1.0) = d2[0]
        // d1[1] * w1 + d3[1] * w3 = -d2[1] * (-1.0) = d2[1]
        let a11 = d1[0];
        let a12 = d3[0];
        let b1 = d2[0];

        let a21 = d1[1];
        let a22 = d3[1];
        let b2 = d2[1];

        let det = a11 * a22 - a12 * a21;
        if det.abs() < 1e-12 {
            return None;
        }

        let w1 = (b1 * a22 - b2 * a12) / det;
        let w3 = (a11 * b2 - a21 * b1) / det;
        let w2 = -1.0;

        Some([w1, w2, w3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nelson_siegel_curve_yield_and_durations() {
        // Curve: beta0 = 0.05 (5% long-term floor), beta1 = 0.10 (perp at 15%), beta2 = 0.08 (quarterly hump), tau = 0.15 (~2 months)
        let curve = NelsonSiegelCurve::new(0.05, 0.10, 0.08, 0.15);

        // At m = 0: yield should equal beta0 + beta1 = 0.15 (15%)
        let y0 = curve.yield_at(0.0);
        assert!((y0 - 0.15).abs() < 1e-6);

        // At m = 0.25 (3 months):
        let y_3m = curve.yield_at(0.25);
        assert!(y_3m > 0.05 && y_3m < 0.20);

        // Factor durations at m = 0.25
        let dur = curve.factor_durations(0.25);
        assert!((dur[0] - 0.25).abs() < 1e-6); // D_beta0 = m
        assert!(dur[1] > 0.0 && dur[1] < 0.25); // D_beta1 < m
        assert!(dur[2] > 0.0); // D_beta2 hump duration

        // Roll down rate should be finite
        let roll = curve.roll_down_rate(0.25);
        assert!(roll.is_finite());
    }

    #[test]
    fn test_nelson_siegel_calibration() {
        let true_beta0 = 0.06;
        let true_beta1 = 0.08;
        let true_beta2 = 0.04;
        let tau = 0.20;

        let true_curve = NelsonSiegelCurve::new(true_beta0, true_beta1, true_beta2, tau);

        // Generate synthetic points
        let maturities = [0.0, 0.0833, 0.25, 0.50]; // Perp, 1M, 3M, 6M
        let yields: Vec<f64> = maturities.iter().map(|&m| true_curve.yield_at(m)).collect();
        let weights = [1.0, 1.0, 1.0, 1.0];

        let calib = NelsonSiegelCalibrator::calibrate(&maturities, &yields, &weights, tau)
            .expect("Calibration should succeed");

        assert!((calib.beta0 - true_beta0).abs() < 1e-4);
        assert!((calib.beta1 - true_beta1).abs() < 1e-4);
        assert!((calib.beta2 - true_beta2).abs() < 1e-4);
    }

    #[test]
    fn test_basis_butterfly_builder() {
        let curve = NelsonSiegelCurve::new(0.05, 0.08, 0.05, 0.20);
        let m1 = 0.0833; // 1M
        let m2 = 0.25; // 3M (body)
        let m3 = 0.50; // 6M

        let weights = BasisButterflyBuilder::build_butterfly(&curve, m1, m2, m3)
            .expect("Butterfly should be constructed");

        let w1 = weights[0];
        let w2 = weights[1];
        let w3 = weights[2];
        assert_eq!(w2, -1.0);

        // Check level duration neutrality: w1 * D0(m1) + w2 * D0(m2) + w3 * D0(m3) = 0
        let d1 = curve.factor_durations(m1);
        let d2 = curve.factor_durations(m2);
        let d3 = curve.factor_durations(m3);

        let net_d0 = w1 * d1[0] + w2 * d2[0] + w3 * d3[0];
        let net_d1 = w1 * d1[1] + w2 * d2[1] + w3 * d3[1];

        assert!(
            net_d0.abs() < 1e-6,
            "Net D0 duration must be zero: got {net_d0}"
        );
        assert!(
            net_d1.abs() < 1e-6,
            "Net D1 duration must be zero: got {net_d1}"
        );
    }
}
