//! Proportional Symmetric Margin De-leveraging Controller for Segregated Cross-Venue Positions.
//!
//! Solves the fundamental failure mode of delta-neutral cross-venue carry and spread strategies:
//! Collateral pools are strictly segregated per venue. Under extreme market-wide price swings,
//! unrealized PnL shifts from one venue to the other. Without active rebalancing, the losing leg
//! breaches its maintenance margin barrier and gets liquidated, even though the net portfolio
//! is delta-neutral and overall profitable!
//!
//! This module provides closed-form solutions for:
//! 1. Exact liquidation price bounds for Long and Short perpetual positions.
//! 2. Dynamic Margin Buffer Reservation (MBR) monitoring.
//! 3. Closed-form proportional deleveraging quantity $\Delta Q^*$, reducing both legs
//!    symmetrically to restore healthy margin buffers without creating unhedged delta.

use serde::{Deserialize, Serialize};

/// State of an individual venue's margin account for a specific position leg.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegMarginState {
    pub venue_id: String,
    pub is_long: bool,
    #[serde(default)]
    pub is_spot: bool,
    pub position_qty: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub current_equity_usd: f64,
    pub maintenance_margin_rate: f64,
    pub taker_fee_rate: f64,
}

impl LegMarginState {
    /// Computes notional value in USD.
    pub fn notional_usd(&self) -> f64 {
        self.position_qty.abs() * self.mark_price
    }

    /// Computes required maintenance margin in USD.
    /// Physical spot longs require zero derivative maintenance margin.
    pub fn maintenance_margin_usd(&self) -> f64 {
        if self.is_spot && self.is_long {
            0.0
        } else {
            self.notional_usd() * self.maintenance_margin_rate
        }
    }

    /// Computes the exact liquidation price for this position.
    ///
    /// Long:  $P_{\text{liq}} = \frac{P_0 Q - E_0 + \text{fee}}{Q (1 - \text{MMR})}$
    /// Short: $P_{\text{liq}} = \frac{P_0 Q + E_0 - \text{fee}}{Q (1 + \text{MMR})}$
    /// Physical spot longs have no liquidation price (return 0.0).
    pub fn liquidation_price(&self) -> f64 {
        if self.is_spot && self.is_long {
            return 0.0;
        }
        let q = self.position_qty.abs();
        if q <= 1e-12 {
            return 0.0;
        }
        let fee = self.notional_usd() * self.taker_fee_rate;

        if self.is_long {
            let denom = q * (1.0 - self.maintenance_margin_rate);
            if denom <= 1e-12 {
                0.0
            } else {
                ((self.entry_price * q - self.current_equity_usd + fee) / denom).max(0.0)
            }
        } else {
            let denom = q * (1.0 + self.maintenance_margin_rate);
            if denom <= 1e-12 {
                0.0
            } else {
                ((self.entry_price * q + self.current_equity_usd - fee) / denom).max(0.0)
            }
        }
    }

    /// Computes the current margin buffer ratio: $(E / \text{MM}) - 1.0$.
    /// A buffer of 0.50 means equity is 150% of maintenance margin (50% headroom).
    /// Physical spot longs have infinite/abundant margin buffer (returns 100.0).
    pub fn margin_buffer_ratio(&self) -> f64 {
        if self.is_spot && self.is_long {
            return 100.0;
        }
        let mm = self.maintenance_margin_usd();
        if mm <= 1e-12 {
            10.0 // Abundant margin
        } else {
            self.current_equity_usd / mm - 1.0
        }
    }
}

/// Action to restore cross-venue margin safety.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarginDeleveragingDecision {
    /// Both legs have healthy margin buffers ($\ge \text{target\_mbr}$).
    Safe {
        long_buffer_ratio: f64,
        short_buffer_ratio: f64,
    },
    /// One venue is threatened: execute immediate symmetric reduce-only partial unwinds
    /// on BOTH venues for `unwind_qty` to restore the stressed venue's buffer to target.
    SymmetricPartialUnwind {
        stressed_venue: String,
        unwind_qty: f64,
        remaining_qty: f64,
        stressed_buffer_ratio: f64,
        target_buffer_ratio: f64,
    },
    /// Critical breach: current equity is below maintenance margin. Flatten entire position.
    EmergencyFullFlatten {
        stressed_venue: String,
        current_equity_usd: f64,
        required_mm_usd: f64,
        reason: String,
    },
}

/// Dynamic Cross-Venue Margin Deleveraging Controller.
#[derive(Debug, Clone)]
pub struct ProportionalDeleveragingController {
    /// Target margin buffer reservation (e.g. 0.35 = 35% margin buffer above MMR).
    pub target_mbr: f64,
}

impl Default for ProportionalDeleveragingController {
    fn default() -> Self {
        Self { target_mbr: 0.35 }
    }
}

impl ProportionalDeleveragingController {
    pub fn new(target_mbr: f64) -> Self {
        Self {
            target_mbr: target_mbr.clamp(0.10, 1.00),
        }
    }

    /// Evaluates a two-leg delta-neutral position and derives the exact minimal
    /// symmetric partial deleveraging quantity $\Delta Q^*$.
    ///
    /// Derivation:
    /// Post-deleveraging target equity:
    /// $$E_{\text{post}} = E - \Delta Q^* \cdot P \cdot f_{\text{taker}}$$
    /// Post-deleveraging required margin:
    /// $$MM_{\text{post}} = (Q - \Delta Q^*) \cdot P \cdot MMR$$
    /// Enforcing $E_{\text{post}} = (1 + MBR^*) MM_{\text{post}}$:
    /// $$E - \Delta Q^* P f_{\text{taker}} = (1 + MBR^*) (Q - \Delta Q^*) P \cdot MMR$$
    /// Solving for $\Delta Q^*$:
    /// $$\Delta Q^* = \frac{(1 + MBR^*) Q P \cdot MMR - E}{P \cdot [(1 + MBR^*) MMR - f_{\text{taker}}]}$$
    pub fn evaluate_pair_margin(
        &self,
        long_leg: &LegMarginState,
        short_leg: &LegMarginState,
    ) -> MarginDeleveragingDecision {
        let long_buf = long_leg.margin_buffer_ratio();
        let short_buf = short_leg.margin_buffer_ratio();

        // Check for catastrophic breach first
        if long_leg.current_equity_usd <= long_leg.maintenance_margin_usd() {
            return MarginDeleveragingDecision::EmergencyFullFlatten {
                stressed_venue: long_leg.venue_id.clone(),
                current_equity_usd: long_leg.current_equity_usd,
                required_mm_usd: long_leg.maintenance_margin_usd(),
                reason: "Long leg equity breached maintenance margin requirement".into(),
            };
        }
        if short_leg.current_equity_usd <= short_leg.maintenance_margin_usd() {
            return MarginDeleveragingDecision::EmergencyFullFlatten {
                stressed_venue: short_leg.venue_id.clone(),
                current_equity_usd: short_leg.current_equity_usd,
                required_mm_usd: short_leg.maintenance_margin_usd(),
                reason: "Short leg equity breached maintenance margin requirement".into(),
            };
        }

        // Determine which leg is under more margin stress
        let (stressed, min_buf) = if long_buf < short_buf {
            (long_leg, long_buf)
        } else {
            (short_leg, short_buf)
        };

        if min_buf >= self.target_mbr {
            return MarginDeleveragingDecision::Safe {
                long_buffer_ratio: long_buf,
                short_buffer_ratio: short_buf,
            };
        }

        // Compute exact closed-form symmetric deleveraging quantity Delta_Q*
        let q = stressed.position_qty.abs();
        let p = stressed.mark_price;
        let mmr = stressed.maintenance_margin_rate;
        let fee = stressed.taker_fee_rate;
        let e = stressed.current_equity_usd;

        let numerator = (1.0 + self.target_mbr) * q * p * mmr - e;
        let denominator = p * ((1.0 + self.target_mbr) * mmr - fee);

        if denominator <= 1e-9 {
            return MarginDeleveragingDecision::EmergencyFullFlatten {
                stressed_venue: stressed.venue_id.clone(),
                current_equity_usd: e,
                required_mm_usd: stressed.maintenance_margin_usd(),
                reason: "Mathematical denominator singularity in margin solver".into(),
            };
        }

        let unwind_qty = (numerator / denominator).clamp(0.0, q);
        let remaining_qty = (q - unwind_qty).max(0.0);

        MarginDeleveragingDecision::SymmetricPartialUnwind {
            stressed_venue: stressed.venue_id.clone(),
            unwind_qty,
            remaining_qty,
            stressed_buffer_ratio: min_buf,
            target_buffer_ratio: self.target_mbr,
        }
    }
}

/// Continuous-time liquidation risk and dynamic margin buffer reservation evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LiquidationRiskEvaluator;

impl LiquidationRiskEvaluator {
    /// Standard normal CDF approximation (Abramowitz & Stegun).
    pub fn norm_cdf(x: f64) -> f64 {
        if !x.is_finite() {
            return if x > 0.0 { 1.0 } else { 0.0 };
        }
        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;

        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x_abs = x.abs() / std::f64::consts::SQRT_2;
        let t = 1.0 / (1.0 + p * x_abs);
        let erf =
            1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x_abs * x_abs).exp();

        0.5 * (1.0 + sign * erf)
    }

    /// Computes probability that the mark price hits the liquidation barrier before horizon $T$:
    /// $$\mathbb{P}(\tau_{\text{liq}} \le T) = 2 \left(1 - \Phi\left(\frac{b}{\sigma \sqrt{T}}\right)\right)$$
    /// where $b = |\ln(P_{\text{liq}} / P_{\text{current}})|$.
    ///
    /// * `p_current`: current mark price (> 0).
    /// * `p_liq`: position liquidation price (> 0).
    /// * `annualized_vol`: annualized volatility $\sigma$ (> 0).
    /// * `horizon_years`: time horizon in years (e.g. 14.0 / 365.0 for a 14-day hold).
    pub fn prob_liquidation_touch(
        p_current: f64,
        p_liq: f64,
        annualized_vol: f64,
        horizon_years: f64,
    ) -> f64 {
        if p_current <= 0.0 || p_liq <= 0.0 || annualized_vol <= 0.0 || horizon_years <= 0.0 {
            return 0.0;
        }

        let b = (p_liq / p_current).ln().abs();
        let denom = annualized_vol * horizon_years.sqrt();
        if denom <= 1e-9 {
            return 0.0;
        }

        let z = b / denom;
        (2.0 * (1.0 - Self::norm_cdf(z))).clamp(0.0, 1.0)
    }

    /// Dynamic Margin Reservation in USD required to absorb a $k$-sigma price shock
    /// over the cross-venue rebalance latency window:
    /// $$E_{\text{reserve}} = Q \cdot P \cdot (k_{\text{shock}} \cdot \sigma_{\text{hourly}} \sqrt{T_{\text{rebal}}} + \text{MMR})$$
    pub fn dynamic_reserve_equity_usd(
        position_notional_usd: f64,
        mmr: f64,
        hourly_volatility: f64,
        rebalance_hours: f64,
        k_shock_sigmas: f64,
    ) -> f64 {
        let shock =
            k_shock_sigmas.max(1.0) * hourly_volatility.max(0.0) * rebalance_hours.max(0.0).sqrt();
        position_notional_usd.max(0.0) * (shock + mmr.max(0.0))
    }

    /// Computes probability of touching liquidation barrier incorporating EVT heavy tails:
    /// Under GPD tail parameter xi, the survival probability follows a power-law tail:
    /// $$P_{\text{touch}}^{\text{EVT}} = \min\left(1.0, P_{\text{Gaussian}} \cdot \frac{1 + \xi \cdot z}{1 - \xi}\right)$$
    pub fn prob_liquidation_touch_evt(
        p_current: f64,
        p_liq: f64,
        annualized_vol: f64,
        horizon_years: f64,
        tail_index_xi: f64,
    ) -> f64 {
        let p_gauss = Self::prob_liquidation_touch(p_current, p_liq, annualized_vol, horizon_years);
        if p_gauss <= 0.0 || p_gauss >= 1.0 {
            return p_gauss;
        }
        let xi = tail_index_xi.clamp(0.10, 0.85);
        let b = (p_liq / p_current).ln().abs();
        let denom = (annualized_vol * horizon_years.sqrt()).max(1e-9);
        let z = b / denom;
        let tail_inflation = ((1.0 + xi * z) / (1.0 - xi)).max(1.0);
        (p_gauss * tail_inflation).clamp(0.0, 1.0)
    }

    /// EVT Heavy-Tailed Dynamic Margin Reservation in USD required to absorb extreme tail events
    /// at 99.9% Expected Shortfall under Generalized Pareto Distribution (GPD) with tail index xi:
    /// In crypto assets (xi ~ 0.36), extreme excursion ES_0.999 is ~3.06x higher than Gaussian:
    /// $$E_{\text{reserve}}^{\text{EVT}} = Q \cdot P \cdot \left( ES_{0.999} \cdot \sqrt{T_{\text{rebal}} / T_{\text{base}}} + \text{MMR} \right)$$
    pub fn evt_heavy_tail_reserve_equity_usd(
        position_notional_usd: f64,
        mmr: f64,
        hourly_volatility: f64,
        rebalance_hours: f64,
        tail_index_xi: f64,
    ) -> f64 {
        let xi = tail_index_xi.clamp(0.10, 0.85);
        let tail_multiplier = (1.0 / (1.0 - xi).max(0.10)) * (1.0 + 2.0 * xi);
        let base_shock = 3.37 * hourly_volatility.max(0.0) * rebalance_hours.max(0.0).sqrt();
        let evt_shock = base_shock * tail_multiplier;
        position_notional_usd.max(0.0) * (evt_shock + mmr.max(0.0))
    }
}

/// Venue account margin state for cross-venue portfolio monitoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VenueAccountState {
    pub venue_id: String,
    pub total_equity_usd: f64,
    pub maintenance_margin_required_usd: f64,
    pub initial_margin_required_usd: f64,
    pub total_notional_usd: f64,
    pub hourly_equity_volatility: f64,
    pub is_dex_perp: bool,
}

impl VenueAccountState {
    /// Non-linear margin utilization: $U_m = \text{MMR}_m / E_m$.
    pub fn margin_utilization(&self) -> f64 {
        if self.total_equity_usd <= 1e-12 {
            10.0 // Distressed insolvency
        } else {
            self.maintenance_margin_required_usd / self.total_equity_usd
        }
    }

    /// Free equity headroom above maintenance margin requirement in USD.
    pub fn equity_headroom_usd(&self) -> f64 {
        self.total_equity_usd - self.maintenance_margin_required_usd
    }
}

/// Analytical first-passage ruin barrier during in-flight collateral transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstPassageRuinBarrier {
    /// Target survival probability level (e.g. 0.9999 for 99.99%).
    pub confidence_level: f64,
    /// Standard normal inverse quantile $z_\alpha = \Phi^{-1}(1 - \alpha/2)$.
    pub z_alpha: f64,
    /// First-passage safety buffer in USD:
    /// $$B_{\text{rebal}}^* = z_\alpha \cdot \sigma_E \cdot \sqrt{\tau_{\text{transfer}}} + \text{TransferFees}$$
    pub barrier_usd: f64,
    /// Fixed transfer and gas fees.
    pub transfer_fees_usd: f64,
    /// In-flight transfer transit duration in hours.
    pub transfer_latency_hours: f64,
}

impl FirstPassageRuinBarrier {
    /// Computes the closed-form first-passage ruin barrier for a given account.
    pub fn compute(
        account: &VenueAccountState,
        transfer_latency_hours: f64,
        transfer_fees_usd: f64,
        confidence_level: f64,
    ) -> Self {
        let conf = confidence_level.clamp(0.90, 0.99999);
        // Inverse normal approximation for upper tail:
        // alpha = 1 - conf -> for 0.9999, alpha = 0.0001 -> z ~ 3.7190
        let z_alpha = if conf >= 0.9999 {
            3.7190
        } else if conf >= 0.999 {
            3.0902
        } else if conf >= 0.99 {
            2.3263
        } else {
            1.6449
        };

        let tau = transfer_latency_hours.max(1e-4);
        let vol_usd = account.total_equity_usd.max(0.0) * account.hourly_equity_volatility.max(0.0);
        let diffusion_shock = z_alpha * vol_usd * tau.sqrt();
        let barrier_usd = diffusion_shock + transfer_fees_usd.max(0.0);

        Self {
            confidence_level: conf,
            z_alpha,
            barrier_usd,
            transfer_fees_usd,
            transfer_latency_hours: tau,
        }
    }
}

/// Analytical water-filling transfer between two segregated margin venues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterFillingTransferVector {
    pub from_venue: String,
    pub to_venue: String,
    /// Analytical water-filling rebalance transfer amount in USD:
    /// $$\Delta C_{i \to j}^* = \frac{E_i \text{MMR}_j - E_j \text{MMR}_i}{\text{MMR}_i + \text{MMR}_j}$$
    pub transfer_amount_usd: f64,
    pub post_transfer_utilization_ratio: f64,
    pub source_pre_utilization: f64,
    pub target_pre_utilization: f64,
}

/// GPD-Clayton tail copula margin haircut multiplier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpdClaytonTailHaircut {
    /// Tail copula haircut multiplier:
    /// $$h_m^* = \left(\frac{\text{ES}_{0.999}}{\text{VaR}_{0.99}}\right) \left(1 + \lambda_L \sqrt{\frac{\tau_{\text{transfer}}}{\tau_{\text{CEX}}}}\right)$$
    pub tail_haircut_multiplier: f64,
    pub adjusted_required_margin_usd: f64,
}

impl GpdClaytonTailHaircut {
    pub fn compute(
        account: &VenueAccountState,
        transfer_latency_hours: f64,
        tail_index_xi: f64,
    ) -> Self {
        let xi = tail_index_xi.clamp(0.10, 0.75);
        // ES(0.999) / VaR(0.99) under Generalized Pareto Distribution:
        let es_var_ratio = (1.0 / (1.0 - xi).max(0.10)) * (0.01 / 0.001f64).powf(xi);

        // Latency penalty lambda_L: DEX bridge latency (~15s-15m) vs CEX internal (~1.5s = 0.000417h)
        let tau_cex_hours = 0.000417; // 1.5s
        let lambda_l = if account.is_dex_perp { 0.35 } else { 0.05 };
        let latency_ratio = (transfer_latency_hours.max(tau_cex_hours) / tau_cex_hours).sqrt();
        let multiplier = es_var_ratio * (1.0 + lambda_l * latency_ratio);

        let adjusted_margin = account.maintenance_margin_required_usd * multiplier;

        Self {
            tail_haircut_multiplier: multiplier,
            adjusted_required_margin_usd: adjusted_margin,
        }
    }
}

/// Action to restore cross-venue multi-account margin equilibrium.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CrossVenueRebalanceAction {
    /// All venues have balanced, safe utilization ($U_m < U_{\text{target}}$).
    Safe { max_utilization: f64 },
    /// Imbalance detected and transfer path is safe within first-passage ruin bounds.
    DispatchWaterFillingTransfer {
        transfer: WaterFillingTransferVector,
        first_passage_survival_prob: f64,
        estimated_arrival_hours: f64,
    },
    /// Transfer latency too slow or volatility too severe; emergency symmetric trim required.
    EmergencySymmetricTrim {
        stressed_venue: String,
        unwind_notional_usd: f64,
        reason: String,
    },
}

/// Dynamic Cross-Venue Margin Deleveraging & Water-Filling Controller.
#[derive(Debug, Clone)]
pub struct CrossVenueMarginDeleveragingController {
    /// Target maximum utilization threshold (e.g. 0.65 = 65% utilization).
    pub max_target_utilization: f64,
    /// Critical emergency utilization barrier (e.g. 0.90 = 90% utilization).
    pub emergency_utilization_barrier: f64,
    /// Heavy-tail index $\xi$ for extreme market volatility (default 0.35).
    pub tail_index_xi: f64,
}

impl Default for CrossVenueMarginDeleveragingController {
    fn default() -> Self {
        Self {
            max_target_utilization: 0.65,
            emergency_utilization_barrier: 0.88,
            tail_index_xi: 0.35,
        }
    }
}

impl CrossVenueMarginDeleveragingController {
    pub fn new(max_target_utilization: f64, emergency_utilization_barrier: f64) -> Self {
        Self {
            max_target_utilization: max_target_utilization.clamp(0.30, 0.80),
            emergency_utilization_barrier: emergency_utilization_barrier.clamp(0.80, 0.98),
            tail_index_xi: 0.35,
        }
    }

    /// Evaluates two segregated accounts and derives either an analytical water-filling
    /// transfer vector or an immediate emergency symmetric trim.
    pub fn evaluate_pair_rebalancing(
        &self,
        account_a: &VenueAccountState,
        account_b: &VenueAccountState,
        transfer_latency_hours: f64,
        transfer_fee_usd: f64,
    ) -> CrossVenueRebalanceAction {
        let u_a = account_a.margin_utilization();
        let u_b = account_b.margin_utilization();
        let max_u = u_a.max(u_b);

        // If both accounts are safely below target utilization, no action required
        if max_u < self.max_target_utilization {
            return CrossVenueRebalanceAction::Safe {
                max_utilization: max_u,
            };
        }

        // Identify surplus vs stressed accounts
        let (surplus, stressed, u_surplus, u_stressed) = if u_a < u_b {
            (account_a, account_b, u_a, u_b)
        } else {
            (account_b, account_a, u_b, u_a)
        };

        // Compute analytical water-filling transfer:
        // Delta C = (E_surplus * MMR_stressed - E_stressed * MMR_surplus) / (MMR_surplus + MMR_stressed)
        let mmr_s = surplus.maintenance_margin_required_usd;
        let mmr_d = stressed.maintenance_margin_required_usd;
        let denom_mmr = mmr_s + mmr_d;

        let delta_c = if denom_mmr <= 1e-9 {
            0.0
        } else {
            (surplus.total_equity_usd * mmr_d - stressed.total_equity_usd * mmr_s) / denom_mmr
        };

        // First-passage ruin barrier on the stressed account during transit
        let ruin_barrier = FirstPassageRuinBarrier::compute(
            stressed,
            transfer_latency_hours,
            transfer_fee_usd,
            0.9999, // 99.99% survival requirement
        );

        // Check if stressed headroom is sufficient to survive transfer latency
        let stressed_headroom = stressed.equity_headroom_usd();
        let can_survive_transfer = stressed_headroom > ruin_barrier.barrier_usd
            && u_stressed < self.emergency_utilization_barrier;

        // Check if surplus account has enough free headroom to fund the transfer
        let surplus_headroom = surplus.equity_headroom_usd();
        let transfer_amount = delta_c.clamp(0.0, surplus_headroom * 0.90);

        if can_survive_transfer && transfer_amount > transfer_fee_usd * 2.0 {
            // Post-transfer utilization
            let post_e_stressed = stressed.total_equity_usd + transfer_amount - transfer_fee_usd;
            let post_u = if post_e_stressed > 1e-6 {
                mmr_d / post_e_stressed
            } else {
                1.0
            };

            CrossVenueRebalanceAction::DispatchWaterFillingTransfer {
                transfer: WaterFillingTransferVector {
                    from_venue: surplus.venue_id.clone(),
                    to_venue: stressed.venue_id.clone(),
                    transfer_amount_usd: transfer_amount,
                    post_transfer_utilization_ratio: post_u,
                    source_pre_utilization: u_surplus,
                    target_pre_utilization: u_stressed,
                },
                first_passage_survival_prob: 0.9999,
                estimated_arrival_hours: transfer_latency_hours,
            }
        } else {
            // Emergency symmetric position reduction:
            // Calculate notional reduction required to restore target utilization
            let target_u = self.max_target_utilization;
            let excess_mmr = (mmr_d - stressed.total_equity_usd * target_u).max(0.0);
            let unwind_notional = if mmr_d > 1e-6 {
                stressed.total_notional_usd * (excess_mmr / mmr_d)
            } else {
                stressed.total_notional_usd * 0.50
            };

            CrossVenueRebalanceAction::EmergencySymmetricTrim {
                stressed_venue: stressed.venue_id.clone(),
                unwind_notional_usd: unwind_notional,
                reason: format!(
                    "Headroom ${:.2} insufficient for transfer ruin barrier ${:.2} or utilization {:.1}% exceeded critical barrier",
                    stressed_headroom, ruin_barrier.barrier_usd, u_stressed * 100.0
                ),
            }
        }
    }
}

/// Second-order margin Hessian evaluator across segregated venues:
/// Delta Pi(Delta S) = Delta_net * Delta S + 0.5 * Gamma_margin * (Delta S)^2
/// For linear USDC perps: Gamma_linear = 0.
/// For coin-margined/inverse contracts: Gamma_inv = -2 * Q / S^3 (USD PnL exhibits negative convexity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondOrderMarginHessian {
    /// Shock grid in relative spot changes (e.g. [-0.35, -0.25, -0.15, 0.15, 0.25, 0.35])
    pub spot_shocks: Vec<f64>,
    /// Maintenance margin safety buffer multiplier (e.g. 1.15 = 15% headroom)
    pub margin_buffer_multiplier: f64,
}

impl Default for SecondOrderMarginHessian {
    fn default() -> Self {
        Self {
            spot_shocks: vec![-0.35, -0.25, -0.15, 0.15, 0.25, 0.35],
            margin_buffer_multiplier: 1.15,
        }
    }
}

impl SecondOrderMarginHessian {
    pub fn new(spot_shocks: Vec<f64>, margin_buffer_multiplier: f64) -> Self {
        Self {
            spot_shocks: if spot_shocks.is_empty() {
                vec![-0.35, -0.25, -0.15, 0.15, 0.25, 0.35]
            } else {
                spot_shocks
            },
            margin_buffer_multiplier: margin_buffer_multiplier.max(1.0),
        }
    }

    /// Evaluates worst-case second-order non-linear mark-to-market loss on a venue under the spot shock grid:
    /// Returns (worst_pnl_usd, worst_shock)
    pub fn evaluate_venue_second_order_pnl(
        &self,
        notional_usd: f64,
        spot_price: f64,
        is_long: bool,
        is_inverse: bool,
    ) -> (f64, f64) {
        let mut worst_pnl = f64::INFINITY;
        let mut worst_shock = 0.0;
        let s0 = spot_price.max(1.0);
        let q_sign = if is_long { 1.0 } else { -1.0 };
        let q_usd = notional_usd.abs();

        for &shock in &self.spot_shocks {
            let ds = s0 * shock;
            let pnl = if is_inverse {
                // Inverse contract PnL in USD with second-order Taylor expansion:
                // Q * (ds / s0) - 0.5 * Q * (ds / s0)^2
                q_sign * q_usd * (ds / s0) - 0.5 * q_sign * q_usd * (ds / s0).powi(2)
            } else {
                // Linear contract PnL in USD
                q_sign * q_usd * shock
            };

            if pnl < worst_pnl {
                worst_pnl = pnl;
                worst_shock = shock;
            }
        }

        (worst_pnl, worst_shock)
    }

    /// Validates the segregated venue solvency barrier:
    /// FreeMargin_m + WorstPnL_m >= MMR_m * (1 + Buffer)
    pub fn check_venue_solvency_barrier(
        &self,
        free_margin_usd: f64,
        maintenance_margin_usd: f64,
        notional_usd: f64,
        spot_price: f64,
        is_long: bool,
        is_inverse: bool,
    ) -> Result<(), String> {
        let (worst_pnl, shock) =
            self.evaluate_venue_second_order_pnl(notional_usd, spot_price, is_long, is_inverse);
        let required_equity = maintenance_margin_usd * self.margin_buffer_multiplier;
        let post_shock_free = free_margin_usd + worst_pnl;

        if post_shock_free < required_equity {
            Err(format!(
                "Solvency barrier breached: post-shock margin ${:.2} < required ${:.2} under {:.1}% shock",
                post_shock_free, required_equity, shock * 100.0
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_margin_safe_state() {
        let ctrl = ProportionalDeleveragingController::new(0.35);

        let long = LegMarginState {
            venue_id: "venue_a".into(),
            is_long: true,
            is_spot: false,
            position_qty: 1.0,
            entry_price: 100.0,
            mark_price: 100.0,
            current_equity_usd: 10.0, // 10 USD equity, req is 100 * 0.05 = 5 USD (buffer 100%)
            maintenance_margin_rate: 0.05,
            taker_fee_rate: 0.0005,
        };

        let short = LegMarginState {
            venue_id: "venue_b".into(),
            is_long: false,
            is_spot: false,
            position_qty: 1.0,
            entry_price: 100.0,
            mark_price: 100.0,
            current_equity_usd: 10.0,
            maintenance_margin_rate: 0.05,
            taker_fee_rate: 0.0005,
        };

        let res = ctrl.evaluate_pair_margin(&long, &short);
        match res {
            MarginDeleveragingDecision::Safe {
                long_buffer_ratio,
                short_buffer_ratio,
            } => {
                assert!(long_buffer_ratio >= 0.35);
                assert!(short_buffer_ratio >= 0.35);
            }
            _ => panic!("Expected Safe decision"),
        }
    }

    #[test]
    fn test_proportional_deleveraging_solves_target_buffer() {
        let ctrl = ProportionalDeleveragingController::new(0.35);

        // Price rallied from 100 to 110: short leg equity degraded from 10 to 0.0 - 0.05 = ~5.5
        let long = LegMarginState {
            venue_id: "venue_a".into(),
            is_long: true,
            is_spot: false,
            position_qty: 1.0,
            entry_price: 100.0,
            mark_price: 110.0,
            current_equity_usd: 20.0, // Long gained equity
            maintenance_margin_rate: 0.05,
            taker_fee_rate: 0.0005,
        };

        // Short leg: req MM = 1.0 * 110 * 0.05 = 5.50. Equity is 6.00 (buffer = 6/5.5 - 1 = 9.1% < 35%)
        let short = LegMarginState {
            venue_id: "venue_b".into(),
            is_long: false,
            is_spot: false,
            position_qty: 1.0,
            entry_price: 100.0,
            mark_price: 110.0,
            current_equity_usd: 6.00,
            maintenance_margin_rate: 0.05,
            taker_fee_rate: 0.0005,
        };

        let res = ctrl.evaluate_pair_margin(&long, &short);
        match res {
            MarginDeleveragingDecision::SymmetricPartialUnwind {
                stressed_venue,
                unwind_qty,
                remaining_qty,
                stressed_buffer_ratio,
                ..
            } => {
                assert_eq!(stressed_venue, "venue_b");
                assert!(unwind_qty > 0.0 && unwind_qty < 1.0);
                assert!(stressed_buffer_ratio < 0.35);

                // Verify that post-unwind, equity / MM satisfies target buffer!
                let post_q = remaining_qty;
                let post_fee = unwind_qty * 110.0 * 0.0005;
                let post_equity = 6.00 - post_fee;
                let post_mm = post_q * 110.0 * 0.05;
                let post_buffer = post_equity / post_mm - 1.0;
                assert!(
                    (post_buffer - 0.35).abs() < 1e-3,
                    "Post buffer was {:.4}, expected ~0.35",
                    post_buffer
                );
            }
            _ => panic!("Expected SymmetricPartialUnwind"),
        }
    }

    #[test]
    fn test_spot_leg_margin_safety() {
        let spot_long = LegMarginState {
            venue_id: "alpaca".into(),
            is_long: true,
            is_spot: true,
            position_qty: 2.0,
            entry_price: 150.0,
            mark_price: 155.0,
            current_equity_usd: 310.0,
            maintenance_margin_rate: 0.05,
            taker_fee_rate: 0.0025,
        };

        assert_eq!(spot_long.maintenance_margin_usd(), 0.0);
        assert_eq!(spot_long.liquidation_price(), 0.0);
        assert_eq!(spot_long.margin_buffer_ratio(), 100.0);
    }

    #[test]
    fn test_liquidation_risk_evaluator() {
        // Price = 100, Liq = 80 (20% drop), 60% annualized vol, 14-day hold (14 / 365 years)
        let prob =
            LiquidationRiskEvaluator::prob_liquidation_touch(100.0, 80.0, 0.60, 14.0 / 365.0);
        assert!(prob > 0.0 && prob < 1.0);

        // Immediate touch if liquidation price is identical to current price
        let prob_touch =
            LiquidationRiskEvaluator::prob_liquidation_touch(100.0, 100.0, 0.60, 14.0 / 365.0);
        assert!((prob_touch - 1.0).abs() < 1e-4);

        // Dynamic reserve equity
        let reserve = LiquidationRiskEvaluator::dynamic_reserve_equity_usd(
            1000.0, // 1000 USD notional
            0.05,   // 5% MMR
            0.005,  // 0.5% hourly vol
            2.0,    // 2-hour rebalance window
            3.5,    // 3.5 sigmas
        );
        assert!(reserve > 50.0); // Must exceed base MMR of 50 USD
    }

    #[test]
    fn test_evt_heavy_tail_liquidation_and_reserve() {
        let p_curr = 100.0;
        let p_liq = 85.0; // 15% distance
        let vol = 0.80; // 80% annual vol
        let horizon = 7.0 / 365.0; // 7-day hold
        let xi = 0.36; // Crypto heavy-tail index

        let p_gauss = LiquidationRiskEvaluator::prob_liquidation_touch(p_curr, p_liq, vol, horizon);
        let p_evt =
            LiquidationRiskEvaluator::prob_liquidation_touch_evt(p_curr, p_liq, vol, horizon, xi);

        // Fat tails must strictly elevate liquidation probability above Gaussian
        assert!(
            p_evt > p_gauss,
            "EVT probability {p_evt} must exceed Gaussian {p_gauss}"
        );
        assert!(p_evt <= 1.0);

        // EVT reserve equity under fat tails vs Gaussian
        let reserve_gauss =
            LiquidationRiskEvaluator::dynamic_reserve_equity_usd(10_000.0, 0.05, 0.008, 1.0, 3.37);
        let reserve_evt = LiquidationRiskEvaluator::evt_heavy_tail_reserve_equity_usd(
            10_000.0, 0.05, 0.008, 1.0, xi,
        );

        assert!(
            reserve_evt > reserve_gauss,
            "EVT reserve {reserve_evt} must exceed Gaussian {reserve_gauss}"
        );
        assert!(reserve_evt > 500.0); // MMR = $500
    }

    #[test]
    fn test_first_passage_ruin_barrier() {
        let account = VenueAccountState {
            venue_id: "hyperliquid".into(),
            total_equity_usd: 10_000.0,
            maintenance_margin_required_usd: 1_000.0,
            initial_margin_required_usd: 2_000.0,
            total_notional_usd: 20_000.0,
            hourly_equity_volatility: 0.015, // 1.5% hourly vol
            is_dex_perp: true,
        };

        // 15-minute bridge transfer (0.25 hours), $5 fee, 99.99% survival
        let barrier = FirstPassageRuinBarrier::compute(&account, 0.25, 5.0, 0.9999);
        assert_eq!(barrier.z_alpha, 3.7190);
        // vol_usd = 10000 * 0.015 = 150. diffusion = 3.719 * 150 * sqrt(0.25) = 3.719 * 75 = 278.925
        // total barrier = 278.925 + 5.0 = 283.925 USD
        assert!((barrier.barrier_usd - 283.925).abs() < 1.0);
        assert!(account.equity_headroom_usd() > barrier.barrier_usd);
    }

    #[test]
    fn test_water_filling_transfer_equalizes_utilization() {
        let account_a = VenueAccountState {
            venue_id: "binance".into(),
            total_equity_usd: 20_000.0,
            maintenance_margin_required_usd: 2_000.0, // U = 10%
            initial_margin_required_usd: 4_000.0,
            total_notional_usd: 40_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: false,
        };

        let account_b = VenueAccountState {
            venue_id: "hyperliquid".into(),
            total_equity_usd: 3_000.0,
            maintenance_margin_required_usd: 2_400.0, // U = 80% (stressed)
            initial_margin_required_usd: 4_800.0,
            total_notional_usd: 48_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: true,
        };

        let ctrl = CrossVenueMarginDeleveragingController::new(0.65, 0.90);
        let action = ctrl.evaluate_pair_rebalancing(
            &account_a, &account_b, 0.005, // ~18 seconds transfer latency
            2.0,   // $2 fee
        );

        match action {
            CrossVenueRebalanceAction::DispatchWaterFillingTransfer {
                transfer,
                first_passage_survival_prob,
                ..
            } => {
                assert_eq!(transfer.from_venue, "binance");
                assert_eq!(transfer.to_venue, "hyperliquid");
                assert!(transfer.transfer_amount_usd > 5000.0);
                assert_eq!(first_passage_survival_prob, 0.9999);
                // Post transfer utilization on stressed account should drop to ~19-21%
                assert!(transfer.post_transfer_utilization_ratio < 0.35);
            }
            _ => panic!("Expected DispatchWaterFillingTransfer"),
        }
    }

    #[test]
    fn test_gpd_clayton_tail_haircut() {
        let account_cex = VenueAccountState {
            venue_id: "binance".into(),
            total_equity_usd: 10_000.0,
            maintenance_margin_required_usd: 1_000.0,
            initial_margin_required_usd: 2_000.0,
            total_notional_usd: 20_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: false,
        };

        let account_dex = VenueAccountState {
            venue_id: "lighter".into(),
            total_equity_usd: 10_000.0,
            maintenance_margin_required_usd: 1_000.0,
            initial_margin_required_usd: 2_000.0,
            total_notional_usd: 20_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: true,
        };

        // 5-minute transfer horizon (0.0833h)
        let haircut_cex = GpdClaytonTailHaircut::compute(&account_cex, 0.0833, 0.35);
        let haircut_dex = GpdClaytonTailHaircut::compute(&account_dex, 0.0833, 0.35);

        // DEX haircut must strictly exceed CEX due to bridge latency penalty lambda_L
        assert!(haircut_dex.tail_haircut_multiplier > haircut_cex.tail_haircut_multiplier);
        assert!(haircut_dex.adjusted_required_margin_usd > 1_000.0);
    }
}
