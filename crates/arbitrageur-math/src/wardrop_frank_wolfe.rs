//! Dynamic Wardrop Equilibrium Multi-Hop Cross-Rollup Routing.
//!
//! Formulates cross-rollup arbitrage settlement and multi-hop DEX liquidity routing
//! under Wardrop's First Principle (User Equilibrium): in congested networks, every
//! active route between origin and destination experiences identical marginal generalized
//! cost (fees, Kyle's lambda price impact, and Kleinrock queueing delays), and no
//! unused route offers lower cost:
//!
//! $$c_p(\mathbf{f}) = \mu_{OD} \quad \text{if } f_p > 0, \quad c_p(\mathbf{f}) \ge \mu_{OD} \quad \text{if } f_p = 0$$
//!
//! Solves the Beckmann convex optimization formulation:
//! $$\min_{\mathbf{f} \in \mathcal{F}} \sum_{e \in E} \int_0^{f_e} c_e(s) ds$$
//! via the Frank-Wolfe algorithm with exact 1D line search (bisection) and
//! all-or-nothing (AON) Dijkstra assignment.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Directed edge representing a bridge, cross-rollup channel, or DEX swap pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardropRoutingEdge {
    pub edge_id: String,
    pub from_node: usize,
    pub to_node: usize,
    /// Base fixed transaction fee in basis points.
    pub base_fee_bps: f64,
    /// Linear price impact parameter $\gamma_e$ (basis points per unit of flow).
    pub price_impact_gamma: f64,
    /// Maximum capacity $C_e$ before infinite queueing delay occurs.
    pub capacity: f64,
    /// Kleinrock congestion queuing coefficient $\kappa_e$ in basis points.
    pub congestion_kappa: f64,
    /// Settlement finality latency in seconds.
    pub latency_sec: f64,
    /// Capital carry cost rate in basis points per second.
    pub carry_cost_bps_per_sec: f64,
}

impl WardropRoutingEdge {
    /// Computes the generalized marginal link cost $c_e(f_e)$ at current flow $f_e$.
    ///
    /// $$c_e(f_e) = \text{base\_fee} + \gamma_e f_e + \kappa_e \frac{f_e}{C_e - f_e} + \Delta t_e \cdot \text{carry\_rate}$$
    pub fn marginal_cost(&self, flow: f64) -> f64 {
        let f = flow.max(0.0);
        let linear_impact = self.price_impact_gamma * f;
        let latency_cost = self.latency_sec * self.carry_cost_bps_per_sec;

        let congestion_cost = if f >= self.capacity {
            1e9 // Severe barrier penalty when capacity is reached or exceeded
        } else {
            let rem = (self.capacity - f).max(1e-6);
            self.congestion_kappa * (f / rem)
        };

        self.base_fee_bps + linear_impact + congestion_cost + latency_cost
    }

    /// Computes the Beckmann link cost integral $\int_0^{f_e} c_e(s) ds$.
    ///
    /// $$\int_0^{f_e} c_e(s) ds = (\text{base\_fee} + \Delta t \cdot \text{carry}) f_e + \frac{1}{2} \gamma_e f_e^2 - \kappa_e C_e \ln\left(1 - \frac{f_e}{C_e}\right) - \kappa_e f_e$$
    pub fn cost_integral(&self, flow: f64) -> f64 {
        let f = flow.clamp(0.0, self.capacity * 0.9999);
        let linear_term = (self.base_fee_bps + self.latency_sec * self.carry_cost_bps_per_sec) * f;
        let quadratic_term = 0.5 * self.price_impact_gamma * f * f;

        // Integral of f/(C - f) df = -C * ln(1 - f/C) - f
        let ratio = f / self.capacity;
        let log_term = -(1.0 - ratio).ln();
        let congestion_integral = self.congestion_kappa * (self.capacity * log_term - f);

        linear_term + quadratic_term + congestion_integral
    }
}

/// Demand request between an origin and destination node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardropDemand {
    pub origin: usize,
    pub destination: usize,
    /// Total volume to route (e.g. notional USD or token quantity).
    pub volume: f64,
}

/// State of an active route with allocated volume and aggregate cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocatedRoute {
    pub path_edges: Vec<usize>,
    pub allocated_flow: f64,
    pub unit_cost_bps: f64,
}

/// Result of the Frank-Wolfe Wardrop equilibrium routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardropEquilibriumResult {
    /// Final flow allocated on each edge $e \in E$.
    pub edge_flows: Vec<f64>,
    /// Marginal cost on each edge $c_e(f_e)$.
    pub marginal_costs: Vec<f64>,
    /// Total Beckmann objective value.
    pub total_beckmann_objective: f64,
    /// Relative duality gap $\text{Gap} / \sum_e c_e f_e$.
    pub relative_duality_gap: f64,
    /// Number of Frank-Wolfe iterations executed.
    pub iterations: usize,
    /// Discovered active paths for the demands.
    pub active_routes: Vec<AllocatedRoute>,
}

/// Dynamic Wardrop Equilibrium Multi-Hop Network Router.
pub struct WardropFrankWolfeRouter {
    pub num_nodes: usize,
    pub edges: Vec<WardropRoutingEdge>,
    pub max_iterations: usize,
    pub tolerance_gap: f64,
}

#[derive(Copy, Clone, PartialEq)]
struct State {
    cost: f64,
    node: usize,
}

impl Eq for State {}

impl Ord for State {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse for min-heap
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for State {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl WardropFrankWolfeRouter {
    pub fn new(num_nodes: usize, edges: Vec<WardropRoutingEdge>) -> Self {
        Self {
            num_nodes,
            edges,
            max_iterations: 30,
            tolerance_gap: 1e-4,
        }
    }

    /// Solves for the Wardrop user equilibrium routing using the Frank-Wolfe algorithm.
    pub fn solve_equilibrium(
        &self,
        demands: &[WardropDemand],
    ) -> Result<WardropEquilibriumResult, String> {
        let num_edges = self.edges.len();
        if num_edges == 0 {
            return Err("Cannot solve routing on empty edge set".into());
        }

        // Step 0: Initial All-or-Nothing (AON) assignment using zero-flow marginal costs
        let zero_costs: Vec<f64> = self.edges.iter().map(|e| e.marginal_cost(0.0)).collect();
        let mut current_flow = self.all_or_nothing_assignment(demands, &zero_costs)?;

        // Ensure initial flow doesn't violate hard capacities
        for (i, e) in self.edges.iter().enumerate() {
            if current_flow[i] >= e.capacity {
                current_flow[i] = e.capacity * 0.95;
            }
        }

        let mut iterations = 0;
        let mut relative_gap = 1.0;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            // 1. Compute marginal costs at current flow
            let marginal_costs: Vec<f64> = self
                .edges
                .iter()
                .enumerate()
                .map(|(i, e)| e.marginal_cost(current_flow[i]))
                .collect();

            // 2. Auxiliary AON assignment along gradient direction
            let target_flow = self.all_or_nothing_assignment(demands, &marginal_costs)?;

            // 3. Compute duality gap: Gap = \sum_e c_e(f_e) * (f_e - y_e)
            let mut gap = 0.0;
            let mut total_cost = 0.0;
            for i in 0..num_edges {
                let cost_i = marginal_costs[i];
                let flow_i = current_flow[i];
                let y_i = target_flow[i];
                gap += cost_i * (flow_i - y_i);
                total_cost += cost_i * flow_i;
            }

            relative_gap = if total_cost > 1e-8 {
                (gap / total_cost).max(0.0)
            } else {
                0.0
            };

            if relative_gap <= self.tolerance_gap {
                break;
            }

            // 4. Exact 1D line search: find alpha in [0, 1] minimizing Beckmann objective
            let alpha = self.line_search_step(&current_flow, &target_flow);

            // 5. Update flow: f^(k+1) = f^(k) + alpha * (y^(k) - f^(k))
            for i in 0..num_edges {
                current_flow[i] = current_flow[i] + alpha * (target_flow[i] - current_flow[i]);
                // Enforce capacity safety margin
                current_flow[i] = current_flow[i].clamp(0.0, self.edges[i].capacity * 0.999);
            }
        }

        // Final marginal costs and Beckmann objective
        let final_costs: Vec<f64> = self
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| e.marginal_cost(current_flow[i]))
            .collect();

        let total_beckmann: f64 = self
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| e.cost_integral(current_flow[i]))
            .sum();

        // Extract active paths for each demand
        let active_routes = self.extract_active_routes(demands, &final_costs, &current_flow);

        Ok(WardropEquilibriumResult {
            edge_flows: current_flow,
            marginal_costs: final_costs,
            total_beckmann_objective: total_beckmann,
            relative_duality_gap: relative_gap,
            iterations,
            active_routes,
        })
    }

    /// Computes All-or-Nothing (AON) assignment routing 100% of demand onto shortest paths.
    fn all_or_nothing_assignment(
        &self,
        demands: &[WardropDemand],
        costs: &[f64],
    ) -> Result<Vec<f64>, String> {
        let mut target_flow = vec![0.0; self.edges.len()];

        for demand in demands {
            if demand.volume <= 0.0 {
                continue;
            }
            let path_edges = self.find_shortest_path(demand.origin, demand.destination, costs)?;
            for edge_idx in path_edges {
                target_flow[edge_idx] += demand.volume;
            }
        }

        Ok(target_flow)
    }

    /// Finds the minimum-cost directed path from origin to destination via Dijkstra.
    fn find_shortest_path(
        &self,
        origin: usize,
        destination: usize,
        costs: &[f64],
    ) -> Result<Vec<usize>, String> {
        if origin == destination {
            return Ok(Vec::new());
        }

        let mut dist = vec![f64::INFINITY; self.num_nodes];
        let mut prev_edge = vec![None; self.num_nodes];
        let mut prev_node = vec![None; self.num_nodes];
        let mut heap = BinaryHeap::new();

        dist[origin] = 0.0;
        heap.push(State {
            cost: 0.0,
            node: origin,
        });

        // Adjacency list: from_node -> Vec<(edge_idx, to_node, cost)>
        let mut adj: Vec<Vec<(usize, usize, f64)>> = vec![Vec::new(); self.num_nodes];
        for (idx, edge) in self.edges.iter().enumerate() {
            if edge.from_node < self.num_nodes && edge.to_node < self.num_nodes {
                adj[edge.from_node].push((idx, edge.to_node, costs[idx]));
            }
        }

        while let Some(State { cost, node }) = heap.pop() {
            if node == destination {
                break;
            }
            if cost > dist[node] {
                continue;
            }

            for &(edge_idx, to_node, edge_cost) in &adj[node] {
                let next_cost = cost + edge_cost;
                if next_cost < dist[to_node] {
                    dist[to_node] = next_cost;
                    prev_edge[to_node] = Some(edge_idx);
                    prev_node[to_node] = Some(node);
                    heap.push(State {
                        cost: next_cost,
                        node: to_node,
                    });
                }
            }
        }

        if dist[destination].is_infinite() {
            return Err(format!(
                "No connected path from node {} to node {}",
                origin, destination
            ));
        }

        // Reconstruct path backwards
        let mut path = Vec::new();
        let mut curr = destination;
        while curr != origin {
            let e = prev_edge[curr].ok_or_else(|| "Failed to trace path".to_string())?;
            path.push(e);
            curr = prev_node[curr].ok_or_else(|| "Failed to trace node".to_string())?;
        }
        path.reverse();
        Ok(path)
    }

    /// Performs exact 1D line search using bisection on the derivative $d\Phi / d\alpha$.
    fn line_search_step(&self, current: &[f64], target: &[f64]) -> f64 {
        let num_edges = self.edges.len();

        // Derivative dPhi/dalpha = \sum_e c_e(f_e(alpha)) * (y_e - f_e)
        let eval_deriv = |alpha: f64| -> f64 {
            let mut deriv = 0.0;
            for i in 0..num_edges {
                let f_alpha = current[i] + alpha * (target[i] - current[i]);
                let diff = target[i] - current[i];
                deriv += self.edges[i].marginal_cost(f_alpha) * diff;
            }
            deriv
        };

        let deriv_0 = eval_deriv(0.0);
        if deriv_0 >= 0.0 {
            // Already minimal at alpha = 0
            return 0.0;
        }

        let deriv_1 = eval_deriv(1.0);
        if deriv_1 <= 0.0 {
            // Objective strictly decreasing all the way to alpha = 1
            return 1.0;
        }

        // Bisection on [0, 1]
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..16 {
            let mid = 0.5 * (low + high);
            let d_mid = eval_deriv(mid);
            if d_mid < 0.0 {
                low = mid;
            } else {
                high = mid;
            }
        }
        0.5 * (low + high)
    }

    /// Reconstructs active decomposed paths from equilibrium flows.
    fn extract_active_routes(
        &self,
        demands: &[WardropDemand],
        final_costs: &[f64],
        current_flow: &[f64],
    ) -> Vec<AllocatedRoute> {
        let mut routes = Vec::new();
        for demand in demands {
            if let Ok(shortest) =
                self.find_shortest_path(demand.origin, demand.destination, final_costs)
            {
                let path_cost: f64 = shortest.iter().map(|&e| final_costs[e]).sum();
                let min_flow: f64 = shortest
                    .iter()
                    .map(|&e| current_flow[e])
                    .fold(f64::INFINITY, f64::min);

                routes.push(AllocatedRoute {
                    path_edges: shortest,
                    allocated_flow: if min_flow.is_infinite() {
                        demand.volume
                    } else {
                        min_flow
                    },
                    unit_cost_bps: path_cost,
                });
            }
        }
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marginal_cost_monotonicity_and_barrier() {
        let edge = WardropRoutingEdge {
            edge_id: "arb-to-base".into(),
            from_node: 0,
            to_node: 1,
            base_fee_bps: 5.0,
            price_impact_gamma: 0.1, // 0.1 bps per unit
            capacity: 100.0,
            congestion_kappa: 2.0,
            latency_sec: 1.5,
            carry_cost_bps_per_sec: 0.5,
        };

        let cost_0 = edge.marginal_cost(0.0);
        let cost_50 = edge.marginal_cost(50.0);
        let cost_90 = edge.marginal_cost(90.0);
        let cost_cap = edge.marginal_cost(100.0);

        // Fixed cost: base_fee (5.0) + latency (1.5 * 0.5 = 0.75) = 5.75 bps
        assert!((cost_0 - 5.75).abs() < 1e-6);
        assert!(cost_50 > cost_0);
        assert!(cost_90 > cost_50);
        assert!(cost_cap >= 1e8); // Barrier triggered
    }

    #[test]
    fn test_beckmann_cost_integral_consistency() {
        let edge = WardropRoutingEdge {
            edge_id: "uni-eth-usdc".into(),
            from_node: 0,
            to_node: 1,
            base_fee_bps: 3.0,
            price_impact_gamma: 0.05,
            capacity: 50.0,
            congestion_kappa: 1.0,
            latency_sec: 0.0,
            carry_cost_bps_per_sec: 0.0,
        };

        let integral_0 = edge.cost_integral(0.0);
        assert!((integral_0 - 0.0).abs() < 1e-6);

        let flow = 20.0;
        let integral_20 = edge.cost_integral(flow);
        assert!(integral_20 > 0.0);
    }

    #[test]
    fn test_parallel_route_wardrop_equilibrium() {
        // Two parallel paths between Node 0 and Node 1:
        // Path A: Low base fee, high price impact (steep slippage)
        // Path B: High base fee, low price impact (flat slippage)
        let edges = vec![
            WardropRoutingEdge {
                edge_id: "path_A".into(),
                from_node: 0,
                to_node: 1,
                base_fee_bps: 2.0,
                price_impact_gamma: 0.2, // c_A(f) = 2.0 + 0.2 * f
                capacity: 100.0,
                congestion_kappa: 0.0,
                latency_sec: 0.0,
                carry_cost_bps_per_sec: 0.0,
            },
            WardropRoutingEdge {
                edge_id: "path_B".into(),
                from_node: 0,
                to_node: 1,
                base_fee_bps: 6.0,
                price_impact_gamma: 0.05, // c_B(f) = 6.0 + 0.05 * f
                capacity: 100.0,
                congestion_kappa: 0.0,
                latency_sec: 0.0,
                carry_cost_bps_per_sec: 0.0,
            },
        ];

        let router = WardropFrankWolfeRouter::new(2, edges);

        // Demand of 40 units:
        // Theoretical Wardrop equilibrium: c_A(f_A) = c_B(f_B)
        // 2 + 0.2 * f_A = 6 + 0.05 * (40 - f_A)
        // 0.25 * f_A = 6 + 2 - 2 = 6  => f_A = 24.0, f_B = 16.0
        // Cost: c_A = 2 + 4.8 = 6.8 bps, c_B = 6 + 0.8 = 6.8 bps!
        let demand = vec![WardropDemand {
            origin: 0,
            destination: 1,
            volume: 40.0,
        }];

        let result = router.solve_equilibrium(&demand).expect("Failed to solve");
        assert!(result.iterations > 0);
        assert!(result.relative_duality_gap < 0.01);

        let flow_a = result.edge_flows[0];
        let flow_b = result.edge_flows[1];

        // Verify total flow is conserved
        assert!((flow_a + flow_b - 40.0).abs() < 1e-3);
        // Verify flows match theoretical equilibrium (~24.0 and ~16.0)
        assert!((flow_a - 24.0).abs() < 0.8);
        assert!((flow_b - 16.0).abs() < 0.8);

        // Verify marginal costs are equilibrated
        let cost_a = result.marginal_costs[0];
        let cost_b = result.marginal_costs[1];
        assert!((cost_a - cost_b).abs() < 0.5);
    }

    #[test]
    fn test_multi_hop_diamond_network_equilibrium() {
        // Diamond network:
        // Node 0 -> Node 1 (Hop A1), Node 1 -> Node 3 (Hop A2)
        // Node 0 -> Node 2 (Hop B1), Node 2 -> Node 3 (Hop B2)
        let edges = vec![
            WardropRoutingEdge {
                edge_id: "e01".into(),
                from_node: 0,
                to_node: 1,
                base_fee_bps: 1.0,
                price_impact_gamma: 0.1,
                capacity: 50.0,
                congestion_kappa: 0.5,
                latency_sec: 0.2,
                carry_cost_bps_per_sec: 0.1,
            },
            WardropRoutingEdge {
                edge_id: "e13".into(),
                from_node: 1,
                to_node: 3,
                base_fee_bps: 1.0,
                price_impact_gamma: 0.1,
                capacity: 50.0,
                congestion_kappa: 0.5,
                latency_sec: 0.2,
                carry_cost_bps_per_sec: 0.1,
            },
            WardropRoutingEdge {
                edge_id: "e02".into(),
                from_node: 0,
                to_node: 2,
                base_fee_bps: 2.5,
                price_impact_gamma: 0.05,
                capacity: 60.0,
                congestion_kappa: 0.5,
                latency_sec: 0.5,
                carry_cost_bps_per_sec: 0.1,
            },
            WardropRoutingEdge {
                edge_id: "e23".into(),
                from_node: 2,
                to_node: 3,
                base_fee_bps: 2.5,
                price_impact_gamma: 0.05,
                capacity: 60.0,
                congestion_kappa: 0.5,
                latency_sec: 0.5,
                carry_cost_bps_per_sec: 0.1,
            },
        ];

        let router = WardropFrankWolfeRouter::new(4, edges);
        let demands = vec![WardropDemand {
            origin: 0,
            destination: 3,
            volume: 25.0,
        }];

        let result = router
            .solve_equilibrium(&demands)
            .expect("Diamond solve failed");
        assert!(result.relative_duality_gap < 0.02);

        // Total out from 0 must equal 25.0
        let flow_out_0 = result.edge_flows[0] + result.edge_flows[2];
        assert!((flow_out_0 - 25.0).abs() < 1e-3);
    }
}
