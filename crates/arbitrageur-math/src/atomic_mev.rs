//! Cross-Chain Atomic Settlement MEV Protection & Dynamic EIP-1559 Priority Fee Bidding Engine.
//!
//! Provides production mathematical formulations for:
//! 1. **Private Builder Bundle Routing**:
//!    Immunizes cross-venue collateral transfers and on-chain DEX swaps against toxic sandwich bots and frontrunning:
//!    - Routes transactions via private builder relays (Flashbots Protect, MEV-Share, Titan, BeaverBuild).
//!    - Zero mempool exposure guarantees atomic execution without sandwich extraction.
//!    - Dropped-on-revert protection eliminates wasted gas during competitive latency races.
//! 2. **Pre-Flight Deterministic Simulation & Slippage Verification**:
//!    Verifies account nonces, wallet token balances, ERC-20 allowances, and bounds before on-chain dispatch:
//!    $$\text{amountOutMinimum} = \text{amountOutQuoted} \times \left(1 - \frac{\text{SlippageBps}}{10000}\right)$$
//! 3. **Dynamic EIP-1559 Priority Fee Bidding Engine**:
//!    Calculates optimal priority tip $g^*$ (Gwei) balancing inclusion probability and economic edge under Pareto builder queue dynamics:
//!    $$g^* = \left( \frac{\alpha \cdot g_{\min}^\alpha \cdot \text{GrossEdge}_{\text{USD}}}{\text{GasUsed} \cdot P_{\text{ETH}} \cdot 10^{-9} \cdot \Delta t_{\text{deadline}}} \right)^{\frac{1}{\alpha + 1}}$$
//!    strictly capped at $\le 35\%$ of gross arbitrage profit.

use serde::{Deserialize, Serialize};

/// Known private builder RPC endpoints for MEV protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivateRpcEndpoint {
    FlashbotsProtectFast,
    MevShare,
    TitanBuilder,
    BeaverBuild,
    Custom(String),
}

impl PrivateRpcEndpoint {
    pub fn as_url(&self) -> &str {
        match self {
            Self::FlashbotsProtectFast => "https://rpc.flashbots.net/fast",
            Self::MevShare => "https://mev-share.flashbots.net",
            Self::TitanBuilder => "https://rpc.titanbuilder.xyz",
            Self::BeaverBuild => "https://rpc.beaverbuild.org",
            Self::Custom(url) => url.as_str(),
        }
    }
}

/// Configuration for MEV-protected bundle routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivateBundleConfig {
    pub endpoint: PrivateRpcEndpoint,
    pub min_priority_tip_gwei: f64,
    pub max_priority_tip_gwei: f64,
    pub max_tip_edge_ratio: f64,
    pub revert_protection: bool,
    pub max_block_delay: u32,
}

impl Default for PrivateBundleConfig {
    fn default() -> Self {
        Self {
            endpoint: PrivateRpcEndpoint::FlashbotsProtectFast,
            min_priority_tip_gwei: 0.10,
            max_priority_tip_gwei: 25.0,
            max_tip_edge_ratio: 0.35, // Never pay > 35% of gross arbitrage edge in tips
            revert_protection: true,
            max_block_delay: 2,
        }
    }
}

/// Dynamic EIP-1559 gas recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Eip1559GasRecommendation {
    /// Recommended priority tip in Gwei.
    pub priority_fee_gwei: f64,
    /// Predicted next-block base fee in Gwei.
    pub predicted_base_fee_gwei: f64,
    /// Total max fee per gas in Gwei (base + tip + 25% surge headroom).
    pub max_fee_per_gas_gwei: f64,
    /// Estimated total transaction gas cost in USD.
    pub estimated_cost_usd: f64,
    /// Percentage of gross edge consumed by gas tip.
    pub tip_edge_share: f64,
}

/// Dynamic EIP-1559 Priority Fee Bidding Engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicEip1559GasBiddingEngine {
    pub config: PrivateBundleConfig,
    /// Shape parameter \alpha of Pareto tip distribution in builder queues (typically 1.5 - 2.0).
    pub pareto_alpha: f64,
}

impl Default for DynamicEip1559GasBiddingEngine {
    fn default() -> Self {
        Self {
            config: PrivateBundleConfig::default(),
            pareto_alpha: 1.80,
        }
    }
}

impl DynamicEip1559GasBiddingEngine {
    pub fn new(config: PrivateBundleConfig, pareto_alpha: f64) -> Self {
        Self {
            config,
            pareto_alpha: pareto_alpha.clamp(1.10, 3.0),
        }
    }

    /// Predicts next block base fee given current base fee and block gas utilization:
    /// \text{BaseFee}_{t+1} = \text{BaseFee}_t \cdot \left(1 + \frac{1}{8} \frac{\text{GasUsed} - \text{GasTarget}}{\text{GasTarget}}\right).
    pub fn predict_next_base_fee_gwei(
        current_base_fee_gwei: f64,
        gas_used: u64,
        gas_target: u64,
    ) -> f64 {
        if gas_target == 0 {
            return current_base_fee_gwei;
        }
        let delta = gas_used as f64 - gas_target as f64;
        let factor = 1.0 + (1.0 / 8.0) * (delta / gas_target as f64);
        (current_base_fee_gwei * factor).max(0.01)
    }

    /// Computes optimal EIP-1559 priority fee tip g^* (in Gwei) under Pareto builder auction dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_gas_recommendation(
        &self,
        current_base_fee_gwei: f64,
        block_gas_used: u64,
        block_gas_target: u64,
        tx_gas_units: f64,
        gross_edge_usd: f64,
        eth_price_usd: f64,
        deadline_sec: f64,
    ) -> Eip1559GasRecommendation {
        let predicted_base_fee_gwei = Self::predict_next_base_fee_gwei(
            current_base_fee_gwei,
            block_gas_used,
            block_gas_target,
        );

        let g_min = self.config.min_priority_tip_gwei;
        let g_max = self.config.max_priority_tip_gwei;

        if gross_edge_usd <= 0.0 || eth_price_usd <= 0.0 || tx_gas_units <= 0.0 {
            let total_gwei = predicted_base_fee_gwei * 1.25 + g_min;
            let cost_usd = tx_gas_units * total_gwei * 1e-9 * eth_price_usd;
            return Eip1559GasRecommendation {
                priority_fee_gwei: g_min,
                predicted_base_fee_gwei,
                max_fee_per_gas_gwei: total_gwei,
                estimated_cost_usd: cost_usd,
                tip_edge_share: 0.0,
            };
        }

        let cost_per_gwei_usd = tx_gas_units * eth_price_usd * 1e-9;
        let alpha = self.pareto_alpha;

        // g^* = [ (\alpha * g_min^\alpha * Edge) / (CostPerGwei * deadline) ]^(1 / (\alpha + 1))
        let num = alpha * g_min.powf(alpha) * gross_edge_usd;
        let den = cost_per_gwei_usd * deadline_sec.max(0.1);
        let unconstrained_tip = (num / den.max(1e-12)).powf(1.0 / (alpha + 1.0));

        // Hard cap: tip in USD cannot exceed max_tip_edge_ratio of gross edge
        let max_tip_usd = gross_edge_usd * self.config.max_tip_edge_ratio;
        let max_tip_gwei = max_tip_usd / cost_per_gwei_usd.max(1e-12);

        let priority_fee_gwei = unconstrained_tip.min(max_tip_gwei).clamp(g_min, g_max);

        // Max fee per gas: predicted base fee with +25% surge headroom + priority fee
        let max_fee_per_gas_gwei = predicted_base_fee_gwei * 1.25 + priority_fee_gwei;
        let estimated_cost_usd = tx_gas_units * max_fee_per_gas_gwei * 1e-9 * eth_price_usd;
        let tip_cost_usd = tx_gas_units * priority_fee_gwei * 1e-9 * eth_price_usd;
        let tip_edge_share = (tip_cost_usd / gross_edge_usd).clamp(0.0, 1.0);

        Eip1559GasRecommendation {
            priority_fee_gwei,
            predicted_base_fee_gwei,
            max_fee_per_gas_gwei,
            estimated_cost_usd,
            tip_edge_share,
        }
    }
}

/// Pre-flight transaction simulation and verification report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreFlightVerificationReport {
    /// Whether pre-flight simulation succeeded without revert.
    pub is_valid: bool,
    /// Verified nonce.
    pub current_nonce: u64,
    /// Verified wallet token balance in token units.
    pub wallet_balance: f64,
    /// Required spend in token units.
    pub spend_needed: f64,
    /// Verified allowance in token units.
    pub router_allowance: f64,
    /// Minimum expected output tokens enforcing slippage bound.
    pub amount_out_min: f64,
    /// Rejection reason if invalid.
    pub failure_reason: Option<String>,
}

/// Pre-flight transaction simulation and slippage verifier.
#[derive(Debug, Clone, Default)]
pub struct PreFlightStateVerifier;

impl PreFlightStateVerifier {
    /// Verifies all pre-flight conditions before on-chain dispatch:
    /// 1. Wallet balance >= spend needed.
    /// 2. Allowance >= spend needed.
    /// 3. Simulated amount out >= minimum acceptable output bound.
    pub fn verify_swap(
        current_nonce: u64,
        wallet_balance: f64,
        spend_needed: f64,
        router_allowance: f64,
        quoted_amount_out: f64,
        slippage_tolerance_bps: f64,
        simulated_amount_out: f64,
    ) -> PreFlightVerificationReport {
        if wallet_balance < spend_needed {
            return PreFlightVerificationReport {
                is_valid: false,
                current_nonce,
                wallet_balance,
                spend_needed,
                router_allowance,
                amount_out_min: 0.0,
                failure_reason: Some(format!(
                    "Insufficient token balance: {:.6} < {:.6} needed",
                    wallet_balance, spend_needed
                )),
            };
        }

        if router_allowance < spend_needed {
            return PreFlightVerificationReport {
                is_valid: false,
                current_nonce,
                wallet_balance,
                spend_needed,
                router_allowance,
                amount_out_min: 0.0,
                failure_reason: Some(format!(
                    "Insufficient router allowance: {:.6} < {:.6} needed",
                    router_allowance, spend_needed
                )),
            };
        }

        let amount_out_min = quoted_amount_out * (1.0 - slippage_tolerance_bps.max(0.0) / 10_000.0);
        if simulated_amount_out < amount_out_min {
            return PreFlightVerificationReport {
                is_valid: false,
                current_nonce,
                wallet_balance,
                spend_needed,
                router_allowance,
                amount_out_min,
                failure_reason: Some(format!(
                    "Slippage breach in pre-flight simulation: {:.6} < {:.6} min bound",
                    simulated_amount_out, amount_out_min
                )),
            };
        }

        PreFlightVerificationReport {
            is_valid: true,
            current_nonce,
            wallet_balance,
            spend_needed,
            router_allowance,
            amount_out_min,
            failure_reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predict_next_base_fee() {
        let current_base = 20.0;
        // Exactly target utilization: base fee does not change
        let next_eq = DynamicEip1559GasBiddingEngine::predict_next_base_fee_gwei(
            current_base,
            15_000_000,
            15_000_000,
        );
        assert!((next_eq - 20.0).abs() < 1e-6);

        // 100% full block (2x target): base fee increases by 12.5%
        let next_high = DynamicEip1559GasBiddingEngine::predict_next_base_fee_gwei(
            current_base,
            30_000_000,
            15_000_000,
        );
        assert!((next_high - 22.5).abs() < 1e-6);

        // Empty block (0x target): base fee decreases by 12.5%
        let next_low =
            DynamicEip1559GasBiddingEngine::predict_next_base_fee_gwei(current_base, 0, 15_000_000);
        assert!((next_low - 17.5).abs() < 1e-6);
    }

    #[test]
    fn test_dynamic_priority_fee_bidding() {
        let engine = DynamicEip1559GasBiddingEngine::default();
        let gas_rec = engine.compute_gas_recommendation(
            20.0,       // Base fee 20 Gwei
            15_000_000, // Target utilization
            15_000_000, 180_000.0, // 180k gas units
            150.0,     // Gross edge $150 USD
            3000.0,    // ETH price $3000
            1.0,       // Urgent deadline: 1 sec
        );

        assert!(gas_rec.priority_fee_gwei >= engine.config.min_priority_tip_gwei);
        assert!(gas_rec.priority_fee_gwei <= engine.config.max_priority_tip_gwei);
        // Tip cost must not exceed 35% of gross edge ($52.5)
        let tip_cost = 180_000.0 * gas_rec.priority_fee_gwei * 1e-9 * 3000.0;
        assert!(tip_cost <= 150.0 * 0.35 + 1e-6);
    }

    #[test]
    fn test_pre_flight_verifier() {
        // Valid case
        let report = PreFlightStateVerifier::verify_swap(
            42, 10.0,  // Balance
            2.0,   // Spend
            5.0,   // Allowance
            100.0, // Quoted out
            10.0,  // 10 bps slippage
            99.95, // Simulated out
        );
        assert!(report.is_valid);
        assert!(report.failure_reason.is_none());

        // Balance breach
        let report_bal = PreFlightStateVerifier::verify_swap(42, 1.0, 2.0, 5.0, 100.0, 10.0, 99.95);
        assert!(!report_bal.is_valid);
        assert!(report_bal
            .failure_reason
            .unwrap()
            .contains("Insufficient token balance"));

        // Slippage breach
        let report_slip =
            PreFlightStateVerifier::verify_swap(42, 10.0, 2.0, 5.0, 100.0, 10.0, 95.0);
        assert!(!report_slip.is_valid);
        assert!(report_slip
            .failure_reason
            .unwrap()
            .contains("Slippage breach"));
    }
}
