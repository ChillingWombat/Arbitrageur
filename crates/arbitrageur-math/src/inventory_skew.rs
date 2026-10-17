//! Dynamic inventory-skew reservation spread adjustment based on
//! Avellaneda-Stoikov (2008) and Guéant-Tapia-Manziadi (2012).
//!
//! In cross-venue spatial arbitrage and pairs trading, repeated directional
//! entries cause one-sided inventory accumulation across venues. While the net
//! dollar delta may be nominally hedged, asymmetric collateral exhaustion and
//! rebalancing costs require dynamic reservation hurdles:
//! - Trades that worsen the inventory skew face an elevated profit hurdle.
//! - Trades that rebalance inventory back to neutral receive a discount down
//!   to the round-trip friction floor.

/// Configuration parameters for inventory-skew reservation adjustments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InventorySkewConfig {
    /// Inventory risk aversion parameter γ > 0 (default: 0.5).
    pub risk_aversion_gamma: f64,
    /// Non-linear barrier penalty scaling factor κ (default: 2.0).
    pub barrier_kappa: f64,
    /// Maximum allowed absolute inventory capacity in USD (default: 10,000).
    pub max_inventory_usd: f64,
    /// Hard floor on the discounted hurdle to ensure fees are covered (bps, default: 2.0).
    pub min_friction_bps: f64,
}

impl Default for InventorySkewConfig {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 0.5,
            barrier_kappa: 2.0,
            max_inventory_usd: 10_000.0,
            min_friction_bps: 2.0,
        }
    }
}

/// Computes the dynamic reservation profit hurdle (in basis points) for a candidate trade.
///
/// # Arguments
/// - `base_target_bps`: The unadjusted target profit hurdle (e.g. 5.0 bps).
/// - `current_inventory_usd`: Current net directional inventory on the asset/pair in USD.
///   Positive indicates long-skew; negative indicates short-skew.
/// - `trade_size_usd`: Notional size of the candidate trade in USD (> 0).
/// - `spread_std_bps`: Standard deviation of the spread in basis points (> 0).
/// - `half_life_ratio`: Normalized OU mean-reversion half-life factor τ ∈ (0, 10] (default: 1.0).
/// - `is_increasing_skew`: `true` if the candidate trade further expands current inventory skew;
///   `false` if the trade moves inventory toward neutral (rebalancing).
/// - `cfg`: The inventory skew configuration.
pub fn reservation_hurdle_bps(
    base_target_bps: f64,
    current_inventory_usd: f64,
    trade_size_usd: f64,
    spread_std_bps: f64,
    half_life_ratio: f64,
    is_increasing_skew: bool,
    cfg: &InventorySkewConfig,
) -> f64 {
    if !base_target_bps.is_finite() || base_target_bps <= 0.0 {
        return cfg.min_friction_bps;
    }
    if !current_inventory_usd.is_finite()
        || !trade_size_usd.is_finite()
        || trade_size_usd <= 0.0
        || !spread_std_bps.is_finite()
        || spread_std_bps <= 0.0
        || cfg.max_inventory_usd <= 0.0
    {
        return base_target_bps;
    }

    let q_norm = (current_inventory_usd.abs() / cfg.max_inventory_usd).clamp(0.0, 0.999);
    let dq_norm = (trade_size_usd / cfg.max_inventory_usd).clamp(0.0, 1.0);
    let tau = half_life_ratio.clamp(0.1, 10.0);

    if is_increasing_skew {
        // Guéant-Tapia-Manziadi marginal reservation spread:
        // Δδ = γ · σ · τ · (q + 0.5 Δq)
        let linear_penalty =
            cfg.risk_aversion_gamma * spread_std_bps * tau * (q_norm + 0.5 * dq_norm);

        // Non-linear barrier function as inventory approaches capacity:
        // Ψ(q) = κ · (q / (1 - q))^2
        let barrier_penalty = if q_norm > 0.0 {
            let ratio = q_norm / (1.0 - q_norm);
            cfg.barrier_kappa * ratio * ratio
        } else {
            0.0
        };

        base_target_bps + linear_penalty + barrier_penalty
    } else {
        // Rebalancing trade reduces inventory skew:
        // Discount the hurdle to encourage natural inventory unwind:
        // H_discounted = max(min_friction, base - γ · σ · τ · q)
        let discount = cfg.risk_aversion_gamma * spread_std_bps * tau * q_norm;
        (base_target_bps - discount).max(cfg.min_friction_bps)
    }
}

/// Computes the dynamic reservation profit hurdle taking into account both
/// portfolio inventory skew AND per-venue margin utilization.
///
/// In cross-venue arbitrage, exchanges are segregated margin pools. If any
/// venue's margin utilization approaches warning/critical thresholds (> 70%),
/// entering a new position that demands collateral on that venue incurs an
/// exponential penalty. Conversely, trades that reduce exposure or rebalance
/// away from the stressed venue receive relief.
#[allow(clippy::too_many_arguments)]
pub fn venue_aware_reservation_hurdle_bps(
    base_target_bps: f64,
    current_inventory_usd: f64,
    trade_size_usd: f64,
    spread_std_bps: f64,
    half_life_ratio: f64,
    is_increasing_skew: bool,
    max_venue_margin_usage_pct: f64,
    cfg: &InventorySkewConfig,
) -> f64 {
    let base_hurdle = reservation_hurdle_bps(
        base_target_bps,
        current_inventory_usd,
        trade_size_usd,
        spread_std_bps,
        half_life_ratio,
        is_increasing_skew,
        cfg,
    );

    if !max_venue_margin_usage_pct.is_finite() || max_venue_margin_usage_pct <= 0.0 {
        return base_hurdle;
    }

    // Normal margin usage <= 70% incurs 0 penalty.
    // Beyond 70%, penalty scales quadratically:
    // penalty = kappa_margin * ((usage - 70) / (100 - usage))^2
    let usage = max_venue_margin_usage_pct.clamp(0.0, 99.5);
    if is_increasing_skew && usage > 70.0 {
        let excess_ratio = (usage - 70.0) / (100.0 - usage);
        let margin_penalty = cfg.barrier_kappa * excess_ratio * excess_ratio * 10.0;
        base_hurdle + margin_penalty
    } else {
        base_hurdle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_inventory_returns_base_target() {
        let cfg = InventorySkewConfig::default();
        let hurdle_inc = reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, true, &cfg);
        // At q=0, marginal dq_norm = 500/10000 = 0.05 -> penalty = 0.5 * 10 * 1 * (0 + 0.025) = 0.125
        assert!((hurdle_inc - 5.125).abs() < 1e-4);

        let hurdle_dec = reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, false, &cfg);
        assert!((hurdle_dec - 5.0).abs() < 1e-4);
    }

    #[test]
    fn adverse_inventory_increases_hurdle() {
        let cfg = InventorySkewConfig::default();
        // 50% inventory skew
        let hurdle_50 = reservation_hurdle_bps(5.0, 5000.0, 500.0, 10.0, 1.0, true, &cfg);
        assert!(hurdle_50 > 5.0);

        // 80% inventory skew
        let hurdle_80 = reservation_hurdle_bps(5.0, 8000.0, 500.0, 10.0, 1.0, true, &cfg);
        assert!(hurdle_80 > hurdle_50);
        // Non-linear barrier penalty significantly boosts hurdle
        assert!(hurdle_80 >= 35.0);
    }

    #[test]
    fn rebalancing_trade_discounts_hurdle_down_to_friction() {
        let cfg = InventorySkewConfig::default();
        // At 50% skew, rebalancing discounts hurdle
        let hurdle_rebal = reservation_hurdle_bps(5.0, 5000.0, 500.0, 10.0, 1.0, false, &cfg);
        assert!(hurdle_rebal < 5.0);
        assert!(hurdle_rebal >= cfg.min_friction_bps);

        // Severe skew discounts to floor
        let hurdle_extreme = reservation_hurdle_bps(5.0, 9500.0, 500.0, 10.0, 1.0, false, &cfg);
        assert_eq!(hurdle_extreme, cfg.min_friction_bps);
    }

    #[test]
    fn venue_aware_margin_usage_penalty() {
        let cfg = InventorySkewConfig::default();
        // Under normal margin usage (<= 70%), venue-aware hurdle equals standard hurdle
        let normal =
            venue_aware_reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, true, 50.0, &cfg);
        let standard = reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, true, &cfg);
        assert!((normal - standard).abs() < 1e-4);

        // Under high margin usage (> 70%), penalty increases hurdle
        let elevated =
            venue_aware_reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, true, 85.0, &cfg);
        assert!(elevated > normal);

        // Rebalancing trade does not suffer the margin penalty
        let rebal =
            venue_aware_reservation_hurdle_bps(5.0, 0.0, 500.0, 10.0, 1.0, false, 85.0, &cfg);
        assert!((rebal - 5.0).abs() < 1e-4);
    }

    #[test]
    fn degenerate_inputs_fail_safe() {
        let cfg = InventorySkewConfig::default();
        assert_eq!(
            reservation_hurdle_bps(0.0, 100.0, 50.0, 10.0, 1.0, true, &cfg),
            cfg.min_friction_bps
        );
        assert_eq!(
            reservation_hurdle_bps(5.0, f64::NAN, 50.0, 10.0, 1.0, true, &cfg),
            5.0
        );
        assert_eq!(
            venue_aware_reservation_hurdle_bps(
                5.0,
                f64::NAN,
                50.0,
                10.0,
                1.0,
                true,
                f64::NAN,
                &cfg
            ),
            5.0
        );
    }
}
