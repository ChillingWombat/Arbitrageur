//! Cross-Chain Account Abstraction (ERC-4337 & ERC-7579) Modular Solvers.
//!
//! Implements an ERC-7579 modular execution smart account with scoped ephemeral `SessionKeyPolicy`:
//! - Time bounds restriction (e.g. 15-minute rolling window).
//! - Allowed target contract addresses whitelist.
//! - 4-byte function selector restriction.
//! - Cumulative spending limits in USD with pre/post invariant checks.
//! - Institutional private builder priority tip ceiling: Tip $\le 0.35 \cdot \text{GrossEdge}_{\text{arb}}$.

use serde::{Deserialize, Serialize};

/// Ephemeral session key delegation policy for ERC-7579 modular smart account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionKeyPolicy {
    pub session_key_address: String,
    pub valid_from_timestamp_sec: u64,
    pub valid_until_timestamp_sec: u64,
    pub allowed_targets: Vec<String>,
    pub allowed_selectors: Vec<[u8; 4]>,
    pub max_cumulative_spend_usd: f64,
    pub current_spent_usd: f64,
}

/// UserOperation execution request with pre/post invariant verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModularUserOpRequest {
    pub user_op_hash: String,
    pub target_contract: String,
    pub selector: [u8; 4],
    pub call_data: Vec<u8>,
    pub requested_spend_usd: f64,
    pub min_expected_out_usd: f64,
    pub max_priority_tip_usd: f64,
    pub gross_arbitrage_edge_usd: f64,
}

/// Result of UserOp policy validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserOpValidationVerdict {
    pub is_valid: bool,
    pub is_tip_capped: bool,
    pub allowed_priority_tip_usd: f64,
    pub remaining_spend_allowance_usd: f64,
    pub rejection_reason: Option<String>,
}

/// Modular Session Account Manager.
pub struct ModularSessionAccountManager;

impl ModularSessionAccountManager {
    /// Validates UserOp compliance against active SessionKeyPolicy and enforces 35% tip ceiling.
    pub fn validate_and_cap_user_op(
        policy: &mut SessionKeyPolicy,
        req: &ModularUserOpRequest,
        current_time_sec: u64,
    ) -> UserOpValidationVerdict {
        // 1. Time bounds check
        if current_time_sec < policy.valid_from_timestamp_sec
            || current_time_sec > policy.valid_until_timestamp_sec
        {
            return UserOpValidationVerdict {
                is_valid: false,
                is_tip_capped: false,
                allowed_priority_tip_usd: 0.0,
                remaining_spend_allowance_usd: policy.max_cumulative_spend_usd
                    - policy.current_spent_usd,
                rejection_reason: Some("SessionKeyExpiredOrNotYetValid".to_string()),
            };
        }

        // 2. Target contract whitelist check
        if !policy
            .allowed_targets
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&req.target_contract))
        {
            return UserOpValidationVerdict {
                is_valid: false,
                is_tip_capped: false,
                allowed_priority_tip_usd: 0.0,
                remaining_spend_allowance_usd: policy.max_cumulative_spend_usd
                    - policy.current_spent_usd,
                rejection_reason: Some("TargetContractNotWhitelisted".to_string()),
            };
        }

        // 3. Function selector whitelist check
        if !policy.allowed_selectors.contains(&req.selector) {
            return UserOpValidationVerdict {
                is_valid: false,
                is_tip_capped: false,
                allowed_priority_tip_usd: 0.0,
                remaining_spend_allowance_usd: policy.max_cumulative_spend_usd
                    - policy.current_spent_usd,
                rejection_reason: Some("FunctionSelectorNotWhitelisted".to_string()),
            };
        }

        // 4. Spending cap check
        let new_spend = policy.current_spent_usd + req.requested_spend_usd;
        if new_spend > policy.max_cumulative_spend_usd {
            return UserOpValidationVerdict {
                is_valid: false,
                is_tip_capped: false,
                allowed_priority_tip_usd: 0.0,
                remaining_spend_allowance_usd: (policy.max_cumulative_spend_usd
                    - policy.current_spent_usd)
                    .max(0.0),
                rejection_reason: Some("CumulativeSpendingCapExceeded".to_string()),
            };
        }

        // 5. Institutional priority tip ceiling: Tip <= 0.35 * GrossEdge
        let institutional_tip_cap = 0.35 * req.gross_arbitrage_edge_usd;
        let allowed_tip = req.max_priority_tip_usd.min(institutional_tip_cap).max(0.0);
        let is_tip_capped = req.max_priority_tip_usd > institutional_tip_cap;

        // Commit spend to policy
        policy.current_spent_usd = new_spend;

        UserOpValidationVerdict {
            is_valid: true,
            is_tip_capped,
            allowed_priority_tip_usd: allowed_tip,
            remaining_spend_allowance_usd: policy.max_cumulative_spend_usd
                - policy.current_spent_usd,
            rejection_reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modular_session_key_and_tip_cap() {
        let mut policy = SessionKeyPolicy {
            session_key_address: "0xsession".to_string(),
            valid_from_timestamp_sec: 1000,
            valid_until_timestamp_sec: 2000,
            allowed_targets: vec!["0xuniswap".to_string(), "0xacross".to_string()],
            allowed_selectors: vec![[0x12, 0x34, 0x56, 0x78]],
            max_cumulative_spend_usd: 10_000.0,
            current_spent_usd: 2_000.0,
        };

        let req = ModularUserOpRequest {
            user_op_hash: "0xhash".to_string(),
            target_contract: "0xuniswap".to_string(),
            selector: [0x12, 0x34, 0x56, 0x78],
            call_data: vec![],
            requested_spend_usd: 1_000.0,
            min_expected_out_usd: 995.0,
            max_priority_tip_usd: 50.0,
            gross_arbitrage_edge_usd: 100.0, // 35% cap is $35
        };

        let verdict =
            ModularSessionAccountManager::validate_and_cap_user_op(&mut policy, &req, 1500);
        assert!(verdict.is_valid);
        assert!(verdict.is_tip_capped);
        assert_eq!(verdict.allowed_priority_tip_usd, 35.0);
        assert_eq!(verdict.remaining_spend_allowance_usd, 7_000.0);
    }

    #[test]
    fn test_modular_session_key_rejections() {
        let mut policy = SessionKeyPolicy {
            session_key_address: "0xsession".to_string(),
            valid_from_timestamp_sec: 1000,
            valid_until_timestamp_sec: 2000,
            allowed_targets: vec!["0xuniswap".to_string()],
            allowed_selectors: vec![[0x12, 0x34, 0x56, 0x78]],
            max_cumulative_spend_usd: 10_000.0,
            current_spent_usd: 9_500.0,
        };

        // Time violation
        let req_time = ModularUserOpRequest {
            user_op_hash: "0x1".into(),
            target_contract: "0xuniswap".into(),
            selector: [0x12, 0x34, 0x56, 0x78],
            call_data: vec![],
            requested_spend_usd: 100.0,
            min_expected_out_usd: 95.0,
            max_priority_tip_usd: 10.0,
            gross_arbitrage_edge_usd: 50.0,
        };
        let v_time =
            ModularSessionAccountManager::validate_and_cap_user_op(&mut policy, &req_time, 2500);
        assert!(!v_time.is_valid);
        assert_eq!(
            v_time.rejection_reason,
            Some("SessionKeyExpiredOrNotYetValid".into())
        );

        // Spend cap violation
        let req_spend = ModularUserOpRequest {
            user_op_hash: "0x2".into(),
            target_contract: "0xuniswap".into(),
            selector: [0x12, 0x34, 0x56, 0x78],
            call_data: vec![],
            requested_spend_usd: 1_000.0,
            min_expected_out_usd: 950.0,
            max_priority_tip_usd: 10.0,
            gross_arbitrage_edge_usd: 50.0,
        };
        let v_spend =
            ModularSessionAccountManager::validate_and_cap_user_op(&mut policy, &req_spend, 1500);
        assert!(!v_spend.is_valid);
        assert_eq!(
            v_spend.rejection_reason,
            Some("CumulativeSpendingCapExceeded".into())
        );
    }
}
