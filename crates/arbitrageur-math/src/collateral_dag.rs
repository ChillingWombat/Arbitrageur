//! Cross-Exchange Smart Settlement & Real-Time Collateral Rehypothecation DAG Router.
//!
//! Models cross-venue, cross-chain, and hot-wallet capital transfer rails as a Directed Multigraph DAG:
//! - Vertices: Segregated venue margin accounts (e.g. `binance`, `okx`, `hyperliquid`, `lighter`, `deribit`)
//!   and treasury liquidity pools.
//! - Edges: Settlement rails with heterogeneous fees, gas costs, transit delays, and transfer capacity:
//!   - `CexInternalZeroFee`: Sub-account transfer API (zero fee, ~1.5s latency).
//!   - `AcrossIntentBridgeL2`: Intent-based optimistic cross-L2 bridge (fast, ~15s, variable LP fee).
//!   - `CctpNativeBridge`: Circle CCTP burn-and-mint bridge (~12-18 min, low fixed fee).
//!   - `TradFiWire`: Fiat bank wire / broker clearing (~24h).
//!
//! Solves the optimal transfer route minimizing total economic cost:
//! $$\mathcal{C}_e(q) = c_e^{\text{fixed}} + \left(\frac{f_e^{\text{bps}}}{10000}\right) q + \text{Gas}_e + \tau_e \cdot r_{\text{opp}} \cdot q$$
//! subject to the strict liquidation latency deadline constraint:
//! $$\sum_{e \in \mathcal{P}} \tau_e < \tau_{\text{deadline}}$$

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Supported collateral transfer rail classifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DagTransferRailType {
    /// CEX internal zero-fee subaccount or omnibus transfer.
    CexInternalZeroFee,
    /// Fast intent-based L2 bridge (e.g. Across Protocol, Hyperliquid L1 bridge).
    AcrossIntentBridgeL2,
    /// Private builder bundle relay (Flashbots Protect, MEV-Share) with zero-sandwich guarantee.
    MevProtectedPrivateBundle,
    /// Native cross-chain bridge (e.g. Circle CCTP, Arbitrum Native).
    CctpNativeBridge,
    /// Institutional TradFi broker clearing / bank wire.
    TradFiWire,
}

/// A directed edge representing an operational capital transfer rail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DagTransferEdge {
    /// Origin venue identifier.
    pub from_venue: String,
    /// Destination venue identifier.
    pub to_venue: String,
    /// Rail category.
    pub rail_type: DagTransferRailType,
    /// Fixed transfer fee in USD (e.g. $1.00 for CEX withdrawal).
    pub fixed_fee_usd: f64,
    /// Proportional fee in basis points (e.g. 2.5 bps for bridge LP).
    pub fee_bps: f64,
    /// Estimated transaction gas cost in USD.
    pub gas_cost_usd: f64,
    /// Operational transfer latency in seconds (including block confirmations).
    pub latency_sec: f64,
    /// Maximum instantaneous transfer capacity in USD.
    pub capacity_usd: f64,
    /// Whether the transfer rail is protected against public mempool frontrunning/MEV sandwiching.
    pub is_mev_protected: bool,
}

impl DagTransferEdge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        from_venue: impl Into<String>,
        to_venue: impl Into<String>,
        rail_type: DagTransferRailType,
        fixed_fee_usd: f64,
        fee_bps: f64,
        gas_cost_usd: f64,
        latency_sec: f64,
        capacity_usd: f64,
    ) -> Self {
        let is_mev_protected = matches!(
            rail_type,
            DagTransferRailType::CexInternalZeroFee
                | DagTransferRailType::MevProtectedPrivateBundle
                | DagTransferRailType::TradFiWire
        );
        Self {
            from_venue: from_venue.into(),
            to_venue: to_venue.into(),
            rail_type,
            fixed_fee_usd: fixed_fee_usd.max(0.0),
            fee_bps: fee_bps.max(0.0),
            gas_cost_usd: gas_cost_usd.max(0.0),
            latency_sec: latency_sec.max(0.1),
            capacity_usd: capacity_usd.max(0.0),
            is_mev_protected,
        }
    }

    pub fn with_mev_protected(mut self, protected: bool) -> Self {
        self.is_mev_protected = protected;
        self
    }
}

/// Priority queue state for Dijkstra shortest-path search.
#[derive(Copy, Clone, PartialEq)]
struct DijkstraNode {
    cost: f64,
    latency_sec: f64,
    venue_idx: usize,
}

impl Eq for DijkstraNode {}

impl Ord for DijkstraNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by cost
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for DijkstraNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Execution plan output from the DAG router.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DagRebalancePlan {
    /// Ordered sequence of venues in the transfer route.
    pub path: Vec<String>,
    /// Total economic transfer cost in USD (fees + gas + idle capital cost).
    pub total_cost_usd: f64,
    /// Total expected transfer latency in seconds.
    pub total_latency_sec: f64,
    /// Transfer amount routed in USD.
    pub amount_usd: f64,
    /// Step-by-step transfer edge details.
    pub steps: Vec<DagTransferEdge>,
}

/// Directed Acyclic Graph router for optimal collateral rehypothecation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollateralRehypothecationDagRouter {
    /// Set of active settlement rail edges.
    pub edges: Vec<DagTransferEdge>,
    /// Annualized opportunity cost rate for idle transit capital (e.g. 0.20 for 20% APR).
    pub opp_cost_annual_rate: f64,
    /// Dynamic EIP-1559 priority fee bidding engine for on-chain transfer rails.
    pub eip1559_engine: Option<crate::atomic_mev::DynamicEip1559GasBiddingEngine>,
}

impl Default for CollateralRehypothecationDagRouter {
    fn default() -> Self {
        Self {
            edges: Vec::new(),
            opp_cost_annual_rate: 0.25, // 25% APR idle capital opportunity cost
            eip1559_engine: None,
        }
    }
}

impl CollateralRehypothecationDagRouter {
    pub fn new(edges: Vec<DagTransferEdge>) -> Self {
        Self {
            edges,
            opp_cost_annual_rate: 0.25,
            eip1559_engine: None,
        }
    }

    pub fn with_eip1559_engine(
        mut self,
        engine: crate::atomic_mev::DynamicEip1559GasBiddingEngine,
    ) -> Self {
        self.eip1559_engine = Some(engine);
        self
    }

    /// Evaluates the total edge economic cost:
    /// $$\mathcal{C}_e(q) = c_e^{\text{fixed}} + \left(\frac{f_e^{\text{bps}}}{10000}\right) q + \text{Gas}_e + \tau_e \cdot r_{\text{opp}} \cdot q + \text{MEV}_{\text{risk}}$$
    pub fn compute_edge_cost(&self, edge: &DagTransferEdge, amount_usd: f64) -> f64 {
        let opp_cost_per_sec = self.opp_cost_annual_rate / (365.25 * 86400.0);
        let idle_capital_cost = edge.latency_sec * opp_cost_per_sec * amount_usd;
        let variable_fee = (edge.fee_bps / 10000.0) * amount_usd;
        let mev_risk_premium = if edge.is_mev_protected {
            0.0
        } else {
            // Unprotected public mempool transfer risks toxic sandwich extraction (5 bps)
            amount_usd * 0.0005
        };
        edge.fixed_fee_usd + variable_fee + edge.gas_cost_usd + idle_capital_cost + mev_risk_premium
    }

    /// Solves the optimal transfer route via Dijkstra under the hard latency deadline constraint.
    pub fn find_optimal_transfer_route(
        &self,
        from_venue: &str,
        to_venue: &str,
        amount_usd: f64,
        max_allowed_latency_sec: f64,
    ) -> Option<DagRebalancePlan> {
        if from_venue == to_venue || amount_usd <= 0.0 {
            return None;
        }

        // Map venue strings to indices
        let mut venue_to_idx = HashMap::new();
        let mut idx_to_venue = Vec::new();

        for edge in &self.edges {
            if !venue_to_idx.contains_key(&edge.from_venue) {
                venue_to_idx.insert(edge.from_venue.clone(), idx_to_venue.len());
                idx_to_venue.push(edge.from_venue.clone());
            }
            if !venue_to_idx.contains_key(&edge.to_venue) {
                venue_to_idx.insert(edge.to_venue.clone(), idx_to_venue.len());
                idx_to_venue.push(edge.to_venue.clone());
            }
        }

        let start_idx = *venue_to_idx.get(from_venue)?;
        let target_idx = *venue_to_idx.get(to_venue)?;

        let n = idx_to_venue.len();
        let mut min_cost = vec![f64::INFINITY; n];
        let mut min_latency = vec![f64::INFINITY; n];
        let mut parent_edge = vec![None; n];

        let mut heap = BinaryHeap::new();
        min_cost[start_idx] = 0.0;
        min_latency[start_idx] = 0.0;

        heap.push(DijkstraNode {
            cost: 0.0,
            latency_sec: 0.0,
            venue_idx: start_idx,
        });

        while let Some(DijkstraNode {
            cost,
            latency_sec,
            venue_idx,
        }) = heap.pop()
        {
            if cost > min_cost[venue_idx] {
                continue;
            }
            if venue_idx == target_idx {
                break;
            }

            let curr_venue = &idx_to_venue[venue_idx];
            for (edge_idx, edge) in self.edges.iter().enumerate() {
                if edge.from_venue == *curr_venue && edge.capacity_usd >= amount_usd {
                    let next_idx = *venue_to_idx.get(&edge.to_venue).unwrap();
                    let edge_c = self.compute_edge_cost(edge, amount_usd);
                    let new_cost = cost + edge_c;
                    let new_latency = latency_sec + edge.latency_sec;

                    // Check liquidation latency deadline constraint
                    if new_latency <= max_allowed_latency_sec && new_cost < min_cost[next_idx] {
                        min_cost[next_idx] = new_cost;
                        min_latency[next_idx] = new_latency;
                        parent_edge[next_idx] = Some(edge_idx);
                        heap.push(DijkstraNode {
                            cost: new_cost,
                            latency_sec: new_latency,
                            venue_idx: next_idx,
                        });
                    }
                }
            }
        }

        if min_cost[target_idx].is_infinite() {
            return None; // No route satisfies the latency deadline and capacity requirements
        }

        // Reconstruct path
        let mut steps = Vec::new();
        let mut path = Vec::new();
        let mut curr = target_idx;
        path.push(idx_to_venue[curr].clone());

        while let Some(e_idx) = parent_edge[curr] {
            let edge = self.edges[e_idx].clone();
            curr = *venue_to_idx.get(&edge.from_venue).unwrap();
            path.push(edge.from_venue.clone());
            steps.push(edge);
        }
        path.reverse();
        steps.reverse();

        Some(DagRebalancePlan {
            path,
            total_cost_usd: min_cost[target_idx],
            total_latency_sec: min_latency[target_idx],
            amount_usd,
            steps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_cex_rail_chosen_over_expensive_bridge() {
        let edges = vec![
            // Option 1: Direct L2 bridge from Binance to Lighter: $5.00 gas, 3 bps fee, 60s latency
            DagTransferEdge::new(
                "binance",
                "lighter",
                DagTransferRailType::AcrossIntentBridgeL2,
                1.0,
                3.0,
                5.0,
                60.0,
                50000.0,
            ),
            // Option 2: 2-step transfer: Binance -> CEX Subaccount ($0 fee, 1.5s) -> Lighter ($2 gas, 10s)
            DagTransferEdge::new(
                "binance",
                "binance_sub",
                DagTransferRailType::CexInternalZeroFee,
                0.0,
                0.0,
                0.0,
                1.5,
                100000.0,
            ),
            DagTransferEdge::new(
                "binance_sub",
                "lighter",
                DagTransferRailType::AcrossIntentBridgeL2,
                0.5,
                1.0,
                2.0,
                10.0,
                100000.0,
            ),
        ];

        let router = CollateralRehypothecationDagRouter::new(edges);
        let plan = router
            .find_optimal_transfer_route("binance", "lighter", 10000.0, 300.0)
            .unwrap();

        assert_eq!(plan.path, vec!["binance", "binance_sub", "lighter"]);
        assert!(plan.total_latency_sec < 15.0);
        assert!(plan.total_cost_usd < 10.0);
    }

    #[test]
    fn test_hard_latency_deadline_prunes_slow_route() {
        let edges = vec![
            // Ultra-cheap but slow CCTP route: $0.10 fee, 1200 seconds (20 minutes)
            DagTransferEdge::new(
                "treasury",
                "hyperliquid",
                DagTransferRailType::CctpNativeBridge,
                0.10,
                0.0,
                0.5,
                1200.0,
                100000.0,
            ),
            // Fast Across route: $2.50 fee, 15 seconds
            DagTransferEdge::new(
                "treasury",
                "hyperliquid",
                DagTransferRailType::AcrossIntentBridgeL2,
                2.50,
                2.0,
                1.5,
                15.0,
                100000.0,
            ),
        ];

        let router = CollateralRehypothecationDagRouter::new(edges);

        // Scenario A: Margin call liquidation deadline is 60 seconds (1 minute).
        // Slow CCTP route (1200s) MUST be pruned!
        let plan = router
            .find_optimal_transfer_route("treasury", "hyperliquid", 5000.0, 60.0)
            .unwrap();

        assert_eq!(
            plan.steps[0].rail_type,
            DagTransferRailType::AcrossIntentBridgeL2
        );
        assert_eq!(plan.total_latency_sec, 15.0);

        // Scenario B: Liquidation deadline is 10 seconds. Neither route can make it!
        let impossible_plan =
            router.find_optimal_transfer_route("treasury", "hyperliquid", 5000.0, 10.0);
        assert!(
            impossible_plan.is_none(),
            "Must return None when deadline is breached"
        );
    }

    #[test]
    fn test_capacity_rejection_falls_back_or_fails() {
        let edges = vec![DagTransferEdge::new(
            "lighter",
            "hyperliquid",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            2.0,
            1.0,
            20.0,
            5000.0, // Cap $5,000
        )];

        let router = CollateralRehypothecationDagRouter::new(edges);

        // Request $4,000 -> succeeds
        assert!(router
            .find_optimal_transfer_route("lighter", "hyperliquid", 4000.0, 60.0)
            .is_some());

        // Request $6,000 -> exceeds capacity, returns None
        assert!(router
            .find_optimal_transfer_route("lighter", "hyperliquid", 6000.0, 60.0)
            .is_none());
    }

    #[test]
    fn test_mev_protected_private_bundle_rail_preferred_over_sandwich_mempool() {
        // Rail A: Unprotected public mempool (subject to 5 bps sandwich risk)
        let edge_public = DagTransferEdge::new(
            "arbitrum",
            "hyperliquid",
            DagTransferRailType::AcrossIntentBridgeL2,
            0.5,
            1.0, // 1 bps fee
            0.5,
            15.0,
            100_000.0,
        )
        .with_mev_protected(false);

        // Rail B: MEV-Protected Private Builder Bundle (0 sandwich risk, higher nominal fee 3 bps)
        let edge_private = DagTransferEdge::new(
            "arbitrum",
            "hyperliquid",
            DagTransferRailType::MevProtectedPrivateBundle,
            0.5,
            3.0, // 3 bps fee
            0.5,
            15.0,
            100_000.0,
        );

        let router = CollateralRehypothecationDagRouter::new(vec![
            edge_public.clone(),
            edge_private.clone(),
        ]);

        // For large transfer ($50,000):
        // Public fee = $0.5 + 1 bps ($5.0) + $0.5 + latency + 5 bps MEV risk ($25.0) = $31.0 + idle
        // Private fee = $0.5 + 3 bps ($15.0) + $0.5 + latency + 0 MEV risk = $16.0 + idle
        // Private rail must be chosen because sandwich protection saves more than nominal fee!
        let cost_public = router.compute_edge_cost(&edge_public, 50_000.0);
        let cost_private = router.compute_edge_cost(&edge_private, 50_000.0);
        assert!(
            cost_private < cost_public,
            "Private bundle should be cheaper after MEV risk adjustment"
        );

        let plan = router
            .find_optimal_transfer_route("arbitrum", "hyperliquid", 50_000.0, 60.0)
            .expect("plan");
        assert_eq!(
            plan.steps[0].rail_type,
            DagTransferRailType::MevProtectedPrivateBundle
        );
    }
}
