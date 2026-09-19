//! External (non-system) position manager — backend.
//!
//! WHAT: positions that exist on exchange accounts but were NOT opened by this
//! system (e.g. manual trades placed on exchange websites). The system knows
//! its own legs from `signal_executions`; anything on the live account above
//! that is flagged external and can be closed through the bot.
//!
//! WHY through the bot: one click closes at a depth-aware limit (or market
//! with measured slippage) instead of hand-typing orders per website — less
//! friction (spread drift, book depth, limit-vs-market choice) and less time.
//!
//! SAFETY (fail-closed, same rules as the rest of the engine):
//! - Reads need an unlocked vault + loaded live credentials (same gate as
//!   `get_venue_balances`). Per-venue failures become `unreadable` notes —
//!   never fabricated or zero-filled positions.
//! - Closes need Live Trading mode + unlocked vault + a venue whose adapter
//!   reports `supports_live_execution`. Reduce-only orders only, full residual
//!   quantity, fresh re-read right before submit (no stale-qty closes).
//! - Every close is recorded as a strategy-"external" signal execution so it
//!   shows up in attribution and audit like any other managed trade.

use crate::state::AppEngineState;
use crate::state_store::{
    canonical_base, LegSide, NewSignalExecution, NewSignalLeg, SignalStatus,
};
use arbitrageur_core::order::OrderRequest;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use tauri::State;

const DUST_NOTIONAL_USD: f64 = 1.0;
const STALE_BOOK_MS: i64 = 5_000;

/// One live on-exchange position minus everything the system opened itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPositionDto {
    pub key: String,
    pub venue: String,
    pub symbol: String,
    pub base: String,
    /// "long" | "short" (exchange-side direction).
    pub side: String,
    pub quantity: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub notional_usd: f64,
    pub unrealized_pnl_usd: f64,
    pub liquidation_price: Option<f64>,
    pub liquidation_distance_pct: Option<f64>,
    pub leverage: f64,
    /// System-owned qty on the same (venue, base, side) — subtracted out.
    pub system_qty: f64,
    /// Live qty minus system qty: what "close" would actually flatten.
    pub external_qty: f64,
    pub close_supported: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPositionsResponse {
    pub positions: Vec<ExternalPositionDto>,
    /// Per-venue read problems ("hyperliquid: no credentials", …). Empty = all read.
    pub unreadable: Vec<String>,
    pub live_mode: bool,
}

/// Friction-aware close quote for one external position.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseRecommendationDto {
    pub venue: String,
    pub symbol: String,
    /// Side of the CLOSING order ("Buy" to cover a short, "Sell" to sell a long).
    pub close_side: String,
    pub close_qty: f64,
    /// Best price on the closing side's book (bid for sells, ask for buys).
    pub touch_price: f64,
    pub touch_spread_bps: f64,
    /// Volume-weighted average fill if the full qty crossed the spread now.
    pub market_vwap: f64,
    /// (touch − vwap)/touch in bps: the market order's depth cost.
    pub vwap_slip_bps: f64,
    /// How much of close_qty the visible book can absorb.
    pub depth_fillable_qty: f64,
    pub full_size_fillable: bool,
    pub recommended_mode: String,
    pub recommended_limit_price: Option<f64>,
    /// Venue taker-fee assumption used in the friction roll-up (input, default 5bps).
    pub taker_fee_bps: f64,
    /// slip + spread/2 + taker fee: all-in friction estimate for a market close.
    pub est_friction_bps: f64,
    /// Live mid vs the account's mark price (mark age unknown — indicative only).
    pub mid_vs_mark_bps: Option<f64>,
    pub book_age_ms: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseExternalPositionInput {
    pub venue: String,
    pub symbol: String,
    /// "limit" | "market".
    pub mode: String,
    pub limit_price: Option<f64>,
    pub taker_fee_bps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseResultDto {
    pub venue: String,
    pub symbol: String,
    pub mode: String,
    pub order_id: Option<String>,
    pub filled_qty: Option<f64>,
    pub avg_fill_price: Option<f64>,
    pub status: String,
    pub note: String,
}

// --- pure helpers (unit-tested below) -----------------------------------------

/// Live qty minus system qty, floored at zero. NaN-safe: garbage in → 0 out.
pub fn residual_qty(live_qty: f64, system_qty: f64) -> f64 {
    if !live_qty.is_finite() || !system_qty.is_finite() {
        return 0.0;
    }
    (live_qty - system_qty).max(0.0)
}

/// Walk price levels (best-first) filling `qty`; returns (vwap, filled).
/// Levels are (price, size); empty/zero qty → None (never a fake 0-price).
pub fn vwap_for_size(levels: &[(f64, f64)], qty: f64) -> Option<(f64, f64)> {
    if !(qty.is_finite()) || qty <= 0.0 {
        return None;
    }
    let mut remaining = qty;
    let mut cost = 0.0;
    let mut filled = 0.0;
    for (price, size) in levels {
        if !price.is_finite() || !size.is_finite() || *price <= 0.0 || *size <= 0.0 {
            continue;
        }
        let take = remaining.min(*size);
        cost += take * price;
        filled += take;
        remaining -= take;
        if remaining <= 1e-12 {
            break;
        }
    }
    if filled <= 0.0 {
        return None;
    }
    Some((cost / filled, filled))
}

fn dec_f64(d: Decimal) -> f64 {
    d.to_string().parse::<f64>().unwrap_or(0.0)
}

fn f64_dec(v: f64) -> Option<Decimal> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    Decimal::from_str(&v.to_string()).ok()
}

/// Sum of open system leg qtys on (venue, base, side-sign). Only legs of
/// non-terminal signals count; closed/failed history never shields a live
/// position from being shown as external.
fn system_qty_for(
    signals: &[crate::state_store::SignalExecution],
    venue_lc: &str,
    base: &str,
    side_sign: f64,
) -> f64 {
    signals
        .iter()
        .filter(|s| {
            !matches!(s.status, SignalStatus::Closed | SignalStatus::Failed)
                && canonical_base(&s.symbol) == base
        })
        .flat_map(|s| s.legs.iter())
        .filter(|l| {
            l.venue.trim().to_lowercase() == venue_lc
                && l.filled_qty.is_finite()
                && l.filled_qty > 0.0
        })
        .filter(|l| match l.side {
            LegSide::Long => side_sign > 0.0,
            LegSide::Short => side_sign < 0.0,
        })
        .map(|l| l.filled_qty)
        .sum()
}

// --- impls --------------------------------------------------------------------

/// List live on-exchange positions with system legs subtracted out.
/// Mirrors the `get_venue_balances` credential gates; per-venue failures are
/// reported in `unreadable`, never raised (one dark venue must not hide the
/// rest of the portfolio).
pub async fn get_external_positions_impl(
    state: &AppEngineState,
) -> Result<ExternalPositionsResponse, String> {
    let live_mode = *state.is_live_mode.read();
    if state.vault.write().is_unlocked() {
        state.sync_vault_credentials_to_adapters();
    }
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;

    let mut positions = Vec::new();
    let mut unreadable = Vec::new();
    for (vid, name, adapter) in state.get_venues() {
        if !adapter.has_live_credentials() {
            unreadable.push(format!("{name}: no live credentials in the vault"));
            continue;
        }
        let account = match adapter.fetch_account().await {
            Ok(a) => a,
            Err(e) => {
                unreadable.push(format!("{name}: account read failed: {e}"));
                continue;
            }
        };
        let venue_lc = vid.to_string().to_lowercase();
        let close_supported =
            adapter.supports_live_execution() && adapter.has_live_credentials();
        for p in &account.positions {
            let qty = dec_f64(p.quantity);
            if !qty.is_finite() || qty <= 0.0 {
                continue;
            }
            let base = canonical_base(&p.symbol);
            let sq = p.signed_quantity();
            let side_sign = (sq.is_sign_positive() as i32 - sq.is_sign_negative() as i32) as f64;
            if side_sign == 0.0 {
                continue;
            }
            let sys = system_qty_for(&signals, &venue_lc, &base, side_sign);
            let ext = residual_qty(qty, sys);
            let mark = dec_f64(p.mark_price);
            if ext * mark < DUST_NOTIONAL_USD {
                continue;
            }
            let entry = dec_f64(p.entry_price);
            let upnl = dec_f64(p.unrealized_pnl);
            positions.push(ExternalPositionDto {
                key: format!("external|{}|{}|{}", venue_lc, base, if side_sign > 0.0 { "long" } else { "short" }),
                venue: venue_lc.clone(),
                symbol: p.symbol.clone(),
                base: base.clone(),
                side: if side_sign > 0.0 { "long".into() } else { "short".into() },
                quantity: qty,
                entry_price: entry,
                mark_price: mark,
                notional_usd: ext * mark,
                unrealized_pnl_usd: upnl,
                liquidation_price: p
                    .liquidation_price
                    .map(dec_f64)
                    .filter(|v| v.is_finite() && *v > 0.0),
                liquidation_distance_pct: p
                    .liquidation_distance_pct()
                    .map(dec_f64)
                    .filter(|v| v.is_finite()),
                leverage: dec_f64(p.leverage),
                system_qty: sys,
                external_qty: ext,
                close_supported,
                updated_at: p.updated_at.to_rfc3339(),
            });
        }
    }
    positions.sort_by(|a, b| {
        b.notional_usd
            .partial_cmp(&a.notional_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(ExternalPositionsResponse {
        positions,
        unreadable,
        live_mode,
    })
}

/// Orderbook depth (bids, asks) plus the book timestamp.
type DepthWithTs = (Vec<(f64, f64)>, Vec<(f64, f64)>, i64);

fn hub_book_levels(
    state: &AppEngineState,
    venue: &str,
    symbol: &str,
) -> Result<DepthWithTs, String> {
    let book = state
        .market_data_hub
        .get_valid_orderbook(&VenueId::from(venue), symbol, None)
        .ok_or_else(|| format!("No live orderbook for {symbol} on {venue} — refresh the venue feed first"))?;
    let bids: Vec<(f64, f64)> = book
        .bids
        .iter()
        .map(|l| (dec_f64(l.price), dec_f64(l.size)))
        .filter(|(p, s)| p.is_finite() && s.is_finite() && *p > 0.0 && *s > 0.0)
        .collect();
    let asks: Vec<(f64, f64)> = book
        .asks
        .iter()
        .map(|l| (dec_f64(l.price), dec_f64(l.size)))
        .filter(|(p, s)| p.is_finite() && s.is_finite() && *p > 0.0 && *s > 0.0)
        .collect();
    if bids.is_empty() || asks.is_empty() {
        return Err(format!("Orderbook for {symbol} on {venue} has an empty side — cannot price a close"));
    }
    let age_ms = Utc::now()
        .signed_duration_since(book.timestamp)
        .num_milliseconds();
    Ok((bids, asks, age_ms))
}

/// Price a friction-aware close: touch vs depth-VWAP, limit-vs-market call,
/// and an honest warning list. Read-only — places no orders.
pub async fn recommend_external_close_impl(
    venue: &str,
    symbol: &str,
    taker_fee_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<CloseRecommendationDto, String> {
    let venue_lc = venue.trim().to_lowercase();
    if state.vault.write().is_unlocked() {
        state.sync_vault_credentials_to_adapters();
    }
    let adapter = state
        .get_adapter(&venue_lc)
        .ok_or_else(|| format!("Unknown venue '{venue}'"))?;
    if !adapter.has_live_credentials() {
        return Err(format!("No live credentials for {venue} — unlock the vault and configure the account first"));
    }
    let account = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("Account read failed for {venue}: {e}"))?;
    let base = canonical_base(symbol);
    let pos = account
        .positions
        .iter()
        .find(|p| {
            canonical_base(&p.symbol) == base
                && dec_f64(p.quantity) > 0.0
        })
        .ok_or_else(|| format!("No live position in {base} on {venue}"))?;
    let qty = dec_f64(pos.quantity);
    let long = pos.signed_quantity() > dec!(0);
    let close_side = if long { "Sell" } else { "Buy" };

    // Hub symbols may differ in quote suffix; try the live spelling first,
    // then fall back to any hub book for the same base on this venue.
    let (bids, asks, age_ms) = match hub_book_levels(state, &venue_lc, &pos.symbol) {
        Ok(v) => v,
        Err(_) => hub_book_levels(state, &venue_lc, symbol)
            .map_err(|e| e.to_string())?,
    };
    // Closing levels: a long sells into bids, a short buys from asks.
    let levels: Vec<(f64, f64)> = if long { bids.clone() } else { asks.clone() };
    let touch = levels[0].0;
    let best_bid = bids[0].0;
    let best_ask = asks[0].0;
    let mid = (best_bid + best_ask) / 2.0;
    let touch_spread_bps = if mid > 0.0 { (best_ask - best_bid) / mid * 10_000.0 } else { 0.0 };

    let (vwap, filled) = vwap_for_size(&levels, qty).ok_or_else(|| {
        format!("Book too thin to price closing {qty} {base} on {venue}")
    })?;
    let full_fill = filled + 1e-9 >= qty;
    let slip_bps = if touch > 0.0 {
        // Signed so that paying through the touch reads positive.
        let raw = if long { (touch - vwap) / touch } else { (vwap - touch) / touch };
        raw * 10_000.0
    } else {
        0.0
    };
    let fee_bps = match taker_fee_bps {
        Some(f) if f.is_finite() && (0.0..=1_000.0).contains(&f) => f,
        _ => 5.0,
    };
    let est_friction = slip_bps.max(0.0) + touch_spread_bps / 2.0 + fee_bps;
    let mark = dec_f64(pos.mark_price);
    let drift = if mark > 0.0 && mid > 0.0 {
        Some((mid - mark) / mark * 10_000.0)
    } else {
        None
    };

    let mut warnings = Vec::new();
    if age_ms > STALE_BOOK_MS {
        warnings.push(format!("Book snapshot is {age_ms}ms old — re-check before submitting"));
    }
    if !full_fill {
        warnings.push(format!(
            "Visible depth only absorbs {filled:.4} of {qty:.4} {base} — a market close would walk past the book"
        ));
    }
    if slip_bps > 25.0 {
        warnings.push(format!("Depth slip is {slip_bps:.1} bps — prefer a pegged limit and work the order"));
    }
    if let Some(d) = drift {
        if d.abs() > 50.0 {
            warnings.push(format!(
                "Live mid sits {d:.0} bps off the account mark — spread has drifted since the mark printed"
            ));
        }
    }
    let recommended_mode = if full_fill && slip_bps <= 25.0 { "limit" } else { "market" };
    Ok(CloseRecommendationDto {
        venue: venue_lc,
        symbol: pos.symbol.clone(),
        close_side: close_side.into(),
        close_qty: qty,
        touch_price: touch,
        touch_spread_bps,
        market_vwap: vwap,
        vwap_slip_bps: slip_bps,
        depth_fillable_qty: filled,
        full_size_fillable: full_fill,
        recommended_mode: recommended_mode.into(),
        recommended_limit_price: if recommended_mode == "limit" { Some(touch) } else { None },
        taker_fee_bps: fee_bps,
        est_friction_bps: est_friction,
        mid_vs_mark_bps: drift,
        book_age_ms: age_ms,
        warnings,
    })
}

/// Close the external residual with a live reduce-only order.
/// Paper mode refuses (there is no paper equivalent of a real position);
/// the recommendation endpoint stays available for analysis either way.
pub async fn close_external_position_impl(
    input: CloseExternalPositionInput,
    state: &AppEngineState,
) -> Result<CloseResultDto, String> {
    if !*state.is_live_mode.read() {
        return Err("Switch to Live Trading to close external positions — paper mode cannot flatten real exchange positions".into());
    }
    if !state.vault.write().is_unlocked() {
        return Err("Vault is locked. Unlock before closing external positions.".into());
    }
    state.sync_vault_credentials_to_adapters();
    let venue_lc = input.venue.trim().to_lowercase();
    let adapter = state
        .get_adapter(&venue_lc)
        .ok_or_else(|| format!("Unknown venue '{}'", input.venue))?;
    if !adapter.supports_live_execution() {
        return Err(format!("{venue_lc} does not support live close orders through the bot yet"));
    }
    if !adapter.has_live_credentials() {
        return Err(format!("No live credentials for {venue_lc}"));
    }
    let mode = input.mode.trim().to_lowercase();
    if mode != "limit" && mode != "market" {
        return Err("mode must be 'limit' or 'market'".into());
    }
    // Fresh re-read: never close a quantity observed seconds ago.
    let account = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("Account re-read failed for {venue_lc}: {e}"))?;
    let base = canonical_base(&input.symbol);
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let live = account
        .positions
        .iter()
        .find(|p| canonical_base(&p.symbol) == base && dec_f64(p.quantity) > 0.0)
        .ok_or_else(|| format!("No live position in {base} on {venue_lc} (already flat?)"))?;
    let qty = dec_f64(live.quantity);
    let long = live.signed_quantity() > dec!(0);
    let sign = if long { 1.0 } else { -1.0 };
    let ext = residual_qty(qty, system_qty_for(&signals, &venue_lc, &base, sign));
    if ext * dec_f64(live.mark_price) < DUST_NOTIONAL_USD {
        return Err(format!("Nothing external left to close in {base} on {venue_lc}"));
    }
    let qty_dec = f64_dec(ext)
        .ok_or_else(|| "External quantity is not finite — refusing".to_string())?;
    let close_side = if long { Side::Sell } else { Side::Buy };
    let mut order = if mode == "limit" {
        let px = input
            .limit_price
            .filter(|p| p.is_finite() && *p > 0.0)
            .and_then(f64_dec)
            .ok_or_else(|| "Limit close needs a positive limit_price (use the recommendation)".to_string())?;
        let mut o = OrderRequest::new_limit(
            VenueId::from(venue_lc.clone()),
            live.symbol.clone(),
            close_side,
            px,
            qty_dec,
        );
        o.reduce_only = true;
        o
    } else {
        let mut o = OrderRequest::new_market(
            VenueId::from(venue_lc.clone()),
            live.symbol.clone(),
            close_side,
            qty_dec,
        );
        o.reduce_only = true;
        o
    };
    // Tag the client order so venue-side fills reconcile to this flow.
    order.client_order_id = format!("extclose-{}-{}", venue_lc, Utc::now().timestamp_millis());

    let _guard = state.execution_lock.lock().await;
    let resp = adapter
        .place_order(order)
        .await
        .map_err(|e| format!("Close order rejected on {venue_lc}: {e}"))?;

    // Record under strategy "external" so attribution + audit treat the close
    // like any other managed trade (1-leg reduce, filled from the response).
    let sid = format!("extclose-{}", Utc::now().timestamp_millis());
    let leg_side = if long { LegSide::Short } else { LegSide::Long };
    let exec = state
        .state_store
        .record_signal_execution(&NewSignalExecution {
            id: sid,
            strategy_id: "external".into(),
            symbol: live.symbol.clone(),
            route_id: None,
            expected_net_usd: None,
            note: Some(format!("External {base} close via bot ({mode})")),
            legs: vec![NewSignalLeg {
                account_id: None,
                venue: venue_lc.clone(),
                side: leg_side,
                intended_qty: ext,
            }],
        })
        .map_err(|e| e.to_string())?;
    let filled = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
    let avg = resp
        .avg_fill_price
        .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0));
    if filled > 0.0 {
        let _ = state.state_store.apply_leg_fill(
            &exec.id,
            leg_side,
            filled,
            avg.unwrap_or(0.0).max(0.000_001),
            resp.fee.to_string().parse::<f64>().unwrap_or(0.0),
            Some(resp.order_id.as_str()),
        );
        let _ = state.state_store.update_signal_status(
            &exec.id,
            SignalStatus::Closed,
            Some(format!("External close filled {filled} @ {:?}", avg).as_str()),
        );
    }
    Ok(CloseResultDto {
        venue: venue_lc,
        symbol: live.symbol.clone(),
        mode,
        order_id: Some(resp.order_id),
        filled_qty: Some(filled),
        avg_fill_price: avg,
        status: if filled > 0.0 { "filled".into() } else { "submitted".into() },
        note: format!(
            "Reduce-only {close_side:?} {ext} {base}: {}",
            if filled > 0.0 { "filled" } else { "working — re-check the venue" }
        ),
    })
}

// --- Tauri wrappers -------------------------------------------------------------

#[tauri::command]
pub async fn get_external_positions(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<ExternalPositionsResponse, String> {
    get_external_positions_impl(&state).await
}

#[tauri::command]
pub async fn recommend_external_close(
    venue: String,
    symbol: String,
    taker_fee_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<CloseRecommendationDto, String> {
    recommend_external_close_impl(&venue, &symbol, taker_fee_bps, &state).await
}

#[tauri::command]
pub async fn close_external_position(
    input: CloseExternalPositionInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<CloseResultDto, String> {
    close_external_position_impl(input, &state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residual_subtracts_system_legs_and_floors_at_zero() {
        assert_eq!(residual_qty(2.5, 1.0), 1.5);
        assert_eq!(residual_qty(1.0, 1.0), 0.0);
        assert_eq!(residual_qty(0.5, 1.0), 0.0);
        assert_eq!(residual_qty(f64::NAN, 1.0), 0.0);
        assert_eq!(residual_qty(1.0, f64::INFINITY), 0.0);
    }

    #[test]
    fn vwap_walks_levels_and_reports_partial_fills() {
        let levels = vec![(100.0, 1.0), (101.0, 1.0)];
        let (vwap, filled) = vwap_for_size(&levels, 1.5).expect("priced");
        assert!((vwap - 100.333_333).abs() < 1e-4);
        assert_eq!(filled, 1.5);
        // Thinner than asked: honest partial, never padded.
        let (vwap2, filled2) = vwap_for_size(&levels, 5.0).expect("partial");
        assert_eq!(filled2, 2.0);
        assert!((vwap2 - 100.5).abs() < 1e-9);
        assert!(vwap_for_size(&[], 1.0).is_none());
        assert!(vwap_for_size(&levels, 0.0).is_none());
        assert!(vwap_for_size(&[(100.0, 0.0)], 1.0).is_none());
    }
}
