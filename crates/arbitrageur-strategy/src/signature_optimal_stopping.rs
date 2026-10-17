//! Non-Markovian Signature Optimal Stopping & randomized Longstaff-Schwartz Continuation Engine (Tranche 75).
//!
//! Classical optimal stopping rules (e.g., Ornstein-Uhlenbeck threshold rules) assume the spread
//! process $X_t$ is Markovian, so the continuation value $C_t = \mathbb{E}[\sup_{\tau > t} G_\tau \mid \mathcal{F}_t]$
//! depends only on the current level $X_t$.
//!
//! In live high-frequency spatial and funding basis arbitrage, spread trajectories exhibit strong
//! non-Markovian path dependence (fractional memory $H \ne 0.5$, Hawkes self-excitation, and
//! momentum exhaustion). By the **Universal Approximation Theorem on Path Space** (Bayer, Hager,
//! Riedel, Schoenmakers 2023; Lyons 2014), any continuous path-dependent functional on compact sets
//! of rough paths can be uniformly approximated by a linear functional of the **Truncated Path Signature**:
//! $$C_t(\mathbb{X}_{[0, t]}) \approx \langle \ell, \, \mathbb{X}_{[0, t]}^{\le N} \rangle = \sum_{|I| \le N} \ell_I \, S^I(\mathbb{X}_{[0, t]})$$
//!
//! Furthermore, by the **Shuffle Product Identity**:
//! $$S^I(\mathbb{X}) \cdot S^J(\mathbb{X}) = \sum_{K \in I \shuffle J} S^K(\mathbb{X})$$
//! polynomial basis functions of the path signature reduce to linear combinations of higher-order
//! signature coordinates!
//!
//! This module implements a streaming 2D time-augmented path signature optimal stopping engine
//! over $Z_t = (t / T_{\max}, X_t)$ truncated at level $N = 3$ ($1 + 2 + 4 + 8 = 15$ signature features)
//! with Ridge-regularized Backward Induction (Signature Longstaff-Schwartz) to decide whether to
//! `HarvestNow` or `ContinueHolding` an open arbitrage position.

use serde::{Deserialize, Serialize};

/// Dimension of the 2D time-augmented path signature up to truncation level $N = 3$:
/// $1 + 2 + 4 + 8 = 15$.
pub const SIG_STOPPING_DIM: usize = 15;

/// Truncated Level-3 Path Signature of a 2D time-augmented spread trajectory $Z_t = (\tau_t, x_t)$.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeAugmentedSpreadSignature {
    /// Coordinate vector $[1, S^{(1)}, S^{(2)}, S^{(1,1)}, S^{(1,2)}, S^{(2,1)}, S^{(2,2)}, S^{(1,1,1)}, \dots] \in \mathbb{R}^{15}$.
    pub coords: [f64; SIG_STOPPING_DIM],
    pub last_tau: f64,
    pub last_spread_bps: f64,
}

impl TimeAugmentedSpreadSignature {
    /// Initializes the trivial signature at the origin $(\tau_0, x_0)$.
    pub fn new(tau_0: f64, spread_bps_0: f64) -> Self {
        let mut coords = [0.0; SIG_STOPPING_DIM];
        coords[0] = 1.0;
        Self {
            coords,
            last_tau: tau_0,
            last_spread_bps: spread_bps_0,
        }
    }

    /// Updates the signature online with a new observation $(\tau, x)$ via Chen's identity:
    /// $\mathbb{S}_{0, t} = \mathbb{S}_{0, s} \otimes \exp(\Delta Z_{s, t})$.
    #[allow(clippy::needless_range_loop)]
    pub fn push_observation(&mut self, tau: f64, spread_bps: f64) {
        if !tau.is_finite() || !spread_bps.is_finite() {
            return;
        }
        let d1 = tau - self.last_tau;
        let d2 = spread_bps - self.last_spread_bps;
        self.last_tau = tau;
        self.last_spread_bps = spread_bps;

        // Segment exponential signature \exp(\Delta Z) up to level 3
        let e_1 = [d1, d2];
        let e_2 = [0.5 * d1 * d1, 0.5 * d1 * d2, 0.5 * d2 * d1, 0.5 * d2 * d2];
        let mut e_3 = [0.0; 8];
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    e_3[i * 4 + j * 2 + k] = (e_1[i] * e_1[j] * e_1[k]) / 6.0;
                }
            }
        }

        let s1_old = [self.coords[1], self.coords[2]];
        let mut s2_old = [0.0; 4];
        s2_old.copy_from_slice(&self.coords[3..7]);
        let mut s3_old = [0.0; 8];
        s3_old.copy_from_slice(&self.coords[7..15]);

        // Level 3 update: S_3^{ijk} = S_{3,old}^{ijk} + S_{2,old}^{ij} e_1^k + S_{1,old}^i e_2^{jk} + e_3^{ijk}
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    let idx3 = i * 4 + j * 2 + k;
                    let idx2_ij = i * 2 + j;
                    let idx2_jk = j * 2 + k;
                    self.coords[7 + idx3] = s3_old[idx3]
                        + s2_old[idx2_ij] * e_1[k]
                        + s1_old[i] * e_2[idx2_jk]
                        + e_3[idx3];
                }
            }
        }

        // Level 2 update: S_2^{ij} = S_{2,old}^{ij} + S_{1,old}^i e_1^j + e_2^{ij}
        for i in 0..2 {
            for j in 0..2 {
                let idx2 = i * 2 + j;
                self.coords[3 + idx2] = s2_old[idx2] + s1_old[i] * e_1[j] + e_2[idx2];
            }
        }

        // Level 1 update
        self.coords[1] = s1_old[0] + e_1[0];
        self.coords[2] = s1_old[1] + e_1[1];
    }

    /// Computes the Lévy area $A_{1,2} = \frac{1}{2}(S^{(1,2)} - S^{(2,1)})$ measuring
    /// whether the spread compressed early vs late along the holding horizon.
    #[inline(always)]
    pub fn levy_area(&self) -> f64 {
        0.5 * (self.coords[4] - self.coords[5])
    }
}

/// Optimal stopping decision for an open convergence / basis position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureStoppingAction {
    /// Immediate exercise value exceeds signature continuation value ($G_t \ge \hat{C}_t$).
    HarvestNow,
    /// Signature continuation value exceeds immediate exercise ($G_t < \hat{C}_t$).
    ContinueHolding,
    /// Emergency stop-loss or horizon deadline breached.
    EmergencyExit,
}

/// Detailed evaluation of the Signature Optimal Stopping rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureStoppingVerdict {
    pub action: SignatureStoppingAction,
    /// Immediate net exercise payoff $G_t$ in basis points.
    pub immediate_payoff_bps: f64,
    /// Signature-predicted continuation value $\hat{C}_t(\mathbb{X}_{[0, t]})$ in basis points.
    pub continuation_value_bps: f64,
    /// Exercise advantage $G_t - \hat{C}_t$ in basis points.
    pub exercise_advantage_bps: f64,
    /// Path Lévy area $A_{1,2}$ capturing non-Markovian trajectory convexity.
    pub trajectory_levy_area: f64,
}

/// Non-Markovian Signature Optimal Stopping Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureOptimalStoppingEngine {
    /// Linear functional weights $\ell \in \mathbb{R}^{15}$ mapping truncated signature to continuation value.
    pub continuation_weights: [f64; SIG_STOPPING_DIM],
    /// Entry spread in basis points when the arbitrage position was opened.
    pub entry_spread_bps: f64,
    /// Round-trip exit transaction cost in basis points.
    pub exit_fee_bps: f64,
    /// Hard adverse excursion stop-loss threshold in basis points.
    pub max_adverse_loss_bps: f64,
}

impl SignatureOptimalStoppingEngine {
    /// Constructs a calibrated Signature Optimal Stopping Engine.
    pub fn new(
        entry_spread_bps: f64,
        exit_fee_bps: f64,
        max_adverse_loss_bps: f64,
        continuation_weights: [f64; SIG_STOPPING_DIM],
    ) -> Self {
        Self {
            continuation_weights,
            entry_spread_bps,
            exit_fee_bps: exit_fee_bps.max(0.0),
            max_adverse_loss_bps: max_adverse_loss_bps.max(1.0),
        }
    }

    /// Default calibrated signature functional for mean-reverting + momentum-exhaustion spreads:
    /// - Positive baseline continuation that decays as normalized time $S^{(1)} = \tau \to 1$,
    /// - Increases when remaining spread is still wide ($S^{(2)}$),
    /// - Decreases when negative Lévy area / third-order signature indicates convergence velocity has stalled.
    pub fn default_calibrated(
        entry_spread_bps: f64,
        exit_fee_bps: f64,
        max_adverse_loss_bps: f64,
    ) -> Self {
        let mut w = [0.0; SIG_STOPPING_DIM];
        let max_net = (entry_spread_bps - exit_fee_bps).max(0.0);
        w[0] = 0.82 * max_net; // Baseline expected capture at t=0
        w[1] = -0.75 * max_net; // Time decay as \tau -> 1
        w[2] = -0.35; // If spread narrows (dX < 0), remaining future upside shrinks
        w[4] = 0.40; // S^{(1,2)}: positive when spread stays wide early
        w[5] = -0.40; // S^{(2,1)}: negative Levy area adjustment
        w[14] = -0.05; // S^{(2,2,2)}: cubic jump exhaustion dampening
        Self::new(entry_spread_bps, exit_fee_bps, max_adverse_loss_bps, w)
    }

    /// Evaluates the linear signature functional $\langle \ell, \mathbb{S}_{0, t} \rangle$.
    #[inline]
    pub fn predict_continuation_value(&self, sig: &TimeAugmentedSpreadSignature) -> f64 {
        let mut val = 0.0;
        for (w, &s) in self.continuation_weights.iter().zip(sig.coords.iter()) {
            val += w * s;
        }
        // Continuation value vanishes at terminal horizon \tau >= 1.0
        let time_remaining_factor = (1.0 - sig.last_tau).clamp(0.0, 1.0);
        let raw_continuation = (val * time_remaining_factor).max(0.0);
        // No-arbitrage physical upper bound: remaining future payoff cannot exceed remaining uncompressed spread
        raw_continuation.min(sig.last_spread_bps.max(0.0))
    }

    /// Evaluates whether to harvest the open convergence position now or continue holding.
    pub fn evaluate_stopping(
        &self,
        sig: &TimeAugmentedSpreadSignature,
    ) -> SignatureStoppingVerdict {
        // Immediate payoff from closing the spread: (entry_spread - current_spread) - exit_fee
        let captured_spread_bps = self.entry_spread_bps - sig.last_spread_bps;
        let immediate_payoff_bps = captured_spread_bps - self.exit_fee_bps;
        let continuation_value_bps = self.predict_continuation_value(sig);
        let advantage = immediate_payoff_bps - continuation_value_bps;

        let action = if immediate_payoff_bps <= -self.max_adverse_loss_bps || sig.last_tau >= 1.0 {
            SignatureStoppingAction::EmergencyExit
        } else if immediate_payoff_bps > 0.0 && immediate_payoff_bps >= continuation_value_bps {
            SignatureStoppingAction::HarvestNow
        } else {
            SignatureStoppingAction::ContinueHolding
        };

        SignatureStoppingVerdict {
            action,
            immediate_payoff_bps,
            continuation_value_bps,
            exercise_advantage_bps: advantage,
            trajectory_levy_area: sig.levy_area(),
        }
    }

    /// Calibrates the signature continuation weights $\ell \in \mathbb{R}^{15}$ via Ridge-regularized
    /// Longstaff-Schwartz backward regression from historical `(signature, realized_max_future_payoff)` pairs.
    #[allow(clippy::needless_range_loop)]
    pub fn fit_continuation_weights(
        &mut self,
        samples: &[(TimeAugmentedSpreadSignature, f64)],
        ridge_lambda: f64,
    ) {
        if samples.len() < SIG_STOPPING_DIM {
            return;
        }
        let mut a = [[0.0; SIG_STOPPING_DIM]; SIG_STOPPING_DIM];
        let mut b = [0.0; SIG_STOPPING_DIM];

        for (sig, future_payoff) in samples {
            for i in 0..SIG_STOPPING_DIM {
                b[i] += sig.coords[i] * (*future_payoff);
                for j in 0..SIG_STOPPING_DIM {
                    a[i][j] += sig.coords[i] * sig.coords[j];
                }
            }
        }

        for (i, row) in a.iter_mut().enumerate() {
            row[i] += if i == 0 { 1e-8 } else { ridge_lambda.max(1e-5) };
        }

        // Gauss-Jordan elimination
        for col in 0..SIG_STOPPING_DIM {
            let mut pivot_row = col;
            let mut max_val = a[col][col].abs();
            for (r, row) in a.iter().enumerate().skip(col + 1) {
                if row[col].abs() > max_val {
                    max_val = row[col].abs();
                    pivot_row = r;
                }
            }
            if max_val < 1e-12 {
                continue;
            }
            if pivot_row != col {
                a.swap(col, pivot_row);
                b.swap(col, pivot_row);
            }
            let pivot = a[col][col];
            for c in col..SIG_STOPPING_DIM {
                a[col][c] /= pivot;
            }
            b[col] /= pivot;
            for r in 0..SIG_STOPPING_DIM {
                if r != col {
                    let factor = a[r][col];
                    if factor.abs() > 1e-15 {
                        for c in col..SIG_STOPPING_DIM {
                            a[r][c] -= factor * a[col][c];
                        }
                        b[r] -= factor * b[col];
                    }
                }
            }
        }

        self.continuation_weights = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_markovian_levy_area_distinguishes_paths_with_identical_endpoints() {
        // Two paths starting at (\tau=0, X=30 bps) and ending at (\tau=0.5, X=10 bps):
        // Path A: drops immediately to 10 bps at \tau=0.1, then stalls flat at 10 bps until \tau=0.5.
        // Path B: stays wide at 30 bps until \tau=0.4, then plunges rapidly to 10 bps at \tau=0.5.
        // Both have identical Markovian state (\tau=0.5, X=10 bps), but distinct Levy areas!
        let mut sig_a = TimeAugmentedSpreadSignature::new(0.0, 30.0);
        sig_a.push_observation(0.1, 10.0);
        sig_a.push_observation(0.5, 10.0);

        let mut sig_b = TimeAugmentedSpreadSignature::new(0.0, 30.0);
        sig_b.push_observation(0.4, 30.0);
        sig_b.push_observation(0.5, 10.0);

        // Endpoint levels and Level-1 increments are identical:
        assert!((sig_a.coords[1] - sig_b.coords[1]).abs() < 1e-12);
        assert!((sig_a.coords[2] - sig_b.coords[2]).abs() < 1e-12);

        // Yet their Levy areas A_{1,2} have opposite signs!
        assert!(
            (sig_a.levy_area() - sig_b.levy_area()).abs() > 2.0,
            "Levy area must distinguish fast-then-stalled vs slow-then-fast paths"
        );
    }

    #[test]
    fn test_signature_optimal_stopping_harvest_vs_continue() {
        let engine = SignatureOptimalStoppingEngine::default_calibrated(30.0, 4.0, 25.0);

        // At t=0.05 with spread barely moved (28 bps), immediate payoff is -2 bps -> ContinueHolding
        let mut sig = TimeAugmentedSpreadSignature::new(0.0, 30.0);
        sig.push_observation(0.05, 28.0);
        let v1 = engine.evaluate_stopping(&sig);
        assert_eq!(v1.action, SignatureStoppingAction::ContinueHolding);

        // At t=0.65 with spread converged to 2.0 bps, immediate payoff = (30 - 2) - 4 = 24 bps > continuation -> HarvestNow
        sig.push_observation(0.65, 2.0);
        let v2 = engine.evaluate_stopping(&sig);
        assert_eq!(v2.action, SignatureStoppingAction::HarvestNow);
        assert!((v2.immediate_payoff_bps - 24.0).abs() < 1e-9);
        assert!(v2.exercise_advantage_bps > 0.0);
    }
}
