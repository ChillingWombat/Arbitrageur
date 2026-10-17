//! Quantum Frobenius Maps, Divided Powers & Hall Algebras on Derived Hall Stacks of Liquidity Categories.
//!
//! Models discrete multi-exchange liquidity quantization via quantum groups $U_q(\mathfrak{sl}_2)$
//! at root of unity $q = e^{2\pi i / \ell}$, evaluating divided powers $E^{(n)} = \frac{E^n}{[n]_q!}$
//! and Lusztig's Quantum Frobenius map to detect discrete harmonic resonance in order flow cascades.

use serde::{Deserialize, Serialize};

/// Quantum group parameter and root of unity configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumGroupConfig {
    /// Root of unity order $\ell \ge 2$.
    pub root_order_ell: usize,
    /// Base quantum deformation phase angle $\theta = 2\pi / \ell$.
    pub phase_angle: f64,
}

impl QuantumGroupConfig {
    pub fn new(root_order_ell: usize) -> Self {
        let ell = root_order_ell.max(2);
        let phase = 2.0 * std::f64::consts::PI / ell as f64;
        Self {
            root_order_ell: ell,
            phase_angle: phase,
        }
    }
}

/// Evaluates the quantum integer $[n]_q = \frac{\sin(n \theta)}{\sin(\theta)}$ for $q = e^{i\theta}$.
pub fn quantum_integer(n: i64, theta: f64) -> f64 {
    let sin_theta = theta.sin();
    if sin_theta.abs() < 1e-12 {
        n as f64
    } else {
        (n as f64 * theta).sin() / sin_theta
    }
}

/// Evaluates the quantum factorial $[n]_q! = \prod_{k=1}^n [k]_q$.
pub fn quantum_factorial(n: usize, theta: f64) -> f64 {
    let mut prod = 1.0;
    for k in 1..=n {
        prod *= quantum_integer(k as i64, theta);
    }
    prod
}

/// Evaluates the quantum binomial (Gaussian coefficient) $\begin{bmatrix} n \\ k \end{bmatrix}_q = \frac{[n]_q!}{[k]_q! [n - k]_q!}$.
pub fn quantum_binomial(n: usize, k: usize, theta: f64) -> f64 {
    if k > n {
        return 0.0;
    }
    if k == 0 || k == n {
        return 1.0;
    }
    let num = quantum_factorial(n, theta);
    let den = quantum_factorial(k, theta) * quantum_factorial(n - k, theta);
    if den.abs() < 1e-12 {
        1.0
    } else {
        num / den
    }
}

/// State representation for a quantized order book channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedChannelState {
    pub channel_id: usize,
    pub venue_pair: (String, String),
    pub raw_order_depth: f64,
    pub discrete_quantum_power: usize,
}

/// Diagnostic result from the Quantum Frobenius evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumFrobeniusResult {
    pub channel_id: usize,
    pub quantum_divided_power_value: f64,
    pub classical_frobenius_image: f64,
    pub is_harmonic_resonance_detected: bool,
    pub resonance_amplification_factor: f64,
}

/// Quantum Frobenius & Divided Powers Liquidity Engine.
pub struct QuantumFrobeniusEngine {
    pub config: QuantumGroupConfig,
}

impl QuantumFrobeniusEngine {
    pub fn new(config: QuantumGroupConfig) -> Self {
        Self { config }
    }

    /// Evaluates divided power $E^{(n)} = \frac{E^n}{[n]_q!}$ and checks whether Lusztig's
    /// Quantum Frobenius map signals discrete block-cadence harmonic resonance.
    pub fn evaluate_divided_power_resonance(
        &self,
        state: &QuantizedChannelState,
    ) -> QuantumFrobeniusResult {
        let n = state.discrete_quantum_power;
        let ell = self.config.root_order_ell;
        let theta = self.config.phase_angle;

        let q_fac = quantum_factorial(n % ell, theta);
        let q_fac_safe = if q_fac.abs() < 1e-6 {
            1e-6 * q_fac.signum()
        } else {
            q_fac
        };

        // Divided power action on order depth
        let divided_power_val = state.raw_order_depth.powi(n as i32) / q_fac_safe;

        // Lusztig's Quantum Frobenius map Fr_q:
        // When n is a multiple of ell, Fr_q(E^{(k * ell)}) = E^k (survives to classical non-zero image)
        // Otherwise Fr_q maps to 0 in the classical limit
        let (classical_img, is_resonance, amp_factor) = if n >= ell && n.is_multiple_of(ell) {
            let k = n / ell;
            let classical = state.raw_order_depth.powi(k as i32);
            let amp = (divided_power_val / (classical.max(1e-6))).abs();
            (classical, true, amp)
        } else {
            (0.0, false, 1.0)
        };

        QuantumFrobeniusResult {
            channel_id: state.channel_id,
            quantum_divided_power_value: divided_power_val,
            classical_frobenius_image: classical_img,
            is_harmonic_resonance_detected: is_resonance,
            resonance_amplification_factor: amp_factor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantum_integer_and_binomial_properties() {
        // At ell = 4, theta = pi / 2
        let theta = std::f64::consts::PI / 2.0;
        // [1]_q = sin(pi/2) / sin(pi/2) = 1
        assert!((quantum_integer(1, theta) - 1.0).abs() < 1e-10);
        // [2]_q = sin(pi) / sin(pi/2) = 0
        assert!(quantum_integer(2, theta).abs() < 1e-10);
        // [3]_q = sin(3pi/2) / sin(pi/2) = -1
        assert!((quantum_integer(3, theta) - (-1.0)).abs() < 1e-10);

        // At general phase theta = 0.5, quantum binomial symmetry [n, k] = [n, n-k]
        let b1 = quantum_binomial(4, 1, 0.5);
        let b3 = quantum_binomial(4, 3, 0.5);
        assert!((b1 - b3).abs() < 1e-10);
    }

    #[test]
    fn test_quantum_frobenius_harmonic_resonance() {
        let config = QuantumGroupConfig::new(3); // ell = 3
        let engine = QuantumFrobeniusEngine::new(config);

        // Case 1: n = 2 (not a multiple of ell = 3) -> No resonance
        let state_normal = QuantizedChannelState {
            channel_id: 1,
            venue_pair: ("binance".into(), "hyperliquid".into()),
            raw_order_depth: 2.0,
            discrete_quantum_power: 2,
        };
        let res_norm = engine.evaluate_divided_power_resonance(&state_normal);
        assert!(!res_norm.is_harmonic_resonance_detected);
        assert_eq!(res_norm.classical_frobenius_image, 0.0);

        // Case 2: n = 3 (multiple of ell = 3) -> Harmonic resonance triggered
        let state_resonant = QuantizedChannelState {
            channel_id: 2,
            venue_pair: ("okx".into(), "deribit".into()),
            raw_order_depth: 2.0,
            discrete_quantum_power: 3,
        };
        let res_res = engine.evaluate_divided_power_resonance(&state_resonant);
        assert!(res_res.is_harmonic_resonance_detected);
        assert!(res_res.classical_frobenius_image > 0.0);
    }
}
