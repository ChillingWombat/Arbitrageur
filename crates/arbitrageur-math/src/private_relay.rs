//! Private Builder Relay Auctions & MEV-Resistant Cross-Chain Transaction Submission Engine.
//!
//! Models Pareto priority gas auction bidding for on-chain DEX swaps and cross-chain bridge deposits
//! routed to private block builders (Flashbots Protect, Titan Builder, BeaverBuild, MEV-Share)
//! with strictly enforced institutional invariant: Total Gas Cost <= 35% of Gross Arbitrage Profit.

use serde::{Deserialize, Serialize};

/// Detailed recommendation for private builder relay transaction submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivateRelayRecommendation {
    /// Optimal EIP-1559 priority fee tip in Gwei.
    pub recommended_priority_fee_gwei: f64,
    /// Total estimated transaction gas cost in USD.
    pub total_gas_cost_usd: f64,
    /// Gross anticipated arbitrage edge or rebalance value in USD.
    pub gross_arbitrage_profit_usd: f64,
    /// Fraction of gross edge consumed by gas fees (strictly <= 0.35).
    pub gas_cost_profit_share: f64,
    /// Whether the transaction complies with institutional gas ceiling.
    pub is_institutionally_compliant: bool,
    /// Target private builder RPC relay endpoints.
    pub target_relay_endpoints: Vec<String>,
}

/// Private Builder Relay Gas Bidding Engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrivateBuilderMevRelayEngine {
    /// Pareto queue tail index alpha (typically 1.5 - 2.0; default 1.75).
    pub pareto_alpha: f64,
    /// Institutional gas cost ceiling fraction (strictly 0.35, i.e. 35%).
    pub max_gas_share_ceiling: f64,
    /// Minimum priority fee floor in Gwei.
    pub min_priority_tip_gwei: f64,
}

impl Default for PrivateBuilderMevRelayEngine {
    fn default() -> Self {
        Self {
            pareto_alpha: 1.75,
            max_gas_share_ceiling: 0.35, // Institutional invariant
            min_priority_tip_gwei: 0.15, // 0.15 Gwei baseline priority tip
        }
    }
}

impl PrivateBuilderMevRelayEngine {
    /// Constructs a new engine with custom parameters.
    pub fn new(pareto_alpha: f64, max_gas_share_ceiling: f64, min_priority_tip_gwei: f64) -> Self {
        Self {
            pareto_alpha: pareto_alpha.clamp(1.1, 3.0),
            max_gas_share_ceiling: max_gas_share_ceiling.clamp(0.05, 0.35),
            min_priority_tip_gwei: min_priority_tip_gwei.max(0.01),
        }
    }

    /// Computes optimal analytical priority fee tip g*(t) = base_fee + min(delta_MEV * EV / kappa_gas, C_gas_cap).
    pub fn compute_priority_fee(
        &self,
        gross_profit_usd: f64,
        gas_units: f64,
        base_fee_gwei: f64,
        eth_price_usd: f64,
    ) -> PrivateRelayRecommendation {
        let kappa_gas = gas_units * 1e-9 * eth_price_usd;
        let max_allowed_gas_usd = gross_profit_usd * self.max_gas_share_ceiling;

        // Pareto bidding fraction delta_MEV* = alpha / (alpha + 1)
        let delta_mev = self.pareto_alpha / (self.pareto_alpha + 1.0);
        let unconstrained_tip =
            (delta_mev * gross_profit_usd / kappa_gas.max(1e-9)) - base_fee_gwei;

        // Institutional cap: C_gas <= 0.35 * GrossProfit
        let max_tip_allowed =
            ((max_allowed_gas_usd / kappa_gas.max(1e-9)) - base_fee_gwei).max(0.0);
        let (optimal_tip, is_tip_capped) = if max_tip_allowed < self.min_priority_tip_gwei {
            (max_tip_allowed, true)
        } else {
            (
                unconstrained_tip.clamp(self.min_priority_tip_gwei, max_tip_allowed),
                false,
            )
        };

        let total_gas_usd = kappa_gas * (base_fee_gwei + optimal_tip);
        let gas_share = total_gas_usd / gross_profit_usd.max(1e-4);

        let is_compliant = !is_tip_capped
            && total_gas_usd <= max_allowed_gas_usd
            && gross_profit_usd > total_gas_usd;

        let endpoints = vec![
            "https://rpc.titanbuilder.xyz".to_string(),
            "https://rpc.beaverbuild.org".to_string(),
            "https://rpc.flashbots.net/fast".to_string(),
        ];

        PrivateRelayRecommendation {
            recommended_priority_fee_gwei: optimal_tip,
            total_gas_cost_usd: total_gas_usd,
            gross_arbitrage_profit_usd: gross_profit_usd,
            gas_cost_profit_share: gas_share,
            is_institutionally_compliant: is_compliant,
            target_relay_endpoints: endpoints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_institutional_ceiling_strictly_enforced() {
        let engine = PrivateBuilderMevRelayEngine::default();
        let gross_edge_usd = 100.0;
        let gas_units = 250_000.0;
        let base_fee_gwei = 15.0;
        let eth_price = 3000.0;

        let rec = engine.compute_priority_fee(gross_edge_usd, gas_units, base_fee_gwei, eth_price);

        // Max gas cost allowed is 35% of $100 = $35.0
        assert!(rec.total_gas_cost_usd <= 35.0001);
        assert!(rec.gas_cost_profit_share <= 0.35001);
        assert!(rec.is_institutionally_compliant);
    }

    #[test]
    fn test_unprofitable_trade_rejects_compliance() {
        let engine = PrivateBuilderMevRelayEngine::default();
        let gross_edge_usd = 1.0; // Very small edge: $1.00
        let gas_units = 300_000.0;
        let base_fee_gwei = 30.0; // Base gas alone is 0.009 ETH = $27
        let eth_price = 3000.0;

        let rec = engine.compute_priority_fee(gross_edge_usd, gas_units, base_fee_gwei, eth_price);

        // Gas exceeds $1, so transaction must be flagged non-compliant!
        assert!(!rec.is_institutionally_compliant);
    }

    #[test]
    fn test_target_relays_presence() {
        let engine = PrivateBuilderMevRelayEngine::default();
        let rec = engine.compute_priority_fee(500.0, 200_000.0, 10.0, 3000.0);

        assert!(rec
            .target_relay_endpoints
            .iter()
            .any(|url| url.contains("titanbuilder")));
        assert!(rec
            .target_relay_endpoints
            .iter()
            .any(|url| url.contains("beaverbuild")));
        assert!(rec
            .target_relay_endpoints
            .iter()
            .any(|url| url.contains("flashbots")));
    }
}
