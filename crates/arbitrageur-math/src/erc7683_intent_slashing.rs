//! ERC-7683 Bonded Solver Slashing & Volatility Hook Router.
//!
//! Enforces bonded solver commitments (1.25x collateral, 0.05x indemnity payout on timeout),
//! dynamic Uniswap V4 hook volatility fees, and concave Dutch auction solver reward sharing.

use serde::{Deserialize, Serialize};

/// Result of ERC-7683 solver bond and slashing evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverSlashingVerdict {
    pub is_bond_sufficient: bool,
    pub solver_bond_usd: f64,
    pub required_bond_usd: f64,
    pub is_timed_out: bool,
    pub indemnity_payout_usd: f64,
    pub net_refund_usd: f64,
}

/// ERC-7683 Bonded Solver Slashing & Volatility Hook Router.
pub struct Erc7683SlashingManager;

impl Erc7683SlashingManager {
    /// Evaluates bonded solver compliance and timeout slashing payouts.
    pub fn evaluate_slashing(
        notional_usd: f64,
        solver_bond_usd: f64,
        elapsed_sec: f64,
        deadline_sec: f64,
        filled: bool,
    ) -> SolverSlashingVerdict {
        let required_bond = 1.25 * notional_usd;
        let is_bond_sufficient = solver_bond_usd >= required_bond;
        let is_timed_out = !filled && (elapsed_sec > deadline_sec);

        let (indemnity_payout_usd, net_refund_usd) = if is_timed_out && is_bond_sufficient {
            (0.05 * notional_usd, notional_usd)
        } else {
            (0.0, if filled { notional_usd } else { 0.0 })
        };

        SolverSlashingVerdict {
            is_bond_sufficient,
            solver_bond_usd,
            required_bond_usd: required_bond,
            is_timed_out,
            indemnity_payout_usd,
            net_refund_usd,
        }
    }

    /// Evaluates Uniswap V4 hook dynamic swap fee based on realized volatility.
    pub fn compute_v4_hook_fee_bps(base_fee_bps: f64, realized_vol: f64, vol_scale: f64) -> f64 {
        base_fee_bps + (vol_scale * realized_vol * 100.0).max(0.0)
    }

    /// Computes optimal Dutch auction solver reward sharing ratio theta_fill*.
    pub fn compute_optimal_reward_sharing(lambda_solver: f64, lambda_sequencer: f64) -> f64 {
        let den = lambda_solver + lambda_sequencer;
        if den <= 0.0 {
            0.50
        } else {
            (lambda_solver / den).clamp(0.10, 0.90)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erc7683_slashing_and_v4_hook_fee() {
        let verdict =
            Erc7683SlashingManager::evaluate_slashing(10_000.0, 12_500.0, 45.0, 30.0, false);
        assert!(verdict.is_bond_sufficient);
        assert!(verdict.is_timed_out);
        assert_eq!(verdict.indemnity_payout_usd, 500.0); // 0.05 * 10k
        assert_eq!(verdict.net_refund_usd, 10_000.0);

        let fee = Erc7683SlashingManager::compute_v4_hook_fee_bps(5.0, 0.60, 0.10);
        assert!(fee > 5.0);

        let share = Erc7683SlashingManager::compute_optimal_reward_sharing(0.6, 0.4);
        assert!((share - 0.60).abs() < 1e-6);
    }
}
