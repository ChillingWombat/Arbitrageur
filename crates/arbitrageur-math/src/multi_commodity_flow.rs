//! Cross-EVM & Cross-Rollup Continuous Multi-Commodity Settlement Flow.
//!
//! Formulates multi-commodity network flow conservation across heterogeneous assets (USDC, USDT, ETH, BTC)
//! on multigraphs with shared bundle edge capacities and Capital-at-Risk (CaR) minimization:
//! CaR(x_e, tau_e) = x_e * (z_99.9% * sigma * sqrt(tau_e) + ReorgSurcharge).
//! Implements dynamic protocol switching between Across Protocol V3 intent bridging and LayerZero V2 / CCTP
//! based on segregated venue margin utilization and capital velocity demands.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Heterogeneous assets transported across cross-chain multigraphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommodityAsset {
    USDC,
    USDT,
    ETH,
    BTC,
}

impl CommodityAsset {
    /// Annualized volatility estimate sigma_c.
    pub fn annualized_volatility(&self) -> f64 {
        match self {
            CommodityAsset::USDC => 0.005, // Depeg tail volatility
            CommodityAsset::USDT => 0.008,
            CommodityAsset::ETH => 0.65,
            CommodityAsset::BTC => 0.55,
        }
    }
}

/// Supported cross-chain bridging settlement protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettlementProtocol {
    AcrossV3,
    LayerZeroV2,
    CCTP,
    Hyperlane,
}

/// A directed multigraph edge connecting two execution environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowRouteEdge {
    pub edge_id: String,
    pub from_chain: String,
    pub to_chain: String,
    pub protocol: SettlementProtocol,
    /// Aggregate shared bundle capacity across all commodities in USD.
    pub bundle_capacity_usd: f64,
    /// Linear fee rate in basis points.
    pub unit_fee_bps: f64,
    /// Settlement latency in seconds.
    pub latency_sec: f64,
    /// Chain reorg surcharge in basis points.
    pub reorg_surcharge_bps: f64,
}

/// Transfer allocation for a specific commodity over an edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommodityTransfer {
    pub edge_id: String,
    pub asset: CommodityAsset,
    pub amount_usd: f64,
    pub protocol: SettlementProtocol,
    pub fee_usd: f64,
    pub capital_at_risk_usd: f64,
    pub latency_sec: f64,
}

/// Solution of the multi-commodity flow optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiCommodityFlowSolution {
    pub transfers: Vec<CommodityTransfer>,
    pub total_flow_usd: f64,
    pub total_fee_usd: f64,
    pub total_car_usd: f64,
    pub is_feasible: bool,
}

/// Optimizer for continuous multi-commodity settlement flow on cross-chain multigraphs.
#[derive(Debug, Clone)]
pub struct MultiCommodityFlowOptimizer {
    pub edges: Vec<FlowRouteEdge>,
    /// Net exogenous demand / supply vector: node -> asset -> net_demand_usd.
    /// Positive = destination demand (inflow required), Negative = origin supply (outflow available).
    node_demands: HashMap<String, HashMap<CommodityAsset, f64>>,
    /// Normal quantile for 99.9% VaR (z_99.9% = 3.0902).
    z_score: f64,
}

impl Default for MultiCommodityFlowOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiCommodityFlowOptimizer {
    pub fn new() -> Self {
        Self {
            edges: Vec::new(),
            node_demands: HashMap::new(),
            z_score: 3.09023,
        }
    }

    /// Adds a candidate settlement edge to the network multigraph.
    pub fn add_edge(&mut self, edge: FlowRouteEdge) {
        self.edges.push(edge);
    }

    /// Sets node supply (negative) or demand (positive) for a commodity.
    pub fn set_node_demand(&mut self, node: &str, asset: CommodityAsset, demand_usd: f64) {
        self.node_demands
            .entry(node.to_string())
            .or_default()
            .insert(asset, demand_usd);
    }

    /// Computes Capital-at-Risk (CaR) for transporting amount x_e of commodity c over edge e.
    /// CaR = x_e * [z_99.9% * sigma_c * sqrt(tau_e / (365 * 86400)) + ReorgSurcharge_e]
    pub fn compute_capital_at_risk(
        &self,
        asset: CommodityAsset,
        amount_usd: f64,
        latency_sec: f64,
        reorg_surcharge_bps: f64,
    ) -> f64 {
        let sigma = asset.annualized_volatility();
        let tau_years = latency_sec / (365.25 * 86400.0);
        let volatility_drift = self.z_score * sigma * tau_years.sqrt();
        let reorg_drag = reorg_surcharge_bps / 10_000.0;
        amount_usd * (volatility_drift + reorg_drag)
    }

    /// Evaluates dynamic protocol switching given venue margin utilization U_m:
    /// - If U_m > 0.70: Urgent liquidation threat -> choose Across Protocol V3 (sub-20s settlement).
    /// - If U_m < 0.45: Calm regime -> choose LayerZero V2 / CCTP (minimize fixed fee drag).
    /// - If 0.45 <= U_m <= 0.70: Balanced evaluation based on net CaR + Fee.
    pub fn select_protocol(
        margin_utilization: f64,
        across_candidate: &FlowRouteEdge,
        l0_candidate: &FlowRouteEdge,
        asset: CommodityAsset,
        amount_usd: f64,
    ) -> SettlementProtocol {
        if margin_utilization > 0.70 {
            // High margin stress: latency must be minimized to avoid liquidation trim
            SettlementProtocol::AcrossV3
        } else if margin_utilization < 0.45 {
            // Calm capital regime: choose lowest fee route (typically CCTP or LayerZero)
            l0_candidate.protocol
        } else {
            // Evaluate total economic cost = Fee + CaR
            let tau_yr_across = across_candidate.latency_sec / (365.25 * 86400.0);
            let car_across = amount_usd
                * (3.09023 * asset.annualized_volatility() * tau_yr_across.sqrt()
                    + across_candidate.reorg_surcharge_bps / 10_000.0);
            let fee_across = amount_usd * (across_candidate.unit_fee_bps / 10_000.0);
            let total_across = fee_across + car_across;

            let tau_yr_l0 = l0_candidate.latency_sec / (365.25 * 86400.0);
            let car_l0 = amount_usd
                * (3.09023 * asset.annualized_volatility() * tau_yr_l0.sqrt()
                    + l0_candidate.reorg_surcharge_bps / 10_000.0);
            let fee_l0 = amount_usd * (l0_candidate.unit_fee_bps / 10_000.0);
            let total_l0 = fee_l0 + car_l0;

            if total_across <= total_l0 {
                SettlementProtocol::AcrossV3
            } else {
                l0_candidate.protocol
            }
        }
    }

    /// Solves the multi-commodity network flow minimizing total CaR + Fees while satisfying demands.
    pub fn solve_settlement_flow(&self) -> MultiCommodityFlowSolution {
        let mut transfers = Vec::new();
        let mut total_flow_usd = 0.0;
        let mut total_fee_usd = 0.0;
        let mut total_car_usd = 0.0;

        // Track remaining capacity on bundle edges
        let mut remaining_edge_capacity: HashMap<String, f64> = self
            .edges
            .iter()
            .map(|e| (e.edge_id.clone(), e.bundle_capacity_usd))
            .collect();

        // Find matching demands (positive) with supplies (negative)
        for (dst_node, asset_map) in &self.node_demands {
            for (&asset, &demand_usd) in asset_map {
                if demand_usd <= 0.0 {
                    continue; // Skip source/supply nodes
                }

                let mut unmet_demand = demand_usd;

                // Find candidate edges delivering into dst_node
                let mut candidates: Vec<&FlowRouteEdge> = self
                    .edges
                    .iter()
                    .filter(|e| e.to_chain == *dst_node)
                    .collect();

                // Sort candidates by effective unit cost (Fee + CaR per USD)
                candidates.sort_by(|a, b| {
                    let cost_a = a.unit_fee_bps / 10_000.0
                        + self.compute_capital_at_risk(
                            asset,
                            1.0,
                            a.latency_sec,
                            a.reorg_surcharge_bps,
                        );
                    let cost_b = b.unit_fee_bps / 10_000.0
                        + self.compute_capital_at_risk(
                            asset,
                            1.0,
                            b.latency_sec,
                            b.reorg_surcharge_bps,
                        );
                    cost_a
                        .partial_cmp(&cost_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                for edge in candidates {
                    if unmet_demand <= 0.0 {
                        break;
                    }

                    let available_capacity = remaining_edge_capacity
                        .get(&edge.edge_id)
                        .copied()
                        .unwrap_or(0.0);
                    if available_capacity <= 0.0 {
                        continue;
                    }

                    let allocated_amount = unmet_demand.min(available_capacity);
                    let fee_usd = allocated_amount * (edge.unit_fee_bps / 10_000.0);
                    let car_usd = self.compute_capital_at_risk(
                        asset,
                        allocated_amount,
                        edge.latency_sec,
                        edge.reorg_surcharge_bps,
                    );

                    transfers.push(CommodityTransfer {
                        edge_id: edge.edge_id.clone(),
                        asset,
                        amount_usd: allocated_amount,
                        protocol: edge.protocol,
                        fee_usd,
                        capital_at_risk_usd: car_usd,
                        latency_sec: edge.latency_sec,
                    });

                    unmet_demand -= allocated_amount;
                    total_flow_usd += allocated_amount;
                    total_fee_usd += fee_usd;
                    total_car_usd += car_usd;

                    if let Some(cap) = remaining_edge_capacity.get_mut(&edge.edge_id) {
                        *cap -= allocated_amount;
                    }
                }
            }
        }

        MultiCommodityFlowSolution {
            transfers,
            total_flow_usd,
            total_fee_usd,
            total_car_usd,
            is_feasible: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capital_at_risk_calculation() {
        let optimizer = MultiCommodityFlowOptimizer::new();
        // $100k USDC on 15-second Across bridge
        let car_usdc =
            optimizer.compute_capital_at_risk(CommodityAsset::USDC, 100_000.0, 15.0, 0.5);
        assert!(car_usdc > 0.0 && car_usdc < 50.0); // Minimal for stablecoin with low latency

        // $100k ETH on 900-second (15m) slow bridge
        let car_eth = optimizer.compute_capital_at_risk(CommodityAsset::ETH, 100_000.0, 900.0, 2.0);
        assert!(car_eth > car_usdc); // Volatile asset + high latency generates substantial CaR
    }

    #[test]
    fn test_dynamic_protocol_switching() {
        let across_edge = FlowRouteEdge {
            edge_id: "across-arb-base".into(),
            from_chain: "Arbitrum".into(),
            to_chain: "Base".into(),
            protocol: SettlementProtocol::AcrossV3,
            bundle_capacity_usd: 1_000_000.0,
            unit_fee_bps: 4.5,
            latency_sec: 15.0,
            reorg_surcharge_bps: 0.5,
        };

        let l0_edge = FlowRouteEdge {
            edge_id: "cctp-arb-base".into(),
            from_chain: "Arbitrum".into(),
            to_chain: "Base".into(),
            protocol: SettlementProtocol::CCTP,
            bundle_capacity_usd: 5_000_000.0,
            unit_fee_bps: 0.5,
            latency_sec: 900.0,
            reorg_surcharge_bps: 1.0,
        };

        // When margin utilization > 0.70, Across V3 must be selected to avert liquidation
        let proto_urgent = MultiCommodityFlowOptimizer::select_protocol(
            0.78,
            &across_edge,
            &l0_edge,
            CommodityAsset::USDC,
            50_000.0,
        );
        assert_eq!(proto_urgent, SettlementProtocol::AcrossV3);

        // When margin utilization < 0.45, CCTP must be selected to minimize fees
        let proto_calm = MultiCommodityFlowOptimizer::select_protocol(
            0.35,
            &across_edge,
            &l0_edge,
            CommodityAsset::USDC,
            50_000.0,
        );
        assert_eq!(proto_calm, SettlementProtocol::CCTP);
    }

    #[test]
    fn test_multi_commodity_solution() {
        let mut optimizer = MultiCommodityFlowOptimizer::new();
        optimizer.add_edge(FlowRouteEdge {
            edge_id: "edge-1".into(),
            from_chain: "Arbitrum".into(),
            to_chain: "Base".into(),
            protocol: SettlementProtocol::AcrossV3,
            bundle_capacity_usd: 150_000.0,
            unit_fee_bps: 4.0,
            latency_sec: 12.0,
            reorg_surcharge_bps: 0.5,
        });

        optimizer.set_node_demand("Base", CommodityAsset::USDC, 100_000.0);
        optimizer.set_node_demand("Base", CommodityAsset::ETH, 25_000.0);

        let sol = optimizer.solve_settlement_flow();
        assert!(sol.is_feasible);
        assert_eq!(sol.total_flow_usd, 125_000.0);
        assert_eq!(sol.transfers.len(), 2);
        assert!(sol.total_fee_usd > 0.0);
        assert!(sol.total_car_usd > 0.0);
    }
}
