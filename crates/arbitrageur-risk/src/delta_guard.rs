use arbitrageur_core::position::Position;
use arbitrageur_core::types::{dec, Decimal, Side};
use std::collections::HashMap;
use tracing::warn;

/// Portfolio delta neutrality tracker.
#[derive(Debug, Clone)]
pub struct DeltaGuard {
    /// Maximum allowed unhedged notional in USD per base asset
    max_unhedged_usd_per_asset: Decimal,
}

impl DeltaGuard {
    pub fn new(max_unhedged_usd_per_asset: Decimal) -> Self {
        Self {
            max_unhedged_usd_per_asset,
        }
    }

    /// Get current max unhedged USD exposure limit
    pub fn max_unhedged_usd_per_asset(&self) -> Decimal {
        self.max_unhedged_usd_per_asset
    }

    /// Set max unhedged USD exposure limit
    pub fn set_max_unhedged_usd_per_asset(&mut self, limit: Decimal) {
        self.max_unhedged_usd_per_asset = limit;
    }

    /// Aggregates positions across all venues by base currency and returns net delta and USD exposure.
    /// Signed base quantities are netted first, then the residual quantity is
    /// valued at the maximum positive mark observed for that base, so equal
    /// long/short quantities remain delta-neutral even when venue marks differ.
    ///
    /// Option legs are parity-netted per (expiry, strike): put-call parity
    /// makes `Δ_call − Δ_put = 1` identically, so an opposite-sign C/P pair
    /// contributes exactly ±1 per unit of base regardless of greeks — a
    /// conversion (short C + long P) is exactly −1, matching the +1 perp leg
    /// it hedges. Unmatched option residuals count ±1 by type (call +1 /
    /// put −1) — the deep-ITM bound, conservative for guard purposes.
    pub fn calculate_net_delta(
        &self,
        positions: &[Position],
    ) -> HashMap<String, (Decimal, Decimal)> {
        let mut delta_map: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        let mut max_mark: HashMap<String, Decimal> = HashMap::new();
        // (base, expiry, strike) -> (net signed call qty, net signed put qty)
        let mut opt_pairs: HashMap<(String, String, String), (Decimal, Decimal)> = HashMap::new();

        for pos in positions {
            let base = pos
                .symbol
                .split('-')
                .next()
                .unwrap_or(&pos.symbol)
                .to_uppercase();
            delta_map.entry(base.clone()).or_insert((dec!(0), dec!(0)));
            if pos.mark_price > dec!(0) {
                let m = max_mark.entry(base.clone()).or_insert(dec!(0));
                if pos.mark_price > *m {
                    *m = pos.mark_price;
                }
            }
            if let Some((expiry, strike, is_call)) = parse_option_key(&pos.symbol) {
                let e = opt_pairs
                    .entry((base, expiry, strike))
                    .or_insert((dec!(0), dec!(0)));
                if is_call {
                    e.0 += pos.signed_quantity();
                } else {
                    e.1 += pos.signed_quantity();
                }
            } else {
                delta_map.entry(base).or_insert((dec!(0), dec!(0))).0 += pos.signed_quantity();
            }
        }

        for ((base, _, _), (calls, puts)) in opt_pairs {
            // Opposite-sign pairing: a synthetic forward is C − P. Where call
            // and put nets oppose, the matched portion is exactly ±m of base
            // delta — venue greeks cannot improve on the parity identity.
            // `paired` is signed: negative = conversion-like (short C + long
            // P → −m), positive = reversal-like (long C + short P → +m).
            let paired = if calls < dec!(0) && puts > dec!(0) {
                -(-calls).min(puts)
            } else if calls > dec!(0) && puts < dec!(0) {
                calls.min(-puts)
            } else {
                dec!(0)
            };
            let (res_calls, res_puts) = (calls - paired, puts + paired);
            let contribution = paired + res_calls - res_puts;
            if contribution != dec!(0) {
                delta_map.entry(base).or_insert((dec!(0), dec!(0))).0 += contribution;
            }
        }

        for (base, entry) in delta_map.iter_mut() {
            entry.1 = entry.0 * max_mark.get(base).copied().unwrap_or(dec!(0));
        }

        delta_map
    }

    /// Checks if any base asset violates delta neutrality limits.
    pub fn check_delta_neutrality(&self, positions: &[Position]) -> Result<(), (String, Decimal)> {
        let deltas = self.calculate_net_delta(positions);

        for (base, (_qty, usd_exposure)) in deltas {
            let abs_usd = usd_exposure.abs();
            if abs_usd > self.max_unhedged_usd_per_asset {
                warn!(
                    "Unhedged delta breach on asset {}: Net USD exposure ${:.2} (limit: ${:.2})",
                    base, usd_exposure, self.max_unhedged_usd_per_asset
                );
                return Err((base, usd_exposure));
            }
        }

        Ok(())
    }

    /// Pre-trade check for a proposed two-leg pair: aggregates signed base
    /// quantities (Buy positive, Sell negative) by uppercase base asset, then
    /// values each residual quantity at that base's maximum leg price. Rejects
    /// when any absolute exposure exceeds `max_unhedged_usd_per_asset`.
    /// Nonpositive quantities or prices are rejected as that leg's base with a
    /// sentinel exposure exceeding the limit.
    #[allow(clippy::too_many_arguments)]
    pub fn check_proposed_pair(
        &self,
        symbol_a: &str,
        side_a: Side,
        qty_a: Decimal,
        price_a: Decimal,
        symbol_b: &str,
        side_b: Side,
        qty_b: Decimal,
        price_b: Decimal,
    ) -> Result<(), (String, Decimal)> {
        let sentinel = self.max_unhedged_usd_per_asset.abs() + dec!(1);

        let base_a = symbol_a
            .split('-')
            .next()
            .unwrap_or(symbol_a)
            .to_uppercase();
        if qty_a <= dec!(0) || price_a <= dec!(0) {
            return Err((base_a, sentinel));
        }
        let base_b = symbol_b
            .split('-')
            .next()
            .unwrap_or(symbol_b)
            .to_uppercase();
        if qty_b <= dec!(0) || price_b <= dec!(0) {
            return Err((base_b, sentinel));
        }

        let signed_a = match side_a {
            Side::Buy => qty_a,
            Side::Sell => -qty_a,
        };
        let signed_b = match side_b {
            Side::Buy => qty_b,
            Side::Sell => -qty_b,
        };

        let mut net_qty: HashMap<String, Decimal> = HashMap::new();
        let mut max_px: HashMap<String, Decimal> = HashMap::new();
        for (base, signed, price) in [(base_a, signed_a, price_a), (base_b, signed_b, price_b)] {
            *net_qty.entry(base.clone()).or_insert(dec!(0)) += signed;
            let m = max_px.entry(base).or_insert(dec!(0));
            if price > *m {
                *m = price;
            }
        }

        for (base, qty) in net_qty {
            let usd_exposure = qty * max_px.get(&base).copied().unwrap_or(dec!(0));
            if usd_exposure.abs() > self.max_unhedged_usd_per_asset {
                warn!(
                    "Proposed pair breaches delta neutrality on asset {}: Net USD exposure ${:.2} (limit: ${:.2})",
                    base, usd_exposure, self.max_unhedged_usd_per_asset
                );
                return Err((base, usd_exposure));
            }
        }

        Ok(())
    }
}

/// Parse a Deribit-style option symbol (`BTC-27NOV26-112000-C`) into
/// (expiry, strike, is_call). Same four-part shape as the fixed-contract
/// detector used by the samples pipeline: ≥4 dash parts, last part `C`/`P`,
/// penultimate part numeric. Anything else returns `None` and counts as a
/// plain linear position.
fn parse_option_key(symbol: &str) -> Option<(String, String, bool)> {
    let parts: Vec<&str> = symbol.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let is_call = match *parts.last()? {
        "C" => true,
        "P" => false,
        _ => return None,
    };
    let strike = parts[parts.len() - 2];
    strike.parse::<f64>().ok()?;
    // Everything between base and strike is the expiry key — keeps pairs
    // grouped across tenor spellings without validating the date itself.
    let expiry = parts[1..parts.len() - 2].join("-");
    Some((expiry, strike.to_string(), is_call))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_proposed_pair_balanced_passes() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(60000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_check_proposed_pair_equal_qty_different_prices_passes() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Equal quantities hedge fully even when venue marks differ materially
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(62000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_calculate_net_delta_equal_qty_different_marks() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "v".into(),
            symbol: "BTC-USDC".into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let deltas = guard.calculate_net_delta(&[
            pos(Side::Buy, dec!(1.0), dec!(60000.0)),
            pos(Side::Sell, dec!(1.0), dec!(62000.0)),
        ]);
        let (net_qty, exposure) = deltas["BTC"];
        assert_eq!(net_qty, dec!(0));
        assert_eq!(exposure, dec!(0));
    }

    #[test]
    fn test_conversion_package_nets_to_zero_delta() {
        // Short call + long put at the same strike/expiry + long perp is a
        // delta-neutral conversion. Put-call parity makes the option pair
        // exactly −1 per unit — the +1 perp must cancel it precisely.
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "v".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let deltas = guard.calculate_net_delta(&[
            pos("BTC-27NOV26-112000-C", Side::Sell, dec!(1.0), dec!(274.0)),
            pos("BTC-27NOV26-112000-P", Side::Buy, dec!(1.0), dec!(28586.0)),
            pos("BTC-PERPETUAL", Side::Buy, dec!(0.99983), dec!(82984.0)),
        ]);
        let (qty, _) = deltas["BTC"];
        assert!(qty.abs() < dec!(0.001), "conversion must net ~0, got {qty}");
    }

    #[test]
    fn test_unpaired_option_counts_by_type() {
        // Lone options without a parity partner stay conservative: calls +1,
        // puts −1 per unit (deep-ITM bound). A straddle nets ~0.
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |sym: &str, side: Side, qty: Decimal| Position {
            venue: "v".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: dec!(1),
            mark_price: dec!(50000),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let lone_call =
            guard.calculate_net_delta(&[pos("BTC-27NOV26-112000-C", Side::Buy, dec!(0.5))]);
        assert_eq!(lone_call["BTC"].0, dec!(0.5));
        let straddle = guard.calculate_net_delta(&[
            pos("BTC-27NOV26-112000-C", Side::Buy, dec!(1.0)),
            pos("BTC-27NOV26-112000-P", Side::Buy, dec!(1.0)),
        ]);
        assert_eq!(straddle["BTC"].0, dec!(0));
    }

    #[test]
    fn test_check_proposed_pair_mismatched_fails() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Buy 1.0 @ 60000 vs sell 0.5 @ 70000 leaves +0.5 BTC valued at max leg price
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(0.5),
            dec!(70000.0),
        );
        let (base, exposure) = res.expect_err("mismatched pair must fail");
        assert_eq!(base, "BTC");
        assert_eq!(exposure, dec!(35000.0));
    }

    #[test]
    fn test_check_proposed_pair_cross_asset_independent() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Different base assets cannot hedge each other; each leg over limit
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "ETH-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(3000.0),
        );
        assert!(res.is_err());

        // Small balanced cross-asset pair within limit still passes each asset
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(0.01),
            dec!(60000.0),
            "ETH-USDC",
            Side::Sell,
            dec!(0.1),
            dec!(3000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_check_proposed_pair_nonpositive_rejected() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(0.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(60000.0),
        );
        let (base, exposure) = res.expect_err("zero qty must fail");
        assert_eq!(base, "BTC");
        assert!(exposure.abs() > dec!(1000.0));
    }
}
