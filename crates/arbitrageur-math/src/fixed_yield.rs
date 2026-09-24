//! Fixed-yield funding carry: a 4-leg variant of the cross-venue funding
//! trade where both legs' floating funding is swapped to a fixed rate via
//! Pendle Boros Yield-Unit (YU) positions.
//!
//! # Construction
//!
//! The floating carry is `long perp on L` + `short perp on S`, earning
//! `f_S − f_L` while the funding gap persists. The fixed variant adds two
//! Boros YU legs over the same notional and asset:
//!
//! - **Long YU on market L** — pays fixed `F_L`, receives floating `f_L`.
//!   The perp's `−f_L` and the YU's `+f_L` cancel; the leg costs a fixed
//!   `F_L` regardless of how funding moves.
//! - **Short YU on market S** — pays floating `f_S`, receives fixed `F_S`.
//!   The perp's `+f_S` and the YU's `−f_S` cancel; the leg earns a fixed
//!   `F_S`.
//!
//! Net carry becomes `F_S − F_L` — a **locked** rate for the YU term.
//!
//! # Executable rates
//!
//! Entering crosses the Boros book: a SHORT YU hits the bid (receives the
//! bid-side fixed APR), a LONG YU lifts the ask (pays the ask-side fixed
//! APR). Mid prices would overstate the edge, so evaluation uses
//! `bestBid`/`bestAsk`, never `midApr`.
//!
//! # Residuals this model does NOT hide
//!
//! - Hold-to-maturity is the locked case; unwinding early re-crosses the
//!   YU spread plus whatever the fixed rate moved to.
//! - YU legs consume Boros margin (low max leverage, typically ~1.5–3×)
//!   and per-transaction Arbitrum gas — both enter the cost side.
//! - The two YU legs must share one maturity; a mismatched lock leaves a
//!   residual floating window, so callers pair markets on identical
//!   `maturity_unix`.

use serde::{Deserialize, Serialize};

/// One Boros YU hedge leg, pre-priced at its executable side of the book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YuHedgeLeg {
    /// Our canonical venue id for the underlying funding platform.
    pub venue_id: String,
    pub yu_market_id: u64,
    /// Boros market symbol, e.g. `BINANCE-BTCUSDT-30OCT2026`.
    pub yu_symbol: String,
    /// `"long"` = pay fixed / receive floating; `"short"` = receive fixed
    /// / pay floating.
    pub yu_side: String,
    /// The executable fixed APR as a fraction (0.05 = 5%) — bid for a
    /// short YU, ask for a long YU.
    pub fixed_apr: f64,
    /// Taker fee as a fraction of notional (e.g. 0.0005 = 5 bps).
    pub taker_fee_rate: f64,
    /// Boros max leverage for the market — drives the margin estimate.
    pub max_leverage: f64,
    pub maturity_unix: i64,
}

/// Fully evaluated 4-leg fixed-yield candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedYieldOpportunity {
    /// Locked net APR as a fraction: `F_S − F_L`.
    pub fixed_apr: f64,
    /// Locked net APR in percent for display parity with `net_apr_pct`.
    pub fixed_apr_pct: f64,
    /// Hours until the shared YU maturity — the locked holding window.
    pub lock_hours: f64,
    pub lock_until_unix: i64,
    /// Gross locked income over the full term: `size × fixed_apr × t`.
    pub gross_income_usd: f64,
    /// All four legs' costs in USD: perp round-trip fees + measured entry
    /// spread + two YU taker fees + estimated gas.
    pub total_cost_usd: f64,
    /// `gross_income_usd − total_cost_usd` — deterministic while held to
    /// maturity (the rate cannot decay, unlike the floating variant).
    pub expected_return_usd: f64,
    /// `size × fixed_apr / 365`.
    pub daily_income_usd: f64,
    /// Extra cost vs the 2-leg variant, in bps of notional: the two YU
    /// taker fees plus gas amortized over the size.
    pub yu_hedge_cost_bps: f64,
    /// Estimated Boros collateral lock: `Σ size / maxLeverage` per leg.
    pub boros_margin_usd: f64,
    /// Size ceiling imposed by the thinner YU books (USD notional) —
    /// `None` when no YU-side bound is known.
    pub yu_capacity_usd: Option<f64>,
    /// Hours of locked accrual needed to cover `total_cost_usd`.
    pub break_even_hours: f64,
    pub long_leg: YuHedgeLeg,
    pub short_leg: YuHedgeLeg,
}

/// Evaluate the 4-leg fixed-yield variant of a carry pair.
///
/// `perp_rt_fee_bps` is the two-leg round-trip taker fee already computed
/// for the floating row; `entry_spread_bps` the measured entry cross (0
/// when unmeasured); `gas_usd` the per-batch Arbitrum cost estimate.
///
/// Returns `None` (fail closed — emit no row) when the lock is not
/// positive, the maturities differ, the term is shorter than
/// `min_lock_hours`, or the expected return cannot cover costs.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_fixed_yield_carry(
    long_yu: YuHedgeLeg,
    short_yu: YuHedgeLeg,
    trade_size_usd: f64,
    perp_rt_fee_bps: f64,
    entry_spread_bps: f64,
    gas_usd: f64,
    now_unix: i64,
    min_lock_hours: f64,
) -> Option<FixedYieldOpportunity> {
    // A shared maturity is what makes the lock coherent — mismatched
    // terms leave a residual floating window this model refuses to price.
    if long_yu.maturity_unix != short_yu.maturity_unix {
        return None;
    }
    let lock_secs = (long_yu.maturity_unix - now_unix) as f64;
    let lock_hours = lock_secs / 3600.0;
    if !(lock_hours.is_finite() && lock_hours >= min_lock_hours) {
        return None;
    }
    // Executable fixed rates only: short YU receives the bid, long YU
    // pays the ask.
    let f_s = short_yu.fixed_apr;
    let f_l = long_yu.fixed_apr;
    let fixed_apr = f_s - f_l;
    if !(fixed_apr.is_finite() && fixed_apr > 0.0) {
        return None;
    }
    let t_years = lock_secs / (365.0 * 86400.0);
    let gross_income = trade_size_usd * fixed_apr * t_years;
    let perp_cost = trade_size_usd * (perp_rt_fee_bps + entry_spread_bps.max(0.0)) / 1e4;
    let yu_fee_cost = trade_size_usd * (long_yu.taker_fee_rate + short_yu.taker_fee_rate);
    let total_cost = perp_cost + yu_fee_cost + gas_usd.max(0.0);
    let expected = gross_income - total_cost;
    if !(expected.is_finite() && expected > 0.0) {
        return None;
    }
    let hourly_accrual = trade_size_usd * fixed_apr / 8760.0;
    let break_even_hours = if hourly_accrual > 0.0 {
        total_cost / hourly_accrual
    } else {
        f64::INFINITY
    };
    // Boros YU leverage is low; margin estimate = Σ notional / maxLev.
    let margin = trade_size_usd / long_yu.max_leverage.max(1.0)
        + trade_size_usd / short_yu.max_leverage.max(1.0);
    Some(FixedYieldOpportunity {
        fixed_apr,
        fixed_apr_pct: fixed_apr * 100.0,
        lock_hours,
        lock_until_unix: long_yu.maturity_unix,
        gross_income_usd: gross_income,
        total_cost_usd: total_cost,
        expected_return_usd: expected,
        daily_income_usd: trade_size_usd * fixed_apr / 365.0,
        yu_hedge_cost_bps: (yu_fee_cost + gas_usd.max(0.0)) / trade_size_usd * 1e4,
        boros_margin_usd: margin,
        yu_capacity_usd: None,
        break_even_hours,
        long_leg: long_yu,
        short_leg: short_yu,
    })
}

/// Convert a fixed APR (fraction) into the integer Boros tick:
/// `rate = 1.00005^(tick × tickStep) − 1` ⇒
/// `tick = ln(1+rate) / (tickStep × ln 1.00005)`, rounded per side
/// (down for a LONG — never overpay; up for a SHORT — never undersell).
/// Exposed for tests and for callers that self-encode calldata.
pub fn fixed_apr_to_tick(rate: f64, tick_step: i64, round_up: bool) -> Option<i64> {
    if !(rate.is_finite() && rate > -1.0) || tick_step <= 0 {
        return None;
    }
    let raw = (1.0 + rate).ln() / (f64::from(tick_step as i32) * 1.00005f64.ln());
    let tick = if round_up { raw.ceil() } else { raw.floor() } as i64;
    Some(tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(side: &str, apr: f64, maturity: i64) -> YuHedgeLeg {
        YuHedgeLeg {
            venue_id: if side == "long" {
                "okx".into()
            } else {
                "hyperliquid".into()
            },
            yu_market_id: 1,
            yu_symbol: "X".into(),
            yu_side: side.into(),
            fixed_apr: apr,
            taker_fee_rate: 0.0005,
            max_leverage: 3.0,
            maturity_unix: maturity,
        }
    }

    #[test]
    fn fixed_yield_locks_the_spread() {
        let now = 1_800_000_000i64;
        let maturity = now + 30 * 86400;
        let opp = evaluate_fixed_yield_carry(
            leg("long", 0.02, maturity),
            leg("short", 0.10, maturity),
            10_000.0,
            10.0, // perp rt fee bps
            2.0,  // entry spread
            0.60, // gas
            now,
            24.0,
        )
        .expect("profitable lock emits");
        assert!((opp.fixed_apr - 0.08).abs() < 1e-9);
        assert!((opp.lock_hours - 720.0).abs() < 1e-6);
        // 10000 * 0.08 * 30/365 ≈ 65.75 gross; costs ≈ 10000*0.0012 +
        // 2*5bps*10000=10 + 0.6 → ~22.6 → expected ≈ 43.2
        assert!(opp.expected_return_usd > 40.0 && opp.expected_return_usd < 46.0);
        assert!(opp.yu_hedge_cost_bps > 9.0);
        assert!(opp.boros_margin_usd > 6000.0);
    }

    #[test]
    fn rejects_mismatched_maturity() {
        let now = 1_800_000_000i64;
        assert!(evaluate_fixed_yield_carry(
            leg("long", 0.02, now + 30 * 86400),
            leg("short", 0.10, now + 60 * 86400),
            10_000.0,
            10.0,
            0.0,
            0.0,
            now,
            24.0,
        )
        .is_none());
    }

    #[test]
    fn rejects_negative_or_unprofitable_lock() {
        let now = 1_800_000_000i64;
        let maturity = now + 30 * 86400;
        // Inverted spread: paying more than receiving.
        assert!(evaluate_fixed_yield_carry(
            leg("long", 0.12, maturity),
            leg("short", 0.05, maturity),
            10_000.0,
            10.0,
            0.0,
            0.0,
            now,
            24.0,
        )
        .is_none());
        // Positive spread but fees exceed income (tiny term).
        assert!(evaluate_fixed_yield_carry(
            leg("long", 0.02, maturity),
            leg("short", 0.021, maturity),
            10_000.0,
            10.0,
            5.0,
            1.0,
            now,
            24.0,
        )
        .is_none());
    }

    #[test]
    fn rejects_short_lock() {
        let now = 1_800_000_000i64;
        assert!(evaluate_fixed_yield_carry(
            leg("long", 0.02, now + 3600),
            leg("short", 0.10, now + 3600),
            10_000.0,
            10.0,
            0.0,
            0.0,
            now,
            24.0,
        )
        .is_none());
    }

    #[test]
    fn tick_conversion_matches_formula() {
        // rate = 1.00005^(tick*step) - 1 ⇒ round-trip identity.
        for step in [1i64, 2, 5] {
            let tick = fixed_apr_to_tick(0.05, step, false).unwrap();
            let back = 1.00005f64.powi((tick * step) as i32) - 1.0;
            assert!(back <= 0.05 && back > 0.05 - 0.001);
            let tick_up = fixed_apr_to_tick(0.05, step, true).unwrap();
            let back_up = 1.00005f64.powi((tick_up * step) as i32) - 1.0;
            assert!(back_up >= 0.05);
        }
        assert!(fixed_apr_to_tick(f64::NAN, 2, false).is_none());
        assert!(fixed_apr_to_tick(0.05, 0, false).is_none());
    }
}
