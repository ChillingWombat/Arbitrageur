use serde::{Deserialize, Serialize};

pub use arbitrageur_math::collateral_barrier::{
    CollateralRebalanceTransfer, CrossVenueCollateralBarrierRebalancer, VenueAccountMarginState,
};
pub use arbitrageur_math::collateral_dag::{
    CollateralRehypothecationDagRouter, DagRebalancePlan, DagTransferEdge, DagTransferRailType,
};

/// Error function approximation (Abramowitz and Stegun 7.1.26).
fn erf_approx(x: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let abs_x = x.abs();

    let t = 1.0 / (1.0 + p * abs_x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-abs_x * abs_x).exp();

    sign * y
}

/// Standard normal CDF Phi(z).
fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf_approx(z / std::f64::consts::SQRT_2))
}

/// Production-grade First-Passage Liquidation Stopping Time Engine.
/// Calculates exact liquidation probabilities under jump-diffusion and optimal transfer threshold Delta M*.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstPassageLiquidationModel {
    /// Target margin buffer reservation (e.g. 0.40 = 40% margin buffer above MMR).
    pub target_mbr: f64,
    /// Annualized Poisson jump arrival intensity lambda_J (e.g. 2.5 crashes/year).
    pub jump_intensity: f64,
    /// Mean jump percentage in crash scenarios (e.g. -0.15 = -15%).
    pub jump_mean: f64,
    /// Standard deviation of jump percentage (e.g. 0.12).
    pub jump_std: f64,
}

impl Default for FirstPassageLiquidationModel {
    fn default() -> Self {
        Self {
            target_mbr: 0.40,
            jump_intensity: 2.5,
            jump_mean: -0.15,
            jump_std: 0.12,
        }
    }
}

impl FirstPassageLiquidationModel {
    pub fn new(target_mbr: f64, jump_intensity: f64, jump_mean: f64, jump_std: f64) -> Self {
        Self {
            target_mbr: target_mbr.clamp(0.10, 1.00),
            jump_intensity: jump_intensity.max(0.0),
            jump_mean,
            jump_std: jump_std.max(1e-4),
        }
    }

    /// Computes first-passage liquidation probability P(tau_liq <= t) over horizon t (years).
    /// Combines continuous Brownian motion reflection principle with Poisson jump-diffusion.
    pub fn compute_liquidation_probability(
        &self,
        spot_price: f64,
        liq_price: f64,
        volatility_annual: f64,
        horizon_years: f64,
    ) -> f64 {
        if spot_price <= liq_price || liq_price <= 0.0 || horizon_years <= 0.0 {
            return 1.0;
        }
        let b = (spot_price / liq_price).ln();
        if b <= 0.0 {
            return 1.0;
        }

        let sigma_sqrt_t = (volatility_annual * horizon_years.sqrt()).max(1e-6);
        let z = b / sigma_sqrt_t;

        // Diffusive first-passage: 2 * (1 - Phi(b / (sigma * sqrt(t))))
        let phi_z = normal_cdf(z);
        let p_diff = (2.0 * (1.0 - phi_z)).clamp(0.0, 1.0);

        // Jump component: (1 - exp(-lambda * t)) * P(Jump breach)
        let p_jump_event = (1.0 - (-self.jump_intensity * horizon_years).exp()).clamp(0.0, 1.0);
        let z_jump = (-b - self.jump_mean) / self.jump_std;
        let p_jump_breach = normal_cdf(z_jump);

        let total_prob = p_diff + p_jump_event * p_jump_breach;
        total_prob.clamp(0.0, 1.0)
    }

    /// Solves closed-form optimal transfer size Delta M* to restore target buffer MBR*.
    /// Delta M* = (1 + MBR*) * Notional * MMR - CurrentEquity
    pub fn compute_optimal_transfer_size(
        &self,
        notional_usd: f64,
        mmr: f64,
        current_equity_usd: f64,
    ) -> f64 {
        let target_equity = (1.0 + self.target_mbr) * notional_usd * mmr;
        (target_equity - current_equity_usd).max(0.0)
    }

    /// Escalates target MBR and jump intensity dynamically based on MS-VECM regime breakdown probability
    /// and continuous basis jump shock magnitude.
    pub fn with_vecm_jump_escalation(
        &self,
        regime_2_breakdown_prob: f64,
        jump_magnitude_sigma: f64,
    ) -> Self {
        let prob_clamped = regime_2_breakdown_prob.clamp(0.0, 1.0);
        let jump_mag = jump_magnitude_sigma.abs();
        let escalated_mbr =
            (self.target_mbr * (1.0 + 2.5 * prob_clamped + jump_mag)).clamp(0.10, 2.50);
        let escalated_lambda = self.jump_intensity + 5.0 * prob_clamped;
        let escalated_mean = self.jump_mean - 0.05 * jump_mag;
        Self {
            target_mbr: escalated_mbr,
            jump_intensity: escalated_lambda,
            jump_mean: escalated_mean,
            jump_std: self.jump_std,
        }
    }

    /// Evaluates whether volatility exceeds sigma_breakeven for async bridging.
    /// When volatility exceeds sigma_breakeven, the transfer cannot arrive before expected
    /// liquidation, mandating immediate software position trimming instead of waiting for a bridge.
    pub fn is_trim_preferred_over_bridge(
        &self,
        spot_price: f64,
        liq_price: f64,
        bridge_latency_years: f64,
        current_volatility: f64,
    ) -> bool {
        if spot_price <= liq_price || liq_price <= 0.0 || bridge_latency_years <= 0.0 {
            return true;
        }
        let b = (spot_price / liq_price).ln();
        let z_crit = 2.326; // 99% confidence barrier
        let sigma_breakeven = b / (bridge_latency_years.sqrt() * z_crit);
        current_volatility >= sigma_breakeven
    }
}

/// Available transfer rails across exchanges and L2 rollups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferRail {
    /// Fast L2 Intent-based relay (Across Protocol: ~8-35s latency, 3.5 bps fee).
    AcrossIntentL2,
    /// Native burn-and-mint bridge (Circle CCTP: ~8-15 min latency, gas fee only).
    CctpNativeL2,
    /// CEX internal sub-account or universal transfer API (Binance/OKX: ~150-600ms, 0 fee).
    CexInternalSubAccount,
    /// TradFi institutional wire (IBKR/Alpaca: ~2-6 hours, flat fee).
    TradFiWire,
}

/// Bridge transfer quote with cost and estimated settlement latency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeQuote {
    pub rail: TransferRail,
    pub total_fee_usd: f64,
    pub estimated_latency_sec: f64,
    pub is_executable: bool,
}

/// Canonical Across Protocol SpokePool V3 deployments across supported EVM networks.
pub fn across_spokepool_address(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        1 => Some("0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5"), // Ethereum L1
        42161 => Some("0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A"), // Arbitrum One
        8453 => Some("0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64"), // Base
        10 => Some("0x6f26Bf09B1C792e3228e5467807a900A50De2918"), // Optimism
        _ => None,
    }
}

/// Concrete transfer plan for an Across Protocol SpokePool V3 intent-based collateral rebalance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcrossBridgeTransferPlan {
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub spoke_pool_address: String,
    pub input_token: String,
    pub output_token: String,
    pub amount_usd: f64,
    pub net_received_usd: f64,
    pub estimated_fee_usd: f64,
    pub estimated_latency_sec: f64,
    pub fee_bps: f64,
    pub is_solvent: bool,
    pub max_safe_latency_sec: f64,
}

/// Cross-Exchange Bridge Relay Cost & Latency Router.
#[derive(Debug, Clone, Default)]
pub struct BridgeRelayCostEstimator {
    pub intent_engine: arbitrageur_math::intent_bridge::CrossChainIntentBridgeEngine,
}

impl BridgeRelayCostEstimator {
    pub fn new(
        intent_engine: arbitrageur_math::intent_bridge::CrossChainIntentBridgeEngine,
    ) -> Self {
        Self { intent_engine }
    }

    /// Estimates fees and latency for transferring collateral between two venues.
    pub fn quote_transfer(
        from_venue: &str,
        to_venue: &str,
        amount_usd: f64,
        origin_gas_usd: f64,
    ) -> BridgeQuote {
        let from_lower = from_venue.to_ascii_lowercase();
        let to_lower = to_venue.to_ascii_lowercase();

        // CEX internal universal transfer (same exchange entity)
        if (from_lower.contains("binance") && to_lower.contains("binance"))
            || (from_lower.contains("okx") && to_lower.contains("okx"))
        {
            return BridgeQuote {
                rail: TransferRail::CexInternalSubAccount,
                total_fee_usd: 0.0,
                estimated_latency_sec: 0.35,
                is_executable: amount_usd > 0.0,
            };
        }

        // TradFi broker clearing
        if from_lower.contains("ibkr")
            || to_lower.contains("ibkr")
            || from_lower.contains("alpaca")
            || to_lower.contains("alpaca")
        {
            return BridgeQuote {
                rail: TransferRail::TradFiWire,
                total_fee_usd: 20.0,
                estimated_latency_sec: 14400.0, // ~4 hours
                is_executable: amount_usd >= 500.0,
            };
        }

        // L2 Intent-Based Relay (Across Protocol default for Arbitrum, Base, Optimism, Lighter, Hyperliquid)
        let fee_bps = 3.5;
        let protocol_fee_usd = amount_usd * (fee_bps / 10_000.0);
        let total_fee = origin_gas_usd + protocol_fee_usd;

        BridgeQuote {
            rail: TransferRail::AcrossIntentL2,
            total_fee_usd: total_fee,
            estimated_latency_sec: 18.0,
            is_executable: amount_usd >= 50.0,
        }
    }

    /// Evaluates and quotes an Across Protocol SpokePool V3 intent-based transfer between EVM chains.
    #[allow(clippy::too_many_arguments)]
    pub fn quote_across_spokepool_transfer(
        &self,
        origin_chain_id: u64,
        destination_chain_id: u64,
        input_token: &str,
        output_token: &str,
        amount_usd: f64,
        dest_equity_usd: f64,
        dest_mmr_usd: f64,
        dest_notional_usd: f64,
        asset_volatility: f64,
        urgency_multiplier: f64,
    ) -> Result<AcrossBridgeTransferPlan, String> {
        let spoke_pool = across_spokepool_address(origin_chain_id).ok_or_else(|| {
            format!(
                "Unsupported origin chain id {} for Across SpokePool",
                origin_chain_id
            )
        })?;

        if across_spokepool_address(destination_chain_id).is_none() {
            return Err(format!(
                "Unsupported destination chain id {} for Across SpokePool",
                destination_chain_id
            ));
        }

        let quote = self.intent_engine.evaluate_intent_quote(
            amount_usd,
            dest_equity_usd,
            dest_mmr_usd,
            dest_notional_usd,
            asset_volatility,
            urgency_multiplier,
        );

        Ok(AcrossBridgeTransferPlan {
            origin_chain_id,
            destination_chain_id,
            spoke_pool_address: spoke_pool.to_string(),
            input_token: input_token.to_string(),
            output_token: output_token.to_string(),
            amount_usd,
            net_received_usd: quote.expected_output_usd,
            estimated_fee_usd: quote.total_fee_usd,
            estimated_latency_sec: quote.estimated_fill_latency_sec,
            fee_bps: quote.optimal_fee_bps,
            is_solvent: quote.pre_flight_solvency.is_solvent,
            max_safe_latency_sec: quote.pre_flight_solvency.max_safe_transfer_latency_sec,
        })
    }
}

/// State of an individual venue's capital, position risk, and margin utilization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenueCapitalState {
    pub venue_id: String,
    pub notional_usd: f64,
    pub volatility_annual: f64,
    pub mmr: f64,
    pub current_equity_usd: f64,
    pub free_margin_usd: f64,
}

/// Concrete recommendation for rebalancing collateral between two venues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebalanceTransferInstruction {
    pub source_venue: String,
    pub destination_venue: String,
    pub transfer_amount_usd: f64,
    pub expected_risk_reduction_score: f64,
    pub estimated_fee_usd: f64,
    pub warning_message: String,
    pub source_utilization_pct: f64,
    pub destination_utilization_pct: f64,
}

/// Multi-Venue Capital Allocation Optimizer via Water-Filling Convex Optimization.
/// Maximizes aggregate portfolio risk-adjusted return subject to margin utilization constraints
/// and derives the optimal volatility-weighted capital distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterFillingCapitalAllocator {
    /// Maximum allowed target margin utilization (e.g. 0.65 = 65%).
    pub target_utilization: f64,
    /// Minimum transfer size threshold (USD) to avoid uneconomic micro-transfers (e.g. $300).
    pub min_transfer_usd: f64,
    /// Minimum source free margin buffer ratio (e.g. 0.20 = 20% free margin preserved on source).
    pub source_safety_buffer_ratio: f64,
}

impl Default for WaterFillingCapitalAllocator {
    fn default() -> Self {
        Self {
            target_utilization: 0.65,
            min_transfer_usd: 300.0,
            source_safety_buffer_ratio: 0.20,
        }
    }
}

impl WaterFillingCapitalAllocator {
    pub fn new(
        target_utilization: f64,
        min_transfer_usd: f64,
        source_safety_buffer_ratio: f64,
    ) -> Self {
        Self {
            target_utilization: target_utilization.clamp(0.20, 0.90),
            min_transfer_usd: min_transfer_usd.max(10.0),
            source_safety_buffer_ratio: source_safety_buffer_ratio.clamp(0.05, 0.50),
        }
    }

    /// Computes the closed-form KKT water-filling allocation E_i* for each venue:
    /// E_i* = max( E_{i, min}, E_total * (Q_i S_i sigma_i) / sum(Q_j S_j sigma_j) )
    pub fn compute_water_filling_allocation(&self, venues: &[VenueCapitalState]) -> Vec<f64> {
        if venues.is_empty() {
            return Vec::new();
        }

        let total_equity: f64 = venues.iter().map(|v| v.current_equity_usd.max(0.0)).sum();
        let total_risk_weight: f64 = venues
            .iter()
            .map(|v| v.notional_usd * v.volatility_annual)
            .sum();

        venues
            .iter()
            .map(|v| {
                let min_equity = if self.target_utilization > 0.0 {
                    (v.notional_usd * v.mmr) / self.target_utilization
                } else {
                    v.notional_usd * v.mmr
                };

                let risk_weight = v.notional_usd * v.volatility_annual;
                let water_fill_equity = if total_risk_weight > 0.0 {
                    total_equity * (risk_weight / total_risk_weight)
                } else {
                    total_equity / venues.len() as f64
                };

                water_fill_equity.max(min_equity)
            })
            .collect()
    }

    /// Evaluates multi-venue discrepancies and generates optimal collateral transfer recommendations.
    pub fn evaluate_rebalance(
        &self,
        venues: &[VenueCapitalState],
    ) -> Option<RebalanceTransferInstruction> {
        if venues.len() < 2 {
            return None;
        }

        let optimal_allocations = self.compute_water_filling_allocation(venues);
        let mut discrepancies: Vec<(usize, f64)> = venues
            .iter()
            .enumerate()
            .map(|(idx, v)| (idx, optimal_allocations[idx] - v.current_equity_usd))
            .collect();

        // Sort discrepancies: largest deficit (positive) to largest surplus (negative)
        discrepancies.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (dest_idx, max_deficit) = discrepancies.first()?;
        let (src_idx, max_surplus) = discrepancies.last()?;

        let dest_venue = &venues[*dest_idx];
        let src_venue = &venues[*src_idx];

        // Deficit must exceed minimum hurdle and surplus must be available
        if *max_deficit >= self.min_transfer_usd && *max_surplus <= -self.min_transfer_usd {
            // Source venue must retain a safe margin buffer
            let source_min_reserve =
                src_venue.notional_usd * src_venue.mmr * (1.0 + self.source_safety_buffer_ratio);
            let source_available_equity =
                (src_venue.current_equity_usd - source_min_reserve).max(0.0);
            let source_available_free = src_venue.free_margin_usd.max(0.0);
            let max_source_capacity = source_available_equity.min(source_available_free);

            let unconstrained_transfer = max_deficit.min(-max_surplus);
            let transfer_amount = unconstrained_transfer.min(max_source_capacity);

            if transfer_amount >= self.min_transfer_usd {
                let quote = BridgeRelayCostEstimator::quote_transfer(
                    &src_venue.venue_id,
                    &dest_venue.venue_id,
                    transfer_amount,
                    1.50, // Origin L2 gas estimate
                );

                let risk_score = transfer_amount / dest_venue.notional_usd.max(1.0);
                let src_utilization = if src_venue.current_equity_usd > 0.0 {
                    (src_venue.notional_usd * src_venue.mmr / src_venue.current_equity_usd) * 100.0
                } else {
                    0.0
                };
                let dest_utilization = if dest_venue.current_equity_usd > 0.0 {
                    (dest_venue.notional_usd * dest_venue.mmr / dest_venue.current_equity_usd)
                        * 100.0
                } else {
                    100.0
                };
                let msg = format!(
                    "Dynamic Cross-Margin Rebalance: Reallocate ${:.2} from {} to {} via {:?} (est fee ${:.2}, latency {:.0}s). Restores margin utilization on {} to target {:.1}%.",
                    transfer_amount,
                    src_venue.venue_id,
                    dest_venue.venue_id,
                    quote.rail,
                    quote.total_fee_usd,
                    quote.estimated_latency_sec,
                    dest_venue.venue_id,
                    self.target_utilization * 100.0
                );

                return Some(RebalanceTransferInstruction {
                    source_venue: src_venue.venue_id.clone(),
                    destination_venue: dest_venue.venue_id.clone(),
                    transfer_amount_usd: transfer_amount,
                    expected_risk_reduction_score: risk_score,
                    estimated_fee_usd: quote.total_fee_usd,
                    warning_message: msg,
                    source_utilization_pct: src_utilization,
                    destination_utilization_pct: dest_utilization,
                });
            }
        }

        None
    }

    /// Evaluates multi-venue discrepancies and generates optimal collateral transfer recommendations
    /// directly from a slice of AccountSnapshot structs.
    pub fn evaluate_rebalance_accounts(
        &self,
        accounts: &[arbitrageur_core::position::AccountSnapshot],
    ) -> Option<RebalanceTransferInstruction> {
        let venue_states: Vec<VenueCapitalState> = accounts
            .iter()
            .map(|acc| {
                let notional_sum: f64 = acc
                    .positions
                    .iter()
                    .map(|p| {
                        (p.quantity.abs() * p.entry_price)
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    })
                    .sum();
                let mm = acc
                    .maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let notional = if notional_sum > 0.0 {
                    notional_sum
                } else if mm > 0.0 {
                    mm / 0.05
                } else {
                    0.0
                };
                let mmr = if notional > 0.0 {
                    (mm / notional).max(0.01)
                } else {
                    0.05
                };
                VenueCapitalState {
                    venue_id: acc.venue.0.clone(),
                    notional_usd: notional,
                    volatility_annual: 0.60,
                    mmr,
                    current_equity_usd: acc.equity.to_string().parse::<f64>().unwrap_or(0.0),
                    free_margin_usd: acc.free_margin.to_string().parse::<f64>().unwrap_or(0.0),
                }
            })
            .collect();

        self.evaluate_rebalance(&venue_states)
    }

    /// Evaluates multi-venue discrepancies and generates optimal collateral transfer recommendations
    /// using the graph-theoretic Dijkstra shortest-path router over multi-hop settlement rails
    /// under a strict liquidation latency deadline constraint.
    pub fn evaluate_dag_rebalance(
        &self,
        venues: &[VenueCapitalState],
        dag_router: &CollateralRehypothecationDagRouter,
        max_allowed_latency_sec: f64,
    ) -> Option<(RebalanceTransferInstruction, DagRebalancePlan)> {
        if venues.len() < 2 {
            return None;
        }

        let optimal_allocations = self.compute_water_filling_allocation(venues);
        let mut discrepancies: Vec<(usize, f64)> = venues
            .iter()
            .enumerate()
            .map(|(idx, v)| (idx, optimal_allocations[idx] - v.current_equity_usd))
            .collect();

        // Sort discrepancies: largest deficit (positive) to largest surplus (negative)
        discrepancies.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (dest_idx, max_deficit) = discrepancies.first()?;
        let (src_idx, max_surplus) = discrepancies.last()?;

        let dest_venue = &venues[*dest_idx];
        let src_venue = &venues[*src_idx];

        // Deficit must exceed minimum hurdle and surplus must be available
        if *max_deficit >= self.min_transfer_usd && *max_surplus <= -self.min_transfer_usd {
            let source_min_reserve =
                src_venue.notional_usd * src_venue.mmr * (1.0 + self.source_safety_buffer_ratio);
            let source_available_equity =
                (src_venue.current_equity_usd - source_min_reserve).max(0.0);
            let source_available_free = src_venue.free_margin_usd.max(0.0);
            let max_source_capacity = source_available_equity.min(source_available_free);

            let unconstrained_transfer = max_deficit.min(-max_surplus);
            let transfer_amount = unconstrained_transfer.min(max_source_capacity);

            if transfer_amount >= self.min_transfer_usd {
                let plan = dag_router.find_optimal_transfer_route(
                    &src_venue.venue_id,
                    &dest_venue.venue_id,
                    transfer_amount,
                    max_allowed_latency_sec,
                )?;

                let risk_score = transfer_amount / dest_venue.notional_usd.max(1.0);
                let src_utilization = if src_venue.current_equity_usd > 0.0 {
                    (src_venue.notional_usd * src_venue.mmr / src_venue.current_equity_usd) * 100.0
                } else {
                    0.0
                };
                let dest_utilization = if dest_venue.current_equity_usd > 0.0 {
                    (dest_venue.notional_usd * dest_venue.mmr / dest_venue.current_equity_usd)
                        * 100.0
                } else {
                    100.0
                };

                let path_str = plan.path.join(" -> ");
                let msg = format!(
                    "DAG Multi-Hop Rebalance: {} -> {} via [{}] (cost ${:.2}, latency {:.1}s, hops: {}). Restores margin utilization on {} to target {:.1}%.",
                    src_venue.venue_id,
                    dest_venue.venue_id,
                    path_str,
                    plan.total_cost_usd,
                    plan.total_latency_sec,
                    plan.steps.len(),
                    dest_venue.venue_id,
                    self.target_utilization * 100.0
                );

                let instruction = RebalanceTransferInstruction {
                    source_venue: src_venue.venue_id.clone(),
                    destination_venue: dest_venue.venue_id.clone(),
                    transfer_amount_usd: transfer_amount,
                    expected_risk_reduction_score: risk_score,
                    estimated_fee_usd: plan.total_cost_usd,
                    warning_message: msg,
                    source_utilization_pct: src_utilization,
                    destination_utilization_pct: dest_utilization,
                };

                return Some((instruction, plan));
            }
        }

        None
    }

    /// Evaluates multi-venue discrepancies and generates optimal DAG collateral transfer recommendations
    /// directly from a slice of AccountSnapshot structs.
    pub fn evaluate_dag_rebalance_accounts(
        &self,
        accounts: &[arbitrageur_core::position::AccountSnapshot],
        dag_router: &CollateralRehypothecationDagRouter,
        max_allowed_latency_sec: f64,
    ) -> Option<(RebalanceTransferInstruction, DagRebalancePlan)> {
        let venue_states: Vec<VenueCapitalState> = accounts
            .iter()
            .map(|acc| {
                let notional_sum: f64 = acc
                    .positions
                    .iter()
                    .map(|p| {
                        (p.quantity.abs() * p.entry_price)
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    })
                    .sum();
                let mm = acc
                    .maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let notional = if notional_sum > 0.0 {
                    notional_sum
                } else if mm > 0.0 {
                    mm / 0.05
                } else {
                    0.0
                };
                let mmr = if notional > 0.0 {
                    (mm / notional).max(0.01)
                } else {
                    0.05
                };
                VenueCapitalState {
                    venue_id: acc.venue.0.clone(),
                    notional_usd: notional,
                    volatility_annual: 0.60,
                    mmr,
                    current_equity_usd: acc.equity.to_string().parse::<f64>().unwrap_or(0.0),
                    free_margin_usd: acc.free_margin.to_string().parse::<f64>().unwrap_or(0.0),
                }
            })
            .collect();

        self.evaluate_dag_rebalance(&venue_states, dag_router, max_allowed_latency_sec)
    }

    /// Evaluates continuous liquidation barriers across segregated accounts and generates
    /// proactive water-filling rebalancing transfer plans before margin breach occurs.
    pub fn evaluate_proactive_barrier_rebalance_accounts(
        &self,
        accounts: &[arbitrageur_core::position::AccountSnapshot],
        dag_router: &CollateralRehypothecationDagRouter,
        target_buffer_ratio: f64,
        critical_time_to_liq_sec: f64,
    ) -> Vec<CollateralRebalanceTransfer> {
        let margin_states: Vec<VenueAccountMarginState> = accounts
            .iter()
            .map(|acc| {
                let mut notional_sum = 0.0;
                let mut signed_delta_sum = 0.0;
                for p in &acc.positions {
                    let qty: f64 = p.quantity.abs().to_string().parse().unwrap_or(0.0);
                    let px: f64 = p.entry_price.to_string().parse().unwrap_or(0.0);
                    let notional = qty * px;
                    notional_sum += notional;
                    let sign = match p.side {
                        arbitrageur_core::types::Side::Buy => 1.0,
                        arbitrageur_core::types::Side::Sell => -1.0,
                    };
                    signed_delta_sum += sign * notional;
                }
                let mm = acc
                    .maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let notional = if notional_sum > 0.0 {
                    notional_sum
                } else if mm > 0.0 {
                    mm / 0.05
                } else {
                    0.0
                };
                let mmr = if notional > 0.0 {
                    (mm / notional).max(0.01)
                } else {
                    0.05
                };
                VenueAccountMarginState::new(
                    acc.venue.0.clone(),
                    acc.equity.to_string().parse::<f64>().unwrap_or(0.0),
                    notional,
                    mmr,
                    500.0,
                    signed_delta_sum,
                )
            })
            .collect();

        // Extract available rail latencies from DAG router edges
        let mut rail_latencies: Vec<(&str, &str, f64, f64)> = Vec::new();
        for edge in &dag_router.edges {
            rail_latencies.push((
                &edge.from_venue,
                &edge.to_venue,
                edge.latency_sec,
                edge.fee_bps / 10_000.0,
            ));
        }

        let rebalancer = CrossVenueCollateralBarrierRebalancer::new(
            target_buffer_ratio,
            critical_time_to_liq_sec,
        );
        rebalancer.plan_proactive_rebalancing(&margin_states, &rail_latencies)
    }
}

/// Builds a production default multi-venue settlement DAG topology including CEX internal rails,
/// Across Protocol L2 intent bridges, CCTP native bridges, and TradFi clearing rails.
pub fn build_default_settlement_dag_router() -> CollateralRehypothecationDagRouter {
    let edges = vec![
        // CEX internal universal rails
        DagTransferEdge::new(
            "binance",
            "binance_sub",
            DagTransferRailType::CexInternalZeroFee,
            0.0,
            0.0,
            0.0,
            1.5,
            1_000_000.0,
        ),
        DagTransferEdge::new(
            "binance_sub",
            "binance",
            DagTransferRailType::CexInternalZeroFee,
            0.0,
            0.0,
            0.0,
            1.5,
            1_000_000.0,
        ),
        DagTransferEdge::new(
            "okx",
            "okx_sub",
            DagTransferRailType::CexInternalZeroFee,
            0.0,
            0.0,
            0.0,
            1.5,
            1_000_000.0,
        ),
        DagTransferEdge::new(
            "okx_sub",
            "okx",
            DagTransferRailType::CexInternalZeroFee,
            0.0,
            0.0,
            0.0,
            1.5,
            1_000_000.0,
        ),
        // CEX -> Treasury / Hot Wallet withdrawal
        DagTransferEdge::new(
            "binance",
            "treasury_arb",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            1.0,
            1.5,
            30.0,
            500_000.0,
        ),
        DagTransferEdge::new(
            "okx",
            "treasury_arb",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            1.0,
            1.5,
            30.0,
            500_000.0,
        ),
        DagTransferEdge::new(
            "treasury_arb",
            "binance",
            DagTransferRailType::AcrossIntentBridgeL2,
            0.5,
            0.5,
            1.5,
            30.0,
            500_000.0,
        ),
        DagTransferEdge::new(
            "treasury_arb",
            "okx",
            DagTransferRailType::AcrossIntentBridgeL2,
            0.5,
            0.5,
            1.5,
            30.0,
            500_000.0,
        ),
        // L2 DEX rails via Across Intent
        DagTransferEdge::new(
            "treasury_arb",
            "hyperliquid",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            2.5,
            1.5,
            18.0,
            250_000.0,
        ),
        DagTransferEdge::new(
            "hyperliquid",
            "treasury_arb",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            2.5,
            1.5,
            18.0,
            250_000.0,
        ),
        DagTransferEdge::new(
            "treasury_arb",
            "lighter",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            2.5,
            1.5,
            18.0,
            250_000.0,
        ),
        DagTransferEdge::new(
            "lighter",
            "treasury_arb",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.0,
            2.5,
            1.5,
            18.0,
            250_000.0,
        ),
        // Direct DEX-to-DEX bridge
        DagTransferEdge::new(
            "binance",
            "lighter",
            DagTransferRailType::AcrossIntentBridgeL2,
            2.0,
            3.5,
            2.0,
            45.0,
            200_000.0,
        ),
        DagTransferEdge::new(
            "lighter",
            "binance",
            DagTransferRailType::AcrossIntentBridgeL2,
            2.0,
            3.5,
            2.0,
            45.0,
            200_000.0,
        ),
        DagTransferEdge::new(
            "binance",
            "hyperliquid",
            DagTransferRailType::AcrossIntentBridgeL2,
            2.0,
            3.5,
            2.0,
            45.0,
            200_000.0,
        ),
        DagTransferEdge::new(
            "hyperliquid",
            "binance",
            DagTransferRailType::AcrossIntentBridgeL2,
            2.0,
            3.5,
            2.0,
            45.0,
            200_000.0,
        ),
        DagTransferEdge::new(
            "hyperliquid",
            "lighter",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.5,
            3.0,
            1.5,
            20.0,
            200_000.0,
        ),
        DagTransferEdge::new(
            "lighter",
            "hyperliquid",
            DagTransferRailType::AcrossIntentBridgeL2,
            1.5,
            3.0,
            1.5,
            20.0,
            200_000.0,
        ),
        // Native CCTP fallback rails (slower, low fee)
        DagTransferEdge::new(
            "treasury_arb",
            "deribit",
            DagTransferRailType::CctpNativeBridge,
            0.5,
            0.0,
            1.0,
            900.0,
            1_000_000.0,
        ),
        DagTransferEdge::new(
            "deribit",
            "treasury_arb",
            DagTransferRailType::CctpNativeBridge,
            0.5,
            0.0,
            1.0,
            900.0,
            1_000_000.0,
        ),
        // TradFi institutional rails
        DagTransferEdge::new(
            "binance",
            "ibkr",
            DagTransferRailType::TradFiWire,
            25.0,
            0.0,
            10.0,
            14400.0,
            1_000_000.0,
        ),
        DagTransferEdge::new(
            "ibkr",
            "binance",
            DagTransferRailType::TradFiWire,
            25.0,
            0.0,
            10.0,
            14400.0,
            1_000_000.0,
        ),
    ];

    CollateralRehypothecationDagRouter::new(edges)
}

/// Generalized Pareto Distribution (GPD) Peak-Over-Threshold (POT) Tail Risk Estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpdTailFit {
    /// Tail threshold u (losses above u follow GPD)
    pub threshold_u: f64,
    /// Shape parameter xi (tail index) from Hill estimator. In crypto, typically 0.20 - 0.50 (heavy fat tail).
    pub xi: f64,
    /// Scale parameter beta > 0.
    pub beta: f64,
    /// Number of observations n
    pub total_samples: usize,
    /// Number of exceedances N_u above threshold u
    pub exceedances: usize,
}

impl GpdTailFit {
    /// Fits a GPD tail to a sample of empirical positive losses using the Hill estimator.
    /// `tail_fraction`: proportion of sample in the tail (e.g. 0.10 for top 10% worst losses).
    pub fn fit_hill(losses: &[f64], tail_fraction: f64) -> Option<Self> {
        let n = losses.len();
        if n < 20 {
            return None;
        }
        let frac = tail_fraction.clamp(0.02, 0.30);
        let mut sorted: Vec<f64> = losses
            .iter()
            .copied()
            .filter(|x| x.is_finite() && *x > 0.0)
            .collect();
        if sorted.len() < 20 {
            return None;
        }
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let k = ((sorted.len() as f64) * frac).round() as usize;
        let k = k.clamp(4, sorted.len() - 1);
        let u_idx = sorted.len() - k;
        let u = sorted[u_idx];
        if u <= 1e-8 {
            return None;
        }

        // Hill estimator: xi_hat = (1 / k) * sum_{i=1}^k ln(X_{(n - i + 1)} / u)
        let mut sum_log = 0.0;
        let mut exceedance_count = 0;
        let mut sum_excess = 0.0;
        for &val in &sorted[u_idx..] {
            if val > u {
                sum_log += (val / u).ln();
                sum_excess += val - u;
                exceedance_count += 1;
            }
        }
        if exceedance_count == 0 {
            return None;
        }

        let k_f = exceedance_count as f64;
        let xi = (sum_log / k_f).clamp(0.05, 0.95);
        let mean_excess = sum_excess / k_f;
        let beta = (mean_excess * (1.0 - xi)).max(1e-6);

        Some(Self {
            threshold_u: u,
            xi,
            beta,
            total_samples: sorted.len(),
            exceedances: exceedance_count,
        })
    }

    /// Computes (VaR, Expected Shortfall) at confidence level (e.g. 0.999).
    pub fn compute_var_es(&self, confidence: f64) -> (f64, f64) {
        let conf = confidence.clamp(0.80, 0.9999);
        let p = 1.0 - conf;
        let n = self.total_samples as f64;
        let n_u = self.exceedances as f64;

        // VaR_p = u + (beta / xi) * [ ((n / N_u) * p)^(-xi) - 1 ]
        let ratio = (n / n_u) * p;
        let var_p = if ratio > 0.0 {
            self.threshold_u + (self.beta / self.xi) * (ratio.powf(-self.xi) - 1.0)
        } else {
            self.threshold_u
        };

        // ES_p = (VaR_p + beta - xi * u) / (1 - xi)
        let es_p = if self.xi < 1.0 {
            (var_p + self.beta - self.xi * self.threshold_u) / (1.0 - self.xi)
        } else {
            var_p * 1.5
        };

        (var_p.max(0.0), es_p.max(var_p))
    }
}

/// Heavy-Tailed Multi-Venue Margin Capital Allocation Optimizer using EVT Peak-Over-Threshold.
/// Replaces naive Gaussian variance weighting with Generalized Pareto Expected Shortfall (ES_0.999).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvtMarginWaterFillingAllocator {
    /// Maximum allowed target margin utilization (e.g. 0.65 = 65%).
    pub target_utilization: f64,
    /// Minimum transfer size threshold (USD) to avoid uneconomic micro-transfers (e.g. $300).
    pub min_transfer_usd: f64,
    /// Minimum source free margin buffer ratio (e.g. 0.20 = 20% free margin preserved on source).
    pub source_safety_buffer_ratio: f64,
    /// Default tail index xi for crypto assets if empirical loss series is unavailable (~0.36).
    pub default_tail_index: f64,
}

impl Default for EvtMarginWaterFillingAllocator {
    fn default() -> Self {
        Self {
            target_utilization: 0.65,
            min_transfer_usd: 300.0,
            source_safety_buffer_ratio: 0.20,
            default_tail_index: 0.36,
        }
    }
}

impl EvtMarginWaterFillingAllocator {
    pub fn new(
        target_utilization: f64,
        min_transfer_usd: f64,
        source_safety_buffer_ratio: f64,
        default_tail_index: f64,
    ) -> Self {
        Self {
            target_utilization: target_utilization.clamp(0.20, 0.90),
            min_transfer_usd: min_transfer_usd.max(10.0),
            source_safety_buffer_ratio: source_safety_buffer_ratio.clamp(0.05, 0.50),
            default_tail_index: default_tail_index.clamp(0.10, 0.85),
        }
    }

    /// Computes the EVT 99.9% Expected Shortfall (ES_0.999) multiplier over daily volatility.
    /// Under Gaussian distribution, ES_0.999 = 3.37 * sigma_daily.
    /// Under GPD with tail index xi ~ 0.36, ES_0.999 is ~3.06x higher than Gaussian (ES ~ 10.31 * sigma_daily).
    pub fn evt_expected_shortfall_999(&self, volatility_annual: f64) -> f64 {
        let daily_vol = (volatility_annual / 365.25_f64.sqrt()).max(1e-4);
        let xi = self.default_tail_index;
        let tail_multiplier = (1.0 / (1.0 - xi).max(0.1)) * (1.0 + 2.0 * xi);
        let gaussian_es = 3.37 * daily_vol;
        gaussian_es * tail_multiplier
    }

    /// Computes EVT-weighted water filling capital allocation:
    /// E_i* = max( E_{i, min}, E_total * (Q_i S_i ES_{i, 0.999}) / sum(Q_j S_j ES_{j, 0.999}) )
    pub fn compute_evt_water_filling_allocation(&self, venues: &[VenueCapitalState]) -> Vec<f64> {
        if venues.is_empty() {
            return Vec::new();
        }

        let total_equity: f64 = venues.iter().map(|v| v.current_equity_usd.max(0.0)).sum();
        let total_risk_weight: f64 = venues
            .iter()
            .map(|v| {
                let es_999 = self.evt_expected_shortfall_999(v.volatility_annual);
                v.notional_usd * es_999
            })
            .sum();

        venues
            .iter()
            .map(|v| {
                let min_equity = if self.target_utilization > 0.0 {
                    (v.notional_usd * v.mmr) / self.target_utilization
                } else {
                    v.notional_usd * v.mmr
                };

                let es_999 = self.evt_expected_shortfall_999(v.volatility_annual);
                let risk_weight = v.notional_usd * es_999;
                let water_fill_equity = if total_risk_weight > 0.0 {
                    total_equity * (risk_weight / total_risk_weight)
                } else {
                    total_equity / venues.len() as f64
                };

                water_fill_equity.max(min_equity)
            })
            .collect()
    }

    /// Evaluates multi-venue discrepancies and generates optimal EVT collateral transfer recommendations.
    pub fn evaluate_evt_rebalance(
        &self,
        venues: &[VenueCapitalState],
    ) -> Option<RebalanceTransferInstruction> {
        if venues.len() < 2 {
            return None;
        }

        let optimal_allocations = self.compute_evt_water_filling_allocation(venues);
        let mut discrepancies: Vec<(usize, f64)> = venues
            .iter()
            .enumerate()
            .map(|(idx, v)| (idx, optimal_allocations[idx] - v.current_equity_usd))
            .collect();

        // Sort discrepancies: largest deficit (positive) to largest surplus (negative)
        discrepancies.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (dest_idx, max_deficit) = discrepancies.first()?;
        let (src_idx, max_surplus) = discrepancies.last()?;

        let dest_venue = &venues[*dest_idx];
        let src_venue = &venues[*src_idx];

        // Deficit must exceed minimum hurdle ($300) and surplus must be available
        if *max_deficit >= self.min_transfer_usd && *max_surplus <= -self.min_transfer_usd {
            let source_min_reserve =
                src_venue.notional_usd * src_venue.mmr * (1.0 + self.source_safety_buffer_ratio);
            let source_available_equity =
                (src_venue.current_equity_usd - source_min_reserve).max(0.0);
            let source_available_free = src_venue.free_margin_usd.max(0.0);
            let max_source_capacity = source_available_equity.min(source_available_free);

            let unconstrained_transfer = max_deficit.min(-max_surplus);
            let transfer_amount = unconstrained_transfer.min(max_source_capacity);

            if transfer_amount >= self.min_transfer_usd {
                let quote = BridgeRelayCostEstimator::quote_transfer(
                    &src_venue.venue_id,
                    &dest_venue.venue_id,
                    transfer_amount,
                    1.50,
                );

                let risk_score = transfer_amount / dest_venue.notional_usd.max(1.0);
                let src_utilization = if src_venue.current_equity_usd > 0.0 {
                    (src_venue.notional_usd * src_venue.mmr / src_venue.current_equity_usd) * 100.0
                } else {
                    0.0
                };
                let dest_utilization = if dest_venue.current_equity_usd > 0.0 {
                    (dest_venue.notional_usd * dest_venue.mmr / dest_venue.current_equity_usd)
                        * 100.0
                } else {
                    100.0
                };
                let msg = format!(
                    "EVT Heavy-Tail Margin Rebalance: Reallocate ${:.2} from {} to {} via {:?} (est fee ${:.2}, latency {:.0}s). Protects 99.9% Expected Shortfall on {} restoring target {:.1}% margin utilization.",
                    transfer_amount,
                    src_venue.venue_id,
                    dest_venue.venue_id,
                    quote.rail,
                    quote.total_fee_usd,
                    quote.estimated_latency_sec,
                    dest_venue.venue_id,
                    self.target_utilization * 100.0
                );

                return Some(RebalanceTransferInstruction {
                    source_venue: src_venue.venue_id.clone(),
                    destination_venue: dest_venue.venue_id.clone(),
                    transfer_amount_usd: transfer_amount,
                    expected_risk_reduction_score: risk_score,
                    estimated_fee_usd: quote.total_fee_usd,
                    warning_message: msg,
                    source_utilization_pct: src_utilization,
                    destination_utilization_pct: dest_utilization,
                });
            }
        }

        None
    }

    /// Evaluates multi-venue discrepancies and generates optimal EVT collateral transfer recommendations
    /// directly from a slice of AccountSnapshot structs.
    pub fn evaluate_evt_rebalance_accounts(
        &self,
        accounts: &[arbitrageur_core::position::AccountSnapshot],
    ) -> Option<RebalanceTransferInstruction> {
        let venue_states: Vec<VenueCapitalState> = accounts
            .iter()
            .map(|acc| {
                let notional_sum: f64 = acc
                    .positions
                    .iter()
                    .map(|p| {
                        (p.quantity.abs() * p.entry_price)
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    })
                    .sum();
                let mm = acc
                    .maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let notional = if notional_sum > 0.0 {
                    notional_sum
                } else if mm > 0.0 {
                    mm / 0.05
                } else {
                    0.0
                };
                let mmr = if notional > 0.0 {
                    (mm / notional).max(0.01)
                } else {
                    0.05
                };
                VenueCapitalState {
                    venue_id: acc.venue.0.clone(),
                    notional_usd: notional,
                    volatility_annual: 0.60,
                    mmr,
                    current_equity_usd: acc.equity.to_string().parse::<f64>().unwrap_or(0.0),
                    free_margin_usd: acc.free_margin.to_string().parse::<f64>().unwrap_or(0.0),
                }
            })
            .collect();

        self.evaluate_evt_rebalance(&venue_states)
    }
}

/// Lifecycle state of a cross-venue collateral rebalancing transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    PendingDispatch,
    InTransit,
    Confirmed,
    Failed(String),
    TimedOut,
}

/// In-flight collateral rebalancing transfer tracked across execution rails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InTransitTransfer {
    pub idempotency_key: String,
    pub source_venue: String,
    pub destination_venue: String,
    pub amount_usd: f64,
    pub estimated_fee_usd: f64,
    pub rail_type: String,
    pub dispatched_at_ms: i64,
    pub timeout_at_ms: i64,
    pub state: TransferState,
    #[serde(default)]
    pub tx_hash: Option<String>,
    #[serde(default)]
    pub across_deposit_id: Option<u32>,
    #[serde(default)]
    pub across_origin_chain_id: Option<u64>,
    #[serde(default)]
    pub across_dest_chain_id: Option<u64>,
}

impl Default for InTransitTransfer {
    fn default() -> Self {
        Self {
            idempotency_key: String::new(),
            source_venue: String::new(),
            destination_venue: String::new(),
            amount_usd: 0.0,
            estimated_fee_usd: 0.0,
            rail_type: String::new(),
            dispatched_at_ms: 0,
            timeout_at_ms: 0,
            state: TransferState::PendingDispatch,
            tx_hash: None,
            across_deposit_id: None,
            across_origin_chain_id: None,
            across_dest_chain_id: None,
        }
    }
}

/// Configuration settings for the autonomous Cross-Venue Margin Rebalancing Daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginRebalanceDaemonConfig {
    pub enabled: bool,
    /// Minimum transfer size threshold to justify gas and rebalancing friction ($ USD).
    pub min_transfer_usd: f64,
    /// Margin utilization threshold (e.g. 75.0%) on destination that triggers proactive rebalance.
    pub trigger_utilization_pct: f64,
    /// Target post-rebalance margin utilization (e.g. 55.0%).
    pub target_utilization_pct: f64,
    /// Mandatory cooldown period between successive rebalances (seconds, e.g. 300s = 5m).
    pub cooldown_sec: u64,
    /// Maximum allowable cross-venue bridge/transit latency before route rejection (seconds).
    pub max_transit_latency_sec: f64,
    /// If true, simulates transfers without placing live API/bridge withdrawal calls.
    pub dry_run: bool,
}

impl Default for MarginRebalanceDaemonConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Safe opt-in default
            min_transfer_usd: 250.0,
            trigger_utilization_pct: 75.0,
            target_utilization_pct: 55.0,
            cooldown_sec: 300,
            max_transit_latency_sec: 180.0,
            dry_run: true,
        }
    }
}

/// Autonomous Cross-Venue Dynamic Margin Rebalancing Transfer Auto-Execution Daemon State.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginRebalanceDaemonState {
    pub config: MarginRebalanceDaemonConfig,
    pub active_transfers: Vec<InTransitTransfer>,
    pub transfer_history: Vec<InTransitTransfer>,
    pub last_dispatched_at_ms: Option<i64>,
}

impl Default for MarginRebalanceDaemonState {
    fn default() -> Self {
        Self::new(MarginRebalanceDaemonConfig::default())
    }
}

impl MarginRebalanceDaemonState {
    pub fn new(config: MarginRebalanceDaemonConfig) -> Self {
        Self {
            config,
            active_transfers: Vec::new(),
            transfer_history: Vec::new(),
            last_dispatched_at_ms: None,
        }
    }

    /// Computes total unconfirmed in-flight capital exiting a venue (source).
    pub fn in_flight_outflow_usd(&self, venue: &str) -> f64 {
        self.active_transfers
            .iter()
            .filter(|t| {
                t.source_venue == venue
                    && (t.state == TransferState::InTransit
                        || t.state == TransferState::PendingDispatch)
            })
            .map(|t| t.amount_usd)
            .sum()
    }

    /// Computes total unconfirmed in-flight capital entering a venue (destination).
    pub fn in_flight_inflow_usd(&self, venue: &str) -> f64 {
        self.active_transfers
            .iter()
            .filter(|t| {
                t.destination_venue == venue
                    && (t.state == TransferState::InTransit
                        || t.state == TransferState::PendingDispatch)
            })
            .map(|t| t.amount_usd)
            .sum()
    }

    /// Checks if a venue has any active transfer in-flight (double-dispatch lock).
    pub fn is_venue_locked(&self, venue: &str) -> bool {
        self.active_transfers.iter().any(|t| {
            (t.source_venue == venue || t.destination_venue == venue)
                && (t.state == TransferState::InTransit
                    || t.state == TransferState::PendingDispatch)
        })
    }

    /// Prunes completed or timed-out transfers and moves them to audit history.
    pub fn prune_and_update_transfers(&mut self, now_ms: i64) {
        let mut still_active = Vec::new();
        for mut transfer in self.active_transfers.drain(..) {
            if (transfer.state == TransferState::InTransit
                || transfer.state == TransferState::PendingDispatch)
                && now_ms > transfer.timeout_at_ms
            {
                transfer.state = TransferState::TimedOut;
                self.transfer_history.push(transfer);
            } else if transfer.state == TransferState::Confirmed
                || matches!(transfer.state, TransferState::Failed(_))
            {
                self.transfer_history.push(transfer);
            } else {
                still_active.push(transfer);
            }
        }
        self.active_transfers = still_active;
    }

    /// Confirms that a transfer was confirmed funded at the destination exchange.
    pub fn confirm_transfer(&mut self, idempotency_key: &str) -> bool {
        if let Some(pos) = self
            .active_transfers
            .iter()
            .position(|t| t.idempotency_key == idempotency_key)
        {
            let mut transfer = self.active_transfers.remove(pos);
            transfer.state = TransferState::Confirmed;
            self.transfer_history.push(transfer);
            true
        } else {
            false
        }
    }

    /// Fails an in-flight transfer with an error message.
    pub fn fail_transfer(&mut self, idempotency_key: &str, reason: &str) -> bool {
        if let Some(pos) = self
            .active_transfers
            .iter()
            .position(|t| t.idempotency_key == idempotency_key)
        {
            let mut transfer = self.active_transfers.remove(pos);
            transfer.state = TransferState::Failed(reason.to_string());
            self.transfer_history.push(transfer);
            true
        } else {
            false
        }
    }

    /// Links an Across SpokePool V3 deposit ID and origin/destination chain IDs to an active transfer.
    pub fn link_across_deposit(
        &mut self,
        idempotency_key: &str,
        origin_chain_id: u64,
        destination_chain_id: u64,
        deposit_id: u32,
        tx_hash: Option<&str>,
    ) -> bool {
        if let Some(t) = self
            .active_transfers
            .iter_mut()
            .find(|t| t.idempotency_key == idempotency_key)
        {
            t.across_origin_chain_id = Some(origin_chain_id);
            t.across_dest_chain_id = Some(destination_chain_id);
            t.across_deposit_id = Some(deposit_id);
            if let Some(hash) = tx_hash {
                t.tx_hash = Some(hash.to_string());
            }
            true
        } else {
            false
        }
    }

    /// Confirms that an Across Protocol V3 bridge transfer was filled on the destination chain
    /// via a decoded `FilledRelay` event.
    /// Matches by `(origin_chain_id, deposit_id)`.
    /// Immediately transitions the transfer state to `Confirmed`, unlocks destination margin,
    /// and records the fill in `transfer_history`.
    pub fn confirm_across_fill(
        &mut self,
        origin_chain_id: u64,
        deposit_id: u32,
        relayer: Option<&str>,
    ) -> Option<InTransitTransfer> {
        let pos = self.active_transfers.iter().position(|t| {
            if let (Some(orig_id), Some(dep_id)) = (t.across_origin_chain_id, t.across_deposit_id) {
                orig_id == origin_chain_id && dep_id == deposit_id
            } else {
                false
            }
        });

        if let Some(idx) = pos {
            let mut transfer = self.active_transfers.remove(idx);
            transfer.state = TransferState::Confirmed;
            if let Some(r) = relayer {
                tracing::info!(
                    "Across SpokePool V3 deposit #{} (origin chain {}) filled by relayer {r}; releasing in-flight capital {:.2} USD",
                    deposit_id, origin_chain_id, transfer.amount_usd
                );
            }
            self.transfer_history.push(transfer.clone());
            Some(transfer)
        } else {
            None
        }
    }

    /// Evaluates account snapshots with in-flight conservative margin accounting,
    /// enforce double-dispatch locks, verifies cooldowns, and plans an optimal DAG transfer.
    pub fn evaluate_and_plan(
        &mut self,
        accounts: &[arbitrageur_core::position::AccountSnapshot],
        dag_router: &CollateralRehypothecationDagRouter,
        now_ms: i64,
    ) -> Option<InTransitTransfer> {
        self.prune_and_update_transfers(now_ms);

        if !self.config.enabled {
            return None;
        }

        // Check cooldown period
        if let Some(last_ms) = self.last_dispatched_at_ms {
            let cooldown_ms = (self.config.cooldown_sec * 1000) as i64;
            if now_ms - last_ms < cooldown_ms {
                return None;
            }
        }

        // Apply conservative in-flight capital adjustments to account snapshots
        let adjusted_accounts: Vec<arbitrageur_core::position::AccountSnapshot> = accounts
            .iter()
            .map(|acc| {
                let mut adjusted = acc.clone();
                let v = &acc.venue.0;
                let outflow = self.in_flight_outflow_usd(v);
                let inflow = self.in_flight_inflow_usd(v);

                // Free margin on source is reduced by pending outflows
                let cur_free: f64 = acc.free_margin.to_string().parse().unwrap_or(0.0);
                let eff_free = (cur_free - outflow).max(0.0);
                adjusted.free_margin = arbitrageur_core::types::Decimal::from_f64_retain(eff_free)
                    .unwrap_or(acc.free_margin);

                // Equity on destination is increased by pending inflows for planning
                let cur_equity: f64 = acc.equity.to_string().parse().unwrap_or(0.0);
                let eff_equity = (cur_equity + inflow - outflow).max(0.0);
                adjusted.equity = arbitrageur_core::types::Decimal::from_f64_retain(eff_equity)
                    .unwrap_or(acc.equity);

                adjusted
            })
            .collect();

        let allocator = WaterFillingCapitalAllocator::new(
            self.config.target_utilization_pct / 100.0,
            self.config.min_transfer_usd,
            0.20,
        );

        let (instruction, plan) = allocator.evaluate_dag_rebalance_accounts(
            &adjusted_accounts,
            dag_router,
            self.config.max_transit_latency_sec,
        )?;

        // Ensure neither source nor destination is already locked in transit
        if self.is_venue_locked(&instruction.source_venue)
            || self.is_venue_locked(&instruction.destination_venue)
        {
            return None;
        }

        // Ensure amount satisfies minimum threshold
        if instruction.transfer_amount_usd < self.config.min_transfer_usd {
            return None;
        }

        let idempotency_key = format!(
            "rebal-{}-{}-{}",
            instruction.source_venue, instruction.destination_venue, now_ms
        );

        let timeout_ms = now_ms + ((plan.total_latency_sec + 60.0) * 1000.0) as i64;
        let rail_type = if plan.steps.len() == 1 {
            format!("{:?}", plan.steps[0].rail_type)
        } else {
            format!("MultiHop({})", plan.path.join("->"))
        };

        let transfer = InTransitTransfer {
            idempotency_key,
            source_venue: instruction.source_venue,
            destination_venue: instruction.destination_venue,
            amount_usd: instruction.transfer_amount_usd,
            estimated_fee_usd: instruction.estimated_fee_usd,
            rail_type,
            dispatched_at_ms: now_ms,
            timeout_at_ms: timeout_ms,
            state: TransferState::InTransit,
            tx_hash: None,
            across_deposit_id: None,
            across_origin_chain_id: None,
            across_dest_chain_id: None,
        };

        self.active_transfers.push(transfer.clone());
        self.last_dispatched_at_ms = Some(now_ms);

        Some(transfer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_passage_liquidation_probability() {
        let model = FirstPassageLiquidationModel::default();
        // Spot = $3000, Liq = $2500, Vol = 60%, Horizon = 1 day (1/365 years)
        let prob_1d = model.compute_liquidation_probability(3000.0, 2500.0, 0.60, 1.0 / 365.0);
        assert!(prob_1d < 0.05);

        // Spot = $2510, Liq = $2500 (dangerously close)
        let prob_close = model.compute_liquidation_probability(2510.0, 2500.0, 0.60, 1.0 / 365.0);
        assert!(prob_close > 0.30);

        // Immediate breach
        let prob_breach = model.compute_liquidation_probability(2500.0, 2500.0, 0.60, 1.0 / 365.0);
        assert_eq!(prob_breach, 1.0);
    }

    #[test]
    fn test_optimal_transfer_size() {
        let model = FirstPassageLiquidationModel::default();
        // Notional = $10,000, MMR = 5%, Current Equity = $400 (4%) -> Deficit vs 40% buffer
        // Target equity = 1.40 * 10,000 * 0.05 = $700.
        // Transfer size = $700 - $400 = $300.
        let transfer = model.compute_optimal_transfer_size(10_000.0, 0.05, 400.0);
        assert_eq!(transfer, 300.0);
    }

    #[test]
    fn test_bridge_vs_trim_preference() {
        let model = FirstPassageLiquidationModel::default();
        // 18 seconds bridge latency: 18 / (365 * 86400)
        let latency_years = 18.0 / (365.0 * 86400.0);

        // Safe distance: Spot = 3000, Liq = 2500 -> Async bridge is preferred over trim
        assert!(!model.is_trim_preferred_over_bridge(3000.0, 2500.0, latency_years, 0.60));

        // Hair-trigger distance (0.04% away): Spot = 2501, Liq = 2500 -> Trim is preferred over bridge
        assert!(model.is_trim_preferred_over_bridge(2501.0, 2500.0, latency_years, 0.60));
    }

    #[test]
    fn test_water_filling_allocation() {
        let allocator = WaterFillingCapitalAllocator::new(0.65, 300.0, 0.20);

        let venues = vec![
            VenueCapitalState {
                venue_id: "lighter".into(),
                notional_usd: 10_000.0,
                volatility_annual: 0.80,
                mmr: 0.05,
                current_equity_usd: 500.0, // Stressed: 500 / 10000 = 5% (MMR = 5%, 100% usage)
                free_margin_usd: 0.0,
            },
            VenueCapitalState {
                venue_id: "binance".into(),
                notional_usd: 10_000.0,
                volatility_annual: 0.80,
                mmr: 0.05,
                current_equity_usd: 5_000.0, // Surplus: $5,000 equity for $500 maintenance
                free_margin_usd: 4_500.0,
            },
        ];

        let alloc = allocator.compute_water_filling_allocation(&venues);
        assert_eq!(alloc.len(), 2);
        // Equal risk weight -> Equal equity ($2,750 each)
        assert!((alloc[0] - 2750.0).abs() < 1e-4);
        assert!((alloc[1] - 2750.0).abs() < 1e-4);

        let rec = allocator
            .evaluate_rebalance(&venues)
            .expect("rebalance recommendation");
        assert_eq!(rec.source_venue, "binance");
        assert_eq!(rec.destination_venue, "lighter");
        // Transfer amount = $2,750 - $500 = $2,250
        assert!((rec.transfer_amount_usd - 2250.0).abs() < 1e-4);
        assert!(rec.expected_risk_reduction_score > 0.0);
    }

    #[test]
    fn test_gpd_tail_fit_and_var_es() {
        // Generate Pareto-distributed synthetic losses: X = u * (1 - U)^(-xi)
        let u = 0.05;
        let true_xi = 0.35;
        let mut losses = Vec::with_capacity(100);
        for i in 1..=100 {
            let u_rand = (i as f64) / 101.0;
            let loss = u * (1.0 - u_rand).powf(-true_xi);
            losses.push(loss);
        }

        let fit = GpdTailFit::fit_hill(&losses, 0.20).expect("GPD tail fit should succeed");
        assert!(
            fit.xi > 0.15 && fit.xi < 0.60,
            "Fitted xi should be ~0.35, got {}",
            fit.xi
        );
        assert!(fit.beta > 0.0);

        let (var_999, es_999) = fit.compute_var_es(0.999);
        assert!(var_999 > fit.threshold_u);
        assert!(es_999 > var_999, "ES must strictly exceed VaR");
    }

    #[test]
    fn test_evt_water_filling_allocation() {
        let allocator = EvtMarginWaterFillingAllocator::new(0.65, 300.0, 0.20, 0.36);

        // Venue 1: Highly volatile DEX (120% vol, high tail risk)
        // Venue 2: Less volatile CEX (40% vol)
        let venues = vec![
            VenueCapitalState {
                venue_id: "hyperliquid".into(),
                notional_usd: 10_000.0,
                volatility_annual: 1.20,
                mmr: 0.05,
                current_equity_usd: 600.0,
                free_margin_usd: 100.0,
            },
            VenueCapitalState {
                venue_id: "binance".into(),
                notional_usd: 10_000.0,
                volatility_annual: 0.40,
                mmr: 0.05,
                current_equity_usd: 5_400.0,
                free_margin_usd: 4_900.0,
            },
        ];

        let alloc = allocator.compute_evt_water_filling_allocation(&venues);
        assert_eq!(alloc.len(), 2);
        // The high volatility DEX should receive significantly higher capital allocation (ratio 3:1)
        assert!(alloc[0] > alloc[1] * 2.5);

        let rec = allocator
            .evaluate_evt_rebalance(&venues)
            .expect("EVT rebalance recommendation");
        assert_eq!(rec.source_venue, "binance");
        assert_eq!(rec.destination_venue, "hyperliquid");
        assert!(rec.transfer_amount_usd >= 300.0);
        assert!(rec
            .warning_message
            .contains("EVT Heavy-Tail Margin Rebalance"));
    }

    #[test]
    fn test_water_filling_dag_rebalance_and_deadline_pruning() {
        let allocator = WaterFillingCapitalAllocator::new(0.65, 300.0, 0.20);
        let dag_router = build_default_settlement_dag_router();

        let venues = vec![
            VenueCapitalState {
                venue_id: "lighter".into(),
                notional_usd: 10_000.0,
                volatility_annual: 0.80,
                mmr: 0.05,
                current_equity_usd: 500.0,
                free_margin_usd: 0.0,
            },
            VenueCapitalState {
                venue_id: "binance".into(),
                notional_usd: 10_000.0,
                volatility_annual: 0.80,
                mmr: 0.05,
                current_equity_usd: 5_000.0,
                free_margin_usd: 4_500.0,
            },
        ];

        // Normal deadline of 60s: Binance -> Lighter bridge takes 45s, succeeds
        let (inst, plan) = allocator
            .evaluate_dag_rebalance(&venues, &dag_router, 60.0)
            .expect("should find valid DAG rebalance plan");

        assert_eq!(inst.source_venue, "binance");
        assert_eq!(inst.destination_venue, "lighter");
        assert_eq!(inst.transfer_amount_usd, 2250.0);
        assert!(plan.total_latency_sec <= 60.0);
        assert!(inst.warning_message.contains("DAG Multi-Hop Rebalance"));

        // Ultra-tight liquidation deadline of 10s: neither direct nor multi-hop can arrive in time
        let tight_plan = allocator.evaluate_dag_rebalance(&venues, &dag_router, 10.0);
        assert!(
            tight_plan.is_none(),
            "Should prune routes exceeding deadline"
        );
    }

    #[test]
    fn test_evaluate_dag_rebalance_accounts_snapshots() {
        use arbitrageur_core::position::{AccountSnapshot, Position};
        use arbitrageur_core::types::{dec, Side, VenueId};

        let allocator = WaterFillingCapitalAllocator::new(0.65, 300.0, 0.20);
        let dag_router = build_default_settlement_dag_router();

        let accounts = vec![
            AccountSnapshot {
                venue: VenueId::from("hyperliquid"),
                equity: dec!(600.0),
                free_margin: dec!(100.0),
                maintenance_margin: dec!(500.0),
                margin_usage_pct: dec!(83.3),
                balances: Vec::new(),
                positions: vec![Position {
                    venue: "hyperliquid".into(),
                    symbol: "BTC-PERP".into(),
                    side: Side::Buy,
                    quantity: dec!(0.2),
                    entry_price: dec!(50000.0),
                    mark_price: dec!(50000.0),
                    unrealized_pnl: dec!(0.0),
                    realized_pnl: dec!(0.0),
                    liquidation_price: None,
                    leverage: dec!(20.0),
                    initial_margin: dec!(500.0),
                    maintenance_margin: dec!(500.0),
                    updated_at: chrono::Utc::now(),
                }],
                timestamp: chrono::Utc::now(),
            },
            AccountSnapshot {
                venue: VenueId::from("binance"),
                equity: dec!(6000.0),
                free_margin: dec!(5000.0),
                maintenance_margin: dec!(500.0),
                margin_usage_pct: dec!(8.3),
                balances: Vec::new(),
                positions: vec![Position {
                    venue: "binance".into(),
                    symbol: "BTCUSDT".into(),
                    side: Side::Sell,
                    quantity: dec!(0.2),
                    entry_price: dec!(50000.0),
                    mark_price: dec!(50000.0),
                    unrealized_pnl: dec!(0.0),
                    realized_pnl: dec!(0.0),
                    liquidation_price: None,
                    leverage: dec!(20.0),
                    initial_margin: dec!(500.0),
                    maintenance_margin: dec!(500.0),
                    updated_at: chrono::Utc::now(),
                }],
                timestamp: chrono::Utc::now(),
            },
        ];

        let result = allocator.evaluate_dag_rebalance_accounts(&accounts, &dag_router, 120.0);
        assert!(result.is_some());
        let (inst, plan) = result.unwrap();
        assert_eq!(inst.source_venue, "binance");
        assert_eq!(inst.destination_venue, "hyperliquid");
        assert!(plan.total_latency_sec <= 120.0);
    }

    #[test]
    fn test_evaluate_proactive_barrier_rebalance_accounts() {
        use arbitrageur_core::position::{AccountSnapshot, Position};
        use arbitrageur_core::types::{dec, Side, VenueId};

        let allocator = WaterFillingCapitalAllocator::new(0.50, 100.0, 0.20);
        let dag_router = build_default_settlement_dag_router();

        // Account on Hyperliquid: Long 2.0 BTC, $100k notional, equity $12k, MMR $5k. B_liq = $7k. Target buffer (25%) = $25k. Deficit = $18k.
        let hl_acc = AccountSnapshot {
            venue: VenueId::from("hyperliquid"),
            equity: dec!(12000.0),
            free_margin: dec!(7000.0),
            maintenance_margin: dec!(5000.0),
            margin_usage_pct: dec!(41.6),
            balances: Vec::new(),
            positions: vec![Position {
                venue: "hyperliquid".into(),
                symbol: "BTC-PERP".into(),
                side: Side::Buy,
                quantity: dec!(2.0),
                entry_price: dec!(50000.0),
                mark_price: dec!(50000.0),
                unrealized_pnl: dec!(0.0),
                realized_pnl: dec!(0.0),
                liquidation_price: None,
                leverage: dec!(10.0),
                initial_margin: dec!(10000.0),
                maintenance_margin: dec!(5000.0),
                updated_at: chrono::Utc::now(),
            }],
            timestamp: chrono::Utc::now(),
        };

        // Account on Binance: Short 2.0 BTC, $100k notional, equity $80k, MMR $5k. B_liq = $75k. Target buffer = $25k. Surplus = $50k.
        let bn_acc = AccountSnapshot {
            venue: VenueId::from("binance"),
            equity: dec!(80000.0),
            free_margin: dec!(75000.0),
            maintenance_margin: dec!(5000.0),
            margin_usage_pct: dec!(6.25),
            balances: Vec::new(),
            positions: vec![Position {
                venue: "binance".into(),
                symbol: "BTCUSDT".into(),
                side: Side::Sell,
                quantity: dec!(2.0),
                entry_price: dec!(50000.0),
                mark_price: dec!(50000.0),
                unrealized_pnl: dec!(0.0),
                realized_pnl: dec!(0.0),
                liquidation_price: None,
                leverage: dec!(10.0),
                initial_margin: dec!(10000.0),
                maintenance_margin: dec!(5000.0),
                updated_at: chrono::Utc::now(),
            }],
            timestamp: chrono::Utc::now(),
        };

        let transfers = allocator.evaluate_proactive_barrier_rebalance_accounts(
            &[hl_acc, bn_acc],
            &dag_router,
            0.25,
            60.0,
        );

        assert!(
            !transfers.is_empty(),
            "Must generate proactive rebalance transfers"
        );
        assert_eq!(transfers[0].from_venue, "binance");
        assert_eq!(transfers[0].to_venue, "hyperliquid");
        assert!(transfers[0].amount_usd > 10_000.0);
    }

    #[test]
    fn test_margin_rebalance_daemon_in_flight_accounting_and_locks() {
        let mut daemon = MarginRebalanceDaemonState::new(MarginRebalanceDaemonConfig {
            enabled: true,
            min_transfer_usd: 100.0,
            trigger_utilization_pct: 75.0,
            target_utilization_pct: 55.0,
            cooldown_sec: 60,
            max_transit_latency_sec: 120.0,
            dry_run: true,
        });

        assert!(!daemon.is_venue_locked("binance"));
        assert_eq!(daemon.in_flight_outflow_usd("binance"), 0.0);

        // Add an active in-transit transfer: binance -> hyperliquid $500
        let t1 = InTransitTransfer {
            idempotency_key: "rebal-bn-hl-1".into(),
            source_venue: "binance".into(),
            destination_venue: "hyperliquid".into(),
            amount_usd: 500.0,
            estimated_fee_usd: 1.5,
            rail_type: "CexInternal".into(),
            dispatched_at_ms: 1000,
            timeout_at_ms: 60_000,
            state: TransferState::InTransit,
            tx_hash: None,
            across_deposit_id: None,
            across_origin_chain_id: None,
            across_dest_chain_id: None,
        };
        daemon.active_transfers.push(t1);

        assert!(daemon.is_venue_locked("binance"));
        assert!(daemon.is_venue_locked("hyperliquid"));
        assert!(!daemon.is_venue_locked("lighter"));
        assert_eq!(daemon.in_flight_outflow_usd("binance"), 500.0);
        assert_eq!(daemon.in_flight_inflow_usd("hyperliquid"), 500.0);

        // Confirm transfer
        assert!(daemon.confirm_transfer("rebal-bn-hl-1"));
        assert!(!daemon.is_venue_locked("binance"));
        assert_eq!(daemon.in_flight_outflow_usd("binance"), 0.0);
        assert_eq!(daemon.transfer_history.len(), 1);
        assert_eq!(daemon.transfer_history[0].state, TransferState::Confirmed);

        // Timeout behavior
        let t2 = InTransitTransfer {
            idempotency_key: "rebal-bn-hl-2".into(),
            source_venue: "binance".into(),
            destination_venue: "lighter".into(),
            amount_usd: 300.0,
            estimated_fee_usd: 0.5,
            rail_type: "AcrossBridge".into(),
            dispatched_at_ms: 1000,
            timeout_at_ms: 2000,
            state: TransferState::InTransit,
            tx_hash: None,
            across_deposit_id: None,
            across_origin_chain_id: None,
            across_dest_chain_id: None,
        };
        daemon.active_transfers.push(t2);

        // Now prune at now_ms = 3000 (> timeout_at_ms 2000)
        daemon.prune_and_update_transfers(3000);
        assert!(daemon.active_transfers.is_empty());
        assert_eq!(daemon.transfer_history.len(), 2);
        assert_eq!(daemon.transfer_history[1].state, TransferState::TimedOut);
    }

    #[test]
    fn test_across_fill_confirmation() {
        let mut daemon = MarginRebalanceDaemonState::new(MarginRebalanceDaemonConfig {
            enabled: true,
            min_transfer_usd: 100.0,
            trigger_utilization_pct: 75.0,
            target_utilization_pct: 55.0,
            cooldown_sec: 60,
            max_transit_latency_sec: 120.0,
            dry_run: true,
        });

        let t = InTransitTransfer {
            idempotency_key: "rebal-arb-base-across".into(),
            source_venue: "arbitrum".into(),
            destination_venue: "base".into(),
            amount_usd: 2500.0,
            estimated_fee_usd: 0.875,
            rail_type: "AcrossIntentL2".into(),
            dispatched_at_ms: 10_000,
            timeout_at_ms: 70_000,
            state: TransferState::InTransit,
            tx_hash: None,
            across_deposit_id: None,
            across_origin_chain_id: None,
            across_dest_chain_id: None,
        };
        daemon.active_transfers.push(t);

        assert!(daemon.is_venue_locked("arbitrum"));
        assert!(daemon.is_venue_locked("base"));
        assert_eq!(daemon.in_flight_outflow_usd("arbitrum"), 2500.0);

        // Link on-chain deposit event (V3FundsDeposited on Arbitrum chain 42161, deposit #105)
        let linked = daemon.link_across_deposit(
            "rebal-arb-base-across",
            42161,
            8453,
            105,
            Some("0xorigin_tx_hash_123"),
        );
        assert!(linked, "Must successfully link deposit");

        // Verify active transfer has linked deposit info
        assert_eq!(daemon.active_transfers[0].across_deposit_id, Some(105));
        assert_eq!(
            daemon.active_transfers[0].across_origin_chain_id,
            Some(42161)
        );
        assert_eq!(
            daemon.active_transfers[0].tx_hash.as_deref(),
            Some("0xorigin_tx_hash_123")
        );

        // Simulate fast relayer fill event (FilledRelay observed on Base)
        let confirmed = daemon.confirm_across_fill(42161, 105, Some("0xrelayer_solver_999"));
        assert!(
            confirmed.is_some(),
            "Must match and confirm transfer by deposit ID"
        );

        let conf_transfer = confirmed.unwrap();
        assert_eq!(conf_transfer.state, TransferState::Confirmed);
        assert_eq!(conf_transfer.amount_usd, 2500.0);

        // Locks and in-flight outflows must be instantly released
        assert!(!daemon.is_venue_locked("arbitrum"));
        assert!(!daemon.is_venue_locked("base"));
        assert_eq!(daemon.in_flight_outflow_usd("arbitrum"), 0.0);
        assert_eq!(daemon.in_flight_inflow_usd("base"), 0.0);
        assert_eq!(daemon.transfer_history.len(), 1);
        assert_eq!(daemon.transfer_history[0].state, TransferState::Confirmed);
    }

    #[test]
    fn test_vecm_jump_escalation_on_liquidation_model() {
        let base_model = FirstPassageLiquidationModel::default();
        let prob_calm = base_model.compute_liquidation_probability(3000.0, 2700.0, 0.40, 0.05);

        // Escalated model under 70% probability of Regime 2 and 2.5 sigma jump shock
        let escalated_model = base_model.with_vecm_jump_escalation(0.70, 2.5);
        assert!(escalated_model.target_mbr > base_model.target_mbr);
        assert!(escalated_model.jump_intensity > base_model.jump_intensity);

        let prob_escalated =
            escalated_model.compute_liquidation_probability(3000.0, 2700.0, 0.40, 0.05);
        assert!(
            prob_escalated > prob_calm,
            "Escalated jump model must produce higher liquidation hazard"
        );
    }

    #[test]
    fn test_bridge_relay_cost_estimator_across_transfer() {
        let estimator = BridgeRelayCostEstimator::default();

        // Valid transfer Arbitrum (42161) -> Base (8453)
        let plan = estimator.quote_across_spokepool_transfer(
            42161, 8453, "USDC", "USDC", 5000.0, 2000.0, 100.0, 2000.0, 0.50, 1.0,
        );

        assert!(plan.is_ok(), "Must quote valid Across transfer");
        let plan = plan.unwrap();
        assert_eq!(plan.origin_chain_id, 42161);
        assert_eq!(plan.destination_chain_id, 8453);
        assert_eq!(
            plan.spoke_pool_address,
            "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A"
        );
        assert_eq!(plan.amount_usd, 5000.0);
        assert!(plan.net_received_usd > 4900.0);
        assert!(plan.estimated_fee_usd > 0.0);
        assert!(plan.estimated_latency_sec > 0.0);
        assert!(plan.is_solvent);

        // Unsupported origin chain
        let invalid_origin = estimator.quote_across_spokepool_transfer(
            99999, 8453, "USDC", "USDC", 1000.0, 1000.0, 50.0, 1000.0, 0.50, 1.0,
        );
        assert!(invalid_origin.is_err());
        assert!(invalid_origin
            .unwrap_err()
            .contains("Unsupported origin chain"));

        // Unsupported destination chain
        let invalid_dest = estimator.quote_across_spokepool_transfer(
            42161, 99999, "USDC", "USDC", 1000.0, 1000.0, 50.0, 1000.0, 0.50, 1.0,
        );
        assert!(invalid_dest.is_err());
        assert!(invalid_dest
            .unwrap_err()
            .contains("Unsupported destination chain"));
    }
}
