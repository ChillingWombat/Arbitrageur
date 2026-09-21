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
//!   reports `live_order_submission_implemented`. Reduce-only orders only, full
//!   residual quantity, fresh re-read right before submit (no stale-qty closes).
//! - Every close is recorded as a strategy-"external" signal execution so it
//!   shows up in attribution and audit like any other managed trade.

use crate::state::AppEngineState;
use crate::state_store::{canonical_base, LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};
use arbitrageur_core::order::{OrderRequest, OrderResponse};
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
    /// Per-venue read-vs-shown reconciliation ("hyperliquid: read 2 live
    /// position(s), 1 shown externally (1 fully system-owned, …)"). Only
    /// present when rows were read but not all surface — answers "why don't
    /// I see venue X's position?" without guessing.
    pub venue_notes: Vec<String>,
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

/// One-line diagnostic when a venue's live rows don't all surface as
/// external positions. `None` in the happy path (nothing read, or everything
/// read is shown) so the UI stays quiet unless there is something to explain.
pub fn venue_position_note(
    venue_name: &str,
    raw: usize,
    shown: usize,
    system_owned: usize,
    dust_skipped: usize,
    malformed: usize,
) -> Option<String> {
    if raw == 0 || shown >= raw {
        return None;
    }
    Some(format!(
        "{venue_name}: read {raw} live position(s), {shown} shown externally          ({system_owned} fully system-owned, {dust_skipped} dust under $1, {malformed} malformed)"
    ))
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

/// Sum of open system leg qtys on (venue, exact symbol, side-sign). Only
/// legs of non-terminal signals count; closed/failed history never shields a
/// live position from being shown as external. Attribution keys on each
/// leg's own exact symbol — same-base instruments must not net against one
/// another.
fn system_qty_for(
    signals: &[crate::state_store::SignalExecution],
    venue_lc: &str,
    exact_symbol: &str,
    side_sign: f64,
) -> f64 {
    signals
        .iter()
        .filter(|s| !matches!(s.status, SignalStatus::Closed | SignalStatus::Failed))
        .flat_map(|s| s.legs.iter())
        .filter(|l| {
            l.venue.trim().to_lowercase() == venue_lc
                && l.filled_qty.is_finite()
                && l.filled_qty > 0.0
                && l.symbol.eq_ignore_ascii_case(exact_symbol)
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
    let mut venue_notes = Vec::new();
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
            adapter.live_order_submission_implemented() && adapter.has_live_credentials();
        let (mut raw, mut shown, mut sys_n, mut dust_n, mut bad_n) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        for raw_position in &account.positions {
            raw += 1;
            let p = match adapter.normalize_account_position(raw_position) {
                Ok(Some(p)) => p,
                Ok(None) | Err(_) => {
                    bad_n += 1;
                    continue;
                }
            };
            let qty = dec_f64(p.quantity);
            if !qty.is_finite() || qty <= 0.0 {
                bad_n += 1;
                continue;
            }
            let base = canonical_base(&p.symbol);
            let sq = p.signed_quantity();
            let side_sign = (sq.is_sign_positive() as i32 - sq.is_sign_negative() as i32) as f64;
            if side_sign == 0.0 {
                bad_n += 1;
                continue;
            }
            let sys = system_qty_for(&signals, &venue_lc, &p.symbol, side_sign);
            let ext = residual_qty(qty, sys);
            if ext <= 0.0 {
                sys_n += 1;
                continue;
            }
            let mark = dec_f64(p.mark_price);
            if ext * mark < DUST_NOTIONAL_USD {
                dust_n += 1;
                continue;
            }
            shown += 1;
            let entry = dec_f64(p.entry_price);
            let upnl = dec_f64(p.unrealized_pnl);
            positions.push(ExternalPositionDto {
                key: format!(
                    "external|{}|{}|{}",
                    venue_lc,
                    p.symbol,
                    if side_sign > 0.0 { "long" } else { "short" }
                ),
                venue: venue_lc.clone(),
                symbol: p.symbol.clone(),
                base: base.clone(),
                side: if side_sign > 0.0 {
                    "long".into()
                } else {
                    "short".into()
                },
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
        if let Some(note) = venue_position_note(name, raw, shown, sys_n, dust_n, bad_n) {
            venue_notes.push(note);
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
        venue_notes,
        live_mode,
    })
}

/// Orderbook depth (bids, asks) plus the book timestamp.
type DepthWithTs = (Vec<(f64, f64)>, Vec<(f64, f64)>, i64);

fn depth_from_book(
    book: &arbitrageur_core::quote::OrderBook,
    venue: &str,
    symbol: &str,
) -> Result<DepthWithTs, String> {
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
        return Err(format!(
            "Orderbook for {symbol} on {venue} has an empty side — cannot price a close"
        ));
    }
    let age_ms = Utc::now()
        .signed_duration_since(book.timestamp)
        .num_milliseconds();
    Ok((bids, asks, age_ms))
}

/// Resolve a fresh book for a close: hub first (keys are normalized, so
/// WS bare-base books and REST canonical books share one slot), then a
/// single adapter REST fetch across spelling variants that seeds the hub.
/// Fail-closed: when every source is unavailable the error says so; no
/// price is ever synthesized.
async fn close_book_levels(
    state: &AppEngineState,
    adapter: &Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    venue: &str,
    symbols: &[&str],
) -> Result<DepthWithTs, String> {
    let venue_id = VenueId::from(venue);
    for s in symbols {
        if let Some(book) = state
            .market_data_hub
            .get_valid_orderbook(&venue_id, s, None)
        {
            if let Ok(depth) = depth_from_book(&book, venue, s) {
                return Ok(depth);
            }
        }
    }
    let mut aliases: Vec<String> = Vec::new();
    for s in symbols {
        for alias in crate::state_store::spread_symbol_aliases(s) {
            if !aliases.contains(&alias) {
                aliases.push(alias);
            }
        }
    }
    let mut rest_err = String::new();
    for alias in &aliases {
        match adapter.fetch_orderbook(alias).await {
            Ok(book) => {
                state.market_data_hub.update_orderbook(book.clone());
                if let Ok(depth) = depth_from_book(&book, venue, alias) {
                    return Ok(depth);
                }
            }
            Err(e) => rest_err = e.to_string(),
        }
    }
    Err(if rest_err.is_empty() {
        format!(
            "No live orderbook for {} on {venue} — feed is stale or disconnected",
            symbols.first().unwrap_or(&"?")
        )
    } else {
        format!(
            "No live orderbook for {} on {venue} — feed is stale and REST fetch failed: {rest_err}",
            symbols.first().unwrap_or(&"?")
        )
    })
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
        return Err(format!(
            "No live credentials for {venue} — unlock the vault and configure the account first"
        ));
    }
    let account = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("Account read failed for {venue}: {e}"))?;
    let pos = adapter
        .account_position_for(&account, symbol)
        .map_err(|e| format!("Could not resolve {symbol} on {venue}: {e}"))?
        .ok_or_else(|| format!("No live position in {symbol} on {venue}"))?;
    let base = canonical_base(&pos.symbol);
    let qty = dec_f64(pos.quantity);
    let long = pos.signed_quantity() > dec!(0);
    let close_side = if long { "Sell" } else { "Buy" };

    // Hub books key on whichever spelling fed them (WS lanes publish bare
    // bases, REST polls echo the caller's symbol); aliases cover both, then
    // a one-shot REST fetch is the cold-start fallback.
    let (bids, asks, age_ms) =
        close_book_levels(state, &adapter, &venue_lc, &[&pos.symbol, symbol]).await?;
    // Closing levels: a long sells into bids, a short buys from asks.
    let levels: Vec<(f64, f64)> = if long { bids.clone() } else { asks.clone() };
    let touch = levels[0].0;
    let best_bid = bids[0].0;
    let best_ask = asks[0].0;
    let mid = (best_bid + best_ask) / 2.0;
    let touch_spread_bps = if mid > 0.0 {
        (best_ask - best_bid) / mid * 10_000.0
    } else {
        0.0
    };

    let (vwap, filled) = vwap_for_size(&levels, qty)
        .ok_or_else(|| format!("Book too thin to price closing {qty} {base} on {venue}"))?;
    let full_fill = filled + 1e-9 >= qty;
    let slip_bps = if touch > 0.0 {
        // Signed so that paying through the touch reads positive.
        let raw = if long {
            (touch - vwap) / touch
        } else {
            (vwap - touch) / touch
        };
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
        warnings.push(format!(
            "Book snapshot is {age_ms}ms old — re-check before submitting"
        ));
    }
    if !full_fill {
        warnings.push(format!(
            "Visible depth only absorbs {filled:.4} of {qty:.4} {base} — a market close would walk past the book"
        ));
    }
    if slip_bps > 25.0 {
        warnings.push(format!(
            "Depth slip is {slip_bps:.1} bps — prefer a pegged limit and work the order"
        ));
    }
    if let Some(d) = drift {
        if d.abs() > 50.0 {
            warnings.push(format!(
                "Live mid sits {d:.0} bps off the account mark — spread has drifted since the mark printed"
            ));
        }
    }
    let recommended_mode = if full_fill && slip_bps <= 25.0 {
        "limit"
    } else {
        "market"
    };
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
        recommended_limit_price: if recommended_mode == "limit" {
            Some(touch)
        } else {
            None
        },
        taker_fee_bps: fee_bps,
        est_friction_bps: est_friction,
        mid_vs_mark_bps: drift,
        book_age_ms: age_ms,
        warnings,
    })
}

fn validate_close_response(
    adapter_venue: &VenueId,
    expected_symbol: &str,
    expected_client_order_id: &str,
    expected_qty: Decimal,
    resp: &OrderResponse,
) -> Result<(), String> {
    let attributable = resp.venue == *adapter_venue
        && resp.symbol == expected_symbol
        && resp.client_order_id == expected_client_order_id
        && !resp.order_id.trim().is_empty()
        && resp.order_id != "0";
    let bounded_qty = resp.original_qty == expected_qty
        && resp.filled_qty >= dec!(0)
        && resp.filled_qty <= expected_qty
        && resp.remaining_qty >= dec!(0)
        && resp.remaining_qty <= expected_qty;
    let valid_status = match resp.status {
        arbitrageur_core::types::OrderStatus::New => {
            resp.filled_qty == dec!(0) && resp.remaining_qty == expected_qty
        }
        arbitrageur_core::types::OrderStatus::PartiallyFilled => {
            resp.filled_qty > dec!(0)
                && resp.filled_qty < expected_qty
                && resp.remaining_qty == expected_qty - resp.filled_qty
        }
        arbitrageur_core::types::OrderStatus::Filled => {
            resp.filled_qty == expected_qty && resp.remaining_qty == dec!(0)
        }
        arbitrageur_core::types::OrderStatus::Cancelled
        | arbitrageur_core::types::OrderStatus::Rejected
        | arbitrageur_core::types::OrderStatus::Expired => {
            resp.filled_qty == dec!(0) && resp.remaining_qty == expected_qty
        }
    };
    if !(valid_status && attributable && bounded_qty && resp.fee >= dec!(0)) {
        return Err(format!(
            "Close response on {adapter_venue} {expected_symbol} is not attributable/consistent: status={:?} qty={}/{} remaining={} venue={} client_order_id={} order_id={} fee={}",
            resp.status,
            resp.filled_qty,
            expected_qty,
            resp.remaining_qty,
            resp.venue,
            resp.client_order_id,
            resp.order_id,
            resp.fee
        ));
    }
    if resp.filled_qty > dec!(0) && resp.avg_fill_price.filter(|p| *p > dec!(0)).is_none() {
        return Err(format!(
            "Close response on {adapter_venue} {expected_symbol} reports filled quantity without a positive fill price"
        ));
    }
    Ok(())
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
    if !adapter.live_order_submission_implemented() {
        return Err(format!(
            "{venue_lc} does not support live close orders through the bot yet"
        ));
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
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let live = adapter
        .account_position_for(&account, &input.symbol)
        .map_err(|e| format!("Could not resolve {} on {venue_lc}: {e}", input.symbol))?
        .ok_or_else(|| {
            format!(
                "No live position in {} on {venue_lc} (already flat?)",
                input.symbol
            )
        })?;
    let base = canonical_base(&live.symbol);
    let qty = dec_f64(live.quantity);
    let long = live.signed_quantity() > dec!(0);
    let sign = if long { 1.0 } else { -1.0 };
    let ext = residual_qty(qty, system_qty_for(&signals, &venue_lc, &live.symbol, sign));
    if ext * dec_f64(live.mark_price) < DUST_NOTIONAL_USD {
        return Err(format!(
            "Nothing external left to close in {base} on {venue_lc}"
        ));
    }
    let qty_dec =
        f64_dec(ext).ok_or_else(|| "External quantity is not finite — refusing".to_string())?;
    let close_side = if long { Side::Sell } else { Side::Buy };
    let mut order = if mode == "limit" {
        let px = input
            .limit_price
            .filter(|p| p.is_finite() && *p > 0.0)
            .and_then(f64_dec)
            .ok_or_else(|| {
                "Limit close needs a positive limit_price (use the recommendation)".to_string()
            })?;
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
    order.client_order_id = format!(
        "extclose-{}-{}-{:08x}",
        venue_lc,
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let expected_client_order_id = order.client_order_id.clone();
    let expected_symbol = order.symbol.clone();
    let expected_qty = order.quantity;
    let sid = expected_client_order_id.clone();
    let leg_side = if long { LegSide::Short } else { LegSide::Long };

    // Persist the close intent before dispatch. If the process dies after the
    // venue accepts the order, the in-flight row keeps the symbol blocked and
    // preserves the exact leg identity for reconciliation.
    state
        .state_store
        .record_signal_execution(&NewSignalExecution {
            id: sid.clone(),
            strategy_id: "external".into(),
            symbol: live.symbol.clone(),
            route_id: None,
            expected_net_usd: None,
            note: Some(format!("External {base} close via bot ({mode})")),
            legs: vec![NewSignalLeg {
                account_id: None,
                venue: venue_lc.clone(),
                side: leg_side,
                symbol: Some(live.symbol.clone()),
                intended_qty: ext,
            }],
        })
        .map_err(|e| format!("Failed to persist external close intent: {e}"))?;

    let mark_unresolved = |note: &str| {
        let _ = state
            .state_store
            .update_signal_status(&sid, SignalStatus::Unresolved, Some(note));
    };

    let _guard = state.execution_lock.lock().await;
    let resp = match adapter.place_order(order).await {
        Ok(resp) => resp,
        Err(e) => {
            let note = format!(
                "External close submission returned an error after dispatch; verify venue state before retrying: {e}"
            );
            mark_unresolved(&note);
            return Err(format!("Close order failed on {venue_lc}: {e}"));
        }
    };

    if let Err(e) = validate_close_response(
        &adapter.venue_id(),
        &expected_symbol,
        &expected_client_order_id,
        expected_qty,
        &resp,
    ) {
        let note = format!(
            "External close response failed attribution/consistency checks; verify venue order {} before retrying: {e}",
            resp.order_id
        );
        mark_unresolved(&note);
        return Err(e);
    }

    if let Err(e) = state.state_store.record_leg_order_id(
        &sid,
        leg_side,
        &resp.order_id,
        Some(&expected_client_order_id),
    ) {
        let note = format!(
            "External close order {} was accepted but its order id could not be persisted: {e}",
            resp.order_id
        );
        mark_unresolved(&note);
        return Err(note);
    }

    let filled = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
    let avg = resp
        .avg_fill_price
        .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0));
    if filled > 0.0 {
        let fill_price = avg.unwrap_or(0.0).max(0.000_001);
        if let Err(e) = state.state_store.apply_leg_fill(
            &sid,
            leg_side,
            filled,
            fill_price,
            resp.fee.to_string().parse::<f64>().unwrap_or(0.0),
            Some(resp.order_id.as_str()),
        ) {
            let note = format!(
                "External close order {} filled but its fill could not be persisted: {e}",
                resp.order_id
            );
            mark_unresolved(&note);
            return Err(note);
        }
    }

    let fully_reported = resp.status == arbitrageur_core::types::OrderStatus::Filled
        && resp.filled_qty == expected_qty
        && resp.remaining_qty == dec!(0);
    let mut status = if fully_reported {
        "filled"
    } else if filled > 0.0 {
        "partial"
    } else if resp.status == arbitrageur_core::types::OrderStatus::New {
        "submitted"
    } else {
        "not_filled"
    };
    let mut note = format!(
        "Reduce-only {close_side:?} {ext} {base}: {}",
        match status {
            "filled" => "reported filled; confirming account state",
            "partial" => "partially filled — residual exposure remains",
            "submitted" => "accepted/working — confirm terminal state at the venue",
            "unverified" => "reported filled, but account confirmation is incomplete",
            _ => "terminated without a fill — residual exposure remains",
        }
    );

    if fully_reported {
        match adapter.fetch_account().await {
            Ok(acc) => {
                let residual = match adapter.account_position_for(&acc, &live.symbol) {
                    Ok(Some(p)) => residual_qty(
                        dec_f64(p.quantity),
                        system_qty_for(&signals, &venue_lc, &live.symbol, sign),
                    ),
                    Ok(None) => 0.0,
                    Err(e) => {
                        status = "unverified";
                        note = format!(
                            "External close reported filled, but account position resolution failed: {e} — verify venue state"
                        );
                        mark_unresolved(&note);
                        return Ok(CloseResultDto {
                            venue: venue_lc,
                            symbol: live.symbol.clone(),
                            mode,
                            order_id: Some(resp.order_id),
                            filled_qty: Some(filled),
                            avg_fill_price: avg,
                            status: status.into(),
                            note,
                        });
                    }
                };
                if residual * dec_f64(live.mark_price) <= DUST_NOTIONAL_USD {
                    let done =
                        format!("External close filled {filled} and account re-read is flat");
                    let _ = state.state_store.update_signal_status(
                        &sid,
                        SignalStatus::Closed,
                        Some(&done),
                    );
                    note = done;
                } else {
                    status = "unverified";
                    note = format!(
                        "External close reported filled, but account re-read still shows {residual} {base} residual — verify venue state"
                    );
                    mark_unresolved(&note);
                }
            }
            Err(e) => {
                status = "unverified";
                note = format!(
                    "External close reported filled, but account re-read failed: {e} — verify venue state"
                );
                mark_unresolved(&note);
            }
        }
    } else if status == "not_filled" {
        let _ = state
            .state_store
            .update_signal_status(&sid, SignalStatus::Failed, Some(&note));
    } else {
        mark_unresolved(&note);
    }

    Ok(CloseResultDto {
        venue: venue_lc,
        symbol: live.symbol.clone(),
        mode,
        order_id: Some(resp.order_id),
        filled_qty: Some(filled),
        avg_fill_price: avg,
        status: status.into(),
        note,
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
    fn venue_note_only_fires_on_hidden_rows() {
        assert!(venue_position_note("Hyperliquid", 0, 0, 0, 0, 0).is_none());
        assert!(venue_position_note("Hyperliquid", 2, 2, 0, 0, 0).is_none());
        let n = venue_position_note("Hyperliquid", 3, 1, 1, 1, 0).expect("note");
        assert!(n.contains("read 3 live position(s)"), "{n}");
        assert!(n.contains("1 shown externally"), "{n}");
        assert!(n.contains("1 fully system-owned"), "{n}");
        assert!(n.contains("1 dust under $1"), "{n}");
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

    #[test]
    fn close_response_validation_requires_attributable_consistent_ack() {
        use arbitrageur_core::types::OrderStatus;

        let venue = VenueId::new("hyperliquid");
        let mut resp = OrderResponse {
            order_id: "oid-1".into(),
            client_order_id: "client-1".into(),
            venue: venue.clone(),
            symbol: "BTC-USDC".into(),
            status: OrderStatus::New,
            original_qty: dec!(1.0),
            filled_qty: dec!(0),
            remaining_qty: dec!(1.0),
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USDC".into(),
            raw_response: None,
            timestamp: Utc::now(),
        };
        validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &resp)
            .expect("working ack");

        resp.status = OrderStatus::Filled;
        resp.filled_qty = dec!(1.0);
        resp.remaining_qty = dec!(0);
        resp.avg_fill_price = Some(dec!(65000.0));
        validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &resp)
            .expect("terminal fill");

        let mut wrong_client = resp.clone();
        wrong_client.client_order_id = "other".into();
        assert!(
            validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &wrong_client)
                .is_err()
        );

        let mut wrong_symbol = resp.clone();
        wrong_symbol.symbol = "ETH-USDC".into();
        assert!(
            validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &wrong_symbol)
                .is_err()
        );

        let mut inconsistent = resp.clone();
        inconsistent.remaining_qty = dec!(0.5);
        assert!(
            validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &inconsistent)
                .is_err()
        );

        let mut no_price = resp.clone();
        no_price.avg_fill_price = None;
        assert!(
            validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &no_price).is_err()
        );

        let mut impossible_working_fill = resp.clone();
        impossible_working_fill.status = OrderStatus::New;
        assert!(validate_close_response(
            &venue,
            "BTC-USDC",
            "client-1",
            dec!(1.0),
            &impossible_working_fill
        )
        .is_err());

        let mut partial = resp.clone();
        partial.status = OrderStatus::PartiallyFilled;
        partial.filled_qty = dec!(0.4);
        partial.remaining_qty = dec!(0.6);
        validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &partial)
            .expect("partial close remains attributable");

        for status in [
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ] {
            let mut terminal = resp.clone();
            terminal.status = status;
            terminal.filled_qty = dec!(0);
            terminal.remaining_qty = dec!(1.0);
            terminal.avg_fill_price = None;
            validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &terminal)
                .expect("terminal zero-fill close remains attributable");

            terminal.remaining_qty = dec!(0);
            assert!(
                validate_close_response(&venue, "BTC-USDC", "client-1", dec!(1.0), &terminal)
                    .is_err()
            );
        }
    }
}
