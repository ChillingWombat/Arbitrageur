use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A candidate opportunity evaluated for execution across venues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateOpportunity {
    pub id: String,
    pub symbol: String,
    pub venue_a: String,
    pub venue_b: String,
    pub expected_net_bps: f64,
    pub order_qty: f64,
    pub est_profit_usd: f64,
}

/// Greedy venue matching with mutual exclusion (Requirement 10).
/// For a given ticker, orders opportunities by highest expected net BPS profit.
/// Selects the highest profit pair first, marks both participating venues as busy,
/// and greedily selects any remaining disjoint venue pairs that also have valid opportunities.
/// Guarantees that in a single execution cycle, a venue is paired with at most one other venue for that ticker.
pub fn select_disjoint_greedy_opportunities(
    mut opps: Vec<CandidateOpportunity>,
) -> Vec<CandidateOpportunity> {
    opps.sort_by(|a, b| {
        b.expected_net_bps
            .partial_cmp(&a.expected_net_bps)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut selected = Vec::new();
    let mut used_venues = HashSet::new();

    for opp in opps {
        if !used_venues.contains(&opp.venue_a) && !used_venues.contains(&opp.venue_b) {
            used_venues.insert(opp.venue_a.clone());
            used_venues.insert(opp.venue_b.clone());
            selected.push(opp);
        }
    }

    selected
}

/// Details of a slippage violation when book depth is insufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlippageViolation {
    pub top_price: Decimal,
    pub effective_vwap: Decimal,
    pub slippage_bps: Decimal,
    pub max_allowed_bps: Decimal,
}

/// Risk-control rejection raised by synchronous safety guards.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RiskError {
    #[error(
        "directional divergence stop: entry {entry_spread_bps} bps, current {current_spread_bps} bps, max excursion {max_adverse_excursion_bps} bps"
    )]
    DirectionalDivergenceStop {
        entry_spread_bps: Decimal,
        current_spread_bps: Decimal,
        max_adverse_excursion_bps: Decimal,
    },
}

/// Decimal-native catastrophic stop for divergence trades.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionalDivergenceStopGuard {
    pub max_adverse_excursion_bps: Decimal,
}

impl DirectionalDivergenceStopGuard {
    pub fn new(max_adverse_excursion_bps: Decimal) -> Self {
        Self {
            max_adverse_excursion_bps: max_adverse_excursion_bps.max(Decimal::ZERO),
        }
    }

    pub fn check_directional_divergence_stop(
        &self,
        entry_spread_bps: Decimal,
        current_spread_bps: Decimal,
    ) -> Result<(), RiskError> {
        let entry_abs = entry_spread_bps.abs();
        let current_abs = current_spread_bps.abs();
        let limit = self.max_adverse_excursion_bps;
        let adverse_widening = current_abs > entry_abs + limit;
        let sign_flip = entry_spread_bps != Decimal::ZERO
            && current_spread_bps != Decimal::ZERO
            && entry_spread_bps.is_sign_negative() != current_spread_bps.is_sign_negative();

        if adverse_widening || (sign_flip && current_abs > limit) {
            Err(RiskError::DirectionalDivergenceStop {
                entry_spread_bps,
                current_spread_bps,
                max_adverse_excursion_bps: limit,
            })
        } else {
            Ok(())
        }
    }
}

/// Verifies whether the order can be filled within the allowed maximum slippage (Requirement 11).
/// Traverses actual book levels to simulate fill price before dispatching atomic legs.
pub fn verify_pre_execution_slippage(
    book: &OrderBook,
    side: Side,
    requested_qty: Decimal,
    max_slippage_bps: Decimal,
) -> Result<(), SlippageViolation> {
    let top_price = match side {
        Side::Buy => book.best_ask_price().unwrap_or(dec!(0)),
        Side::Sell => book.best_bid_price().unwrap_or(dec!(0)),
    };

    if top_price <= dec!(0) {
        return Err(SlippageViolation {
            top_price,
            effective_vwap: dec!(0),
            slippage_bps: dec!(9999),
            max_allowed_bps: max_slippage_bps,
        });
    }

    // Insufficient depth must fail: never substitute the top price for a VWAP
    // that could not fill the full requested quantity.
    let exec = match side {
        Side::Buy => book.buy_execution_vwap(requested_qty),
        Side::Sell => book.sell_execution_vwap(requested_qty),
    };
    let effective_vwap = match exec {
        Some((vwap, remaining, _)) if remaining <= dec!(0) => vwap,
        Some((vwap, _, _)) => {
            return Err(SlippageViolation {
                top_price,
                effective_vwap: vwap,
                slippage_bps: dec!(9999),
                max_allowed_bps: max_slippage_bps,
            });
        }
        None => {
            return Err(SlippageViolation {
                top_price,
                effective_vwap: dec!(0),
                slippage_bps: dec!(9999),
                max_allowed_bps: max_slippage_bps,
            });
        }
    };

    let slip_ratio = match side {
        Side::Buy => {
            if effective_vwap >= top_price {
                (effective_vwap - top_price) / top_price
            } else {
                dec!(0)
            }
        }
        Side::Sell => {
            if top_price >= effective_vwap {
                (top_price - effective_vwap) / top_price
            } else {
                dec!(0)
            }
        }
    };

    let slippage_bps = slip_ratio * dec!(10000);
    if slippage_bps > max_slippage_bps {
        Err(SlippageViolation {
            top_price,
            effective_vwap,
            slippage_bps,
            max_allowed_bps: max_slippage_bps,
        })
    } else {
        Ok(())
    }
}

/// Checks whether an open arbitrage position has exceeded its maximum holding duration (Requirement 11).
/// Prevents capital from being tied up in stagnant positions where convergence has stalled.
pub fn check_time_decay_exit(
    opened_at: DateTime<Utc>,
    current_time: DateTime<Utc>,
    max_duration_secs: u64,
) -> bool {
    let elapsed = current_time.signed_duration_since(opened_at).num_seconds();
    elapsed >= max_duration_secs as i64
}

/// Evaluates if a divergence has exploded catastrophically against our position (Requirement 11).
/// For example, if a de-peg or structural dislocation occurs, stops scaling in and signals emergency unwind.
pub fn check_divergence_catastrophic_stop(
    entry_spread_bps: f64,
    current_spread_bps: f64,
    _baseline_mean_bps: f64,
    max_adverse_divergence_bps: f64,
) -> bool {
    let entry_abs = entry_spread_bps.abs();
    let current_abs = current_spread_bps.abs();
    let adverse_widening = current_abs > entry_abs + max_adverse_divergence_bps;
    let sign_flip = entry_spread_bps != 0.0
        && current_spread_bps != 0.0
        && entry_spread_bps.is_sign_negative() != current_spread_bps.is_sign_negative();

    adverse_widening || (sign_flip && current_abs > max_adverse_divergence_bps)
}

/// Recommendation for manual balance rebalancing across exchanges (Requirement 9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebalanceRecommendation {
    pub low_venue: String,
    pub current_balance_usd: f64,
    pub allocation_pct: f64,
    pub high_venue: String,
    pub high_venue_balance_usd: f64,
    pub suggested_transfer_usd: f64,
    pub warning_message: String,
}

/// Checks venue balances for asymmetric depletion caused by sequential one-sided arbitrage fills (Requirement 9).
/// Generates actionable rebalancing alerts when any active venue drops below minimum safety thresholds.
pub fn evaluate_venue_rebalance_needs(
    balances: &HashMap<String, Decimal>,
    min_allocation_threshold_pct: Decimal,
) -> Vec<RebalanceRecommendation> {
    let mut recommendations = Vec::new();
    let total_balance: Decimal = balances.values().copied().sum();

    if total_balance <= dec!(0) || balances.len() < 2 {
        return recommendations;
    }

    // Find highest balance venue
    let (highest_venue, highest_bal) = balances
        .iter()
        .max_by(|a, b| a.1.cmp(b.1))
        .map(|(k, v)| (k.clone(), *v))
        .unwrap_or_else(|| ("".into(), dec!(0)));

    for (venue, &bal) in balances {
        let allocation_pct = (bal / total_balance) * dec!(100);
        if allocation_pct < min_allocation_threshold_pct {
            let target_bal = total_balance / Decimal::from(balances.len());
            let deficit = if target_bal > bal {
                target_bal - bal
            } else {
                dec!(0)
            };

            let bal_f = bal.to_string().parse::<f64>().unwrap_or(0.0);
            let alloc_f = allocation_pct.to_string().parse::<f64>().unwrap_or(0.0);
            let high_f = highest_bal.to_string().parse::<f64>().unwrap_or(0.0);
            let def_f = deficit.to_string().parse::<f64>().unwrap_or(0.0);

            // Suppress uneconomic micro-rebalances where fixed cross-chain gas/bridge fees (> $15) exceed 5% of transfer
            let estimated_bridge_friction_usd = 15.0;
            if def_f < estimated_bridge_friction_usd * 20.0 {
                continue; // Deficit < $300 is uneconomic to bridge across L1/L2
            }

            recommendations.push(RebalanceRecommendation {
                low_venue: venue.clone(),
                current_balance_usd: bal_f,
                allocation_pct: alloc_f,
                high_venue: highest_venue.clone(),
                high_venue_balance_usd: high_f,
                suggested_transfer_usd: def_f,
                warning_message: format!(
                    "Asymmetric Margin Depletion: {} balance is ${:.2} ({:.1}% of total). One-sided fills have accumulated. Manually rebalance ~${:.2} from {} to maintain delta-neutral execution.",
                    venue, bal_f, alloc_f, def_f, highest_venue
                ),
            });
        }
    }

    recommendations
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::quote::BookLevel;

    #[test]
    fn test_greedy_venue_matching_mutual_exclusion() {
        // 4 venues: HL, Lighter, Vanta, Robinhood
        let opps = vec![
            CandidateOpportunity {
                id: "opp-1".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Lighter".into(),
                expected_net_bps: 22.0, // Top profit
                order_qty: 0.1,
                est_profit_usd: 45.0,
            },
            CandidateOpportunity {
                id: "opp-2".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Hyperliquid".into(),
                venue_b: "Vanta".into(),
                expected_net_bps: 18.0, // Uses Hyperliquid again -> should be skipped!
                order_qty: 0.1,
                est_profit_usd: 35.0,
            },
            CandidateOpportunity {
                id: "opp-3".into(),
                symbol: "BTC-USDC".into(),
                venue_a: "Vanta".into(),
                venue_b: "Robinhood".into(),
                expected_net_bps: 15.0, // Disjoint from opp-1 (uses Vanta and Robinhood) -> should be selected!
                order_qty: 0.1,
                est_profit_usd: 25.0,
            },
        ];

        let selected = select_disjoint_greedy_opportunities(opps);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "opp-1");
        assert_eq!(selected[1].id, "opp-3");

        // Verify mutual exclusion: each venue appears at most once
        let mut venues = HashSet::new();
        for opp in &selected {
            assert!(venues.insert(opp.venue_a.clone()));
            assert!(venues.insert(opp.venue_b.clone()));
        }
    }

    #[test]
    fn test_pre_execution_slippage_verification() {
        use arbitrageur_core::types::VenueId;

        let mut book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        // Ask levels: 1.0 @ $64,200, then thin book 1.0 @ $64,350 (+23 BPS)
        book.asks.push(BookLevel::new(dec!(64200.0), dec!(1.0)));
        book.asks.push(BookLevel::new(dec!(64350.0), dec!(1.0)));

        // 0.5 qty fits entirely in first level -> 0 slippage
        let res_small = verify_pre_execution_slippage(&book, Side::Buy, dec!(0.5), dec!(5.0));
        assert!(res_small.is_ok());

        // 1.8 qty eats into second level -> slippage exceeds 5 BPS limit
        let res_large = verify_pre_execution_slippage(&book, Side::Buy, dec!(1.8), dec!(5.0));
        assert!(res_large.is_err());
        let violation = res_large.unwrap_err();
        assert!(violation.slippage_bps > dec!(5.0));
    }

    #[test]
    fn test_pre_execution_slippage_insufficient_depth() {
        use arbitrageur_core::types::VenueId;

        let mut book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        book.bids.push(BookLevel::new(dec!(64000.0), dec!(1.0)));
        book.asks.push(BookLevel::new(dec!(64200.0), dec!(1.0)));

        // Total ask depth 1.0 cannot fill 2.0 -> violation even with a huge limit
        let res = verify_pre_execution_slippage(&book, Side::Buy, dec!(2.0), dec!(100000.0));
        let violation = res.expect_err("insufficient depth must violate");
        assert_eq!(violation.slippage_bps, dec!(9999));
        assert_eq!(violation.effective_vwap, dec!(64200.0));

        let res = verify_pre_execution_slippage(&book, Side::Sell, dec!(2.0), dec!(100000.0));
        assert!(res.is_err());
    }

    #[test]
    fn test_time_decay_and_catastrophic_stop() {
        let t0 = Utc::now() - chrono::Duration::seconds(7200); // 2 hours ago
        let now = Utc::now();
        assert!(check_time_decay_exit(t0, now, 3600)); // Past 1h limit
        assert!(!check_time_decay_exit(now, now, 3600));

        // Catastrophic divergence stop:
        // Entry at 15 BPS, expected convergence to baseline 10 BPS.
        // But spread blew out to 65 BPS (adverse +50 BPS):
        assert!(check_divergence_catastrophic_stop(15.0, 68.0, 10.0, 45.0));
        // Normal small noise (15 -> 18 BPS) should not trigger stop:
        assert!(!check_divergence_catastrophic_stop(15.0, 18.0, 10.0, 45.0));
        // Convergence from a wide entry is favourable, not catastrophic:
        assert!(!check_divergence_catastrophic_stop(100.0, 20.0, 10.0, 45.0));
    }

    #[test]
    fn test_directional_divergence_stop_ignores_convergence_and_flags_flips() {
        let guard = DirectionalDivergenceStopGuard::new(dec!(45.0));

        assert!(guard
            .check_directional_divergence_stop(dec!(15.0), dec!(68.0))
            .is_err());
        assert!(guard
            .check_directional_divergence_stop(dec!(100.0), dec!(20.0))
            .is_ok());
        assert!(guard
            .check_directional_divergence_stop(dec!(100.0), dec!(-60.0))
            .is_err());
    }

    #[test]
    fn test_asymmetric_venue_rebalance_needs() {
        let mut balances = HashMap::new();
        balances.insert("Hyperliquid".into(), dec!(8500.0));
        balances.insert("Lighter".into(), dec!(500.0)); // 500 / 9000 = 5.5% (below 15% threshold)

        let recs = evaluate_venue_rebalance_needs(&balances, dec!(15.0));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].low_venue, "Lighter");
        assert_eq!(recs[0].high_venue, "Hyperliquid");
        assert!(recs[0].suggested_transfer_usd > 3000.0);
        assert!(recs[0]
            .warning_message
            .contains("Asymmetric Margin Depletion"));
    }

    #[test]
    fn test_asymmetric_venue_rebalance_suppresses_uneconomic_micro_transfers() {
        let mut balances = HashMap::new();
        // Total balance = $1000. Lighter has $100 (10% < 15%), target = $500, deficit = $400.
        // If total balance is $400, Lighter has $30 (7.5% < 15%), target = $200, deficit = $170 (< $300 minimum economic transfer)
        balances.insert("Hyperliquid".into(), dec!(370.0));
        balances.insert("Lighter".into(), dec!(30.0));

        let recs = evaluate_venue_rebalance_needs(&balances, dec!(15.0));
        // Deficit is $170, which is less than $300 (15 * 20), so transfer is uneconomic and suppressed
        assert_eq!(recs.len(), 0);
    }
}
