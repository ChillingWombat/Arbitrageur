//! Live order reconciliation.
//!
//! Every durable leg that carries a `venue_order_id` on a non-terminal
//! signal (`in_flight`, `imbalanced`, `unresolved`) is polled against the
//! venue's authoritative order state until it resolves. The loop is
//! fail-closed:
//!
//! - A leg only advances: `filled_qty`/`fee_usd` are cumulative maxima and
//!   `fill_price` is refreshed from the venue's reported average. A venue
//!   response that regresses the recorded fill is rejected by the store.
//! - Orders still working past `NON_FINAL_DEADLINE_SECS` escalate the signal
//!   to `unresolved` for operator review — never assumed cancelled, never
//!   assumed filled.
//! - Poll failures (transport, credentials, malformed payloads) keep the
//!   current state; only age escalation changes status, so a transient
//!   outage cannot flip a record to resolved.
//! - `unresolved` is not terminal here: later successful polls still update
//!   the leg and can resolve the signal, which is the restart-recovery path.
//!
//! The supervisor runs one pass immediately at startup (restart recovery)
//! and then every `POLL_INTERVAL_SECS`.

use crate::execution::{passive_cfg_for, FlattenLiveRefresh};
use crate::state::AppEngineState;
use crate::state_store::{
    LegSide, NewSignalExecution, NewSignalLeg, SignalExecution, SignalLeg, SignalStatus,
};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side};
use arbitrageur_execution::passive_unwind::execute_passive_reduce_live;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use std::collections::HashMap;
use std::sync::Arc;

/// Working orders older than this escalate their signal to `unresolved`.
/// Live flows only submit market/IOC/post-only closes and entries, so a
/// genuinely working order past 15 minutes indicates a venue anomaly.
const NON_FINAL_DEADLINE_SECS: i64 = 15 * 60;

/// Supervisor cadence.
const POLL_INTERVAL_SECS: u64 = 5;

/// Bounded cancel attempts for an order still working past the deadline —
/// a venue that rejects every cancel gets escalated, not spammed.
const MAX_CANCEL_ATTEMPTS: u32 = 3;

/// Maximum flatten children auto-dispatched for one imbalanced signal —
/// beyond this the residual is left for the operator.
const MAX_RESIDUAL_ATTEMPTS: usize = 3;
/// Minimum gap between residual dispatches — one attempt per poll pass
/// would spam a child row every 5s through a persistent venue error.
/// Never-submitted children are bounded by this throttle alone, not a
/// hard cap: once venue readiness returns the residual must always be
/// retryable.
const RESIDUAL_RETRY_SECS: i64 = 30;
/// A terminal flatten child whose close order stays working — or stays
/// unverifiable — past this age escalates the parent to `unresolved`:
/// suppression is the right answer for a live close, but an operator
/// must eventually hear about it instead of silence.
const RESIDUAL_ORDER_GRACE_SECS: i64 = 1800;

/// Match instrument symbol between venue wire response and persisted leg.
///
/// Compares case-insensitively, and falls back to comparing canonical base
/// tokens (e.g. `BTC/USD` vs `BTC-USD` vs `BTC`) to absorb venue-specific
/// punctuation and instrument-class packaging.
fn symbols_match(wire_symbol: &str, leg_symbol: &str) -> bool {
    wire_symbol.eq_ignore_ascii_case(leg_symbol)
        || arbitrageur_core::symbol::canonical_base(wire_symbol)
            == arbitrageur_core::symbol::canonical_base(leg_symbol)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileSummary {
    pub pending_legs: usize,
    pub polled: usize,
    pub advanced: usize,
    pub resolved: usize,
    pub escalated: usize,
    pub poll_errors: usize,
    pub skipped: usize,
    /// True when pending legs exist but cannot be polled because the
    /// operator locked the vault (adapter credentials wiped). Legs stay
    /// pending — never resolved — until re-unlock reprovisions adapters.
    pub vault_locked: bool,
}

/// Per-leg outcome of one poll, aggregated per signal for status derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegPoll {
    /// Venue reports the order still working (New / partially filled).
    Working,
    /// Terminal: filled completely.
    Filled,
    /// Terminal: ended with zero fill (cancelled / rejected / expired).
    CancelledEmpty,
    /// Terminal or provable state with a partial fill — never safe to
    /// auto-resolve a two-leg signal or a close around it.
    PartialTerminal,
    /// Poll failed or the venue cannot prove the state.
    Unknown,
}

fn signal_age_secs(created_at: &str) -> i64 {
    DateTime::parse_from_rfc3339(created_at)
        .map(|t| (Utc::now() - t.with_timezone(&Utc)).num_seconds())
        .unwrap_or(0)
}

/// Is `filled` materially below the order's intended quantity? Wire-grid
/// snapping can leave sub-1e-6 relative dust between intended and executed
/// sizes — below that epsilon the fill counts as complete; anything larger
/// is residual exposure the signal must keep surfacing.
fn materially_short(filled: Decimal, intended: Decimal) -> bool {
    intended > Decimal::ZERO
        && filled + (intended / dec!(1_000_000)).max(dec!(0.000000001)) < intended
}

fn is_quote_notional_future(resp: &OrderResponse) -> bool {
    let sym = resp.symbol.to_ascii_uppercase();
    resp.venue.as_str().eq_ignore_ascii_case("deribit")
        && !sym.ends_with("-C")
        && !sym.ends_with("-P")
}

/// Map an authoritative venue snapshot onto a poll outcome for ledger math.
/// `intended_qty` is the leg's planned quantity — a terminal response whose
/// fill does not cover it is partial exposure, never a full fill, whatever
/// the venue's status field claims.
fn classify_response(resp: &OrderResponse, intended_qty: Decimal) -> LegPoll {
    let quote_future_full = is_quote_notional_future(resp)
        && resp.remaining_qty <= Decimal::ZERO
        && resp.filled_qty > Decimal::ZERO
        && resp.original_qty > Decimal::ZERO
        && !materially_short(resp.filled_qty, resp.original_qty)
        && (intended_qty <= Decimal::ZERO
            || resp.filled_qty + (intended_qty * dec!(0.05)) >= intended_qty);
    match resp.status {
        OrderStatus::Filled => {
            // Venue-authoritative full execution — but a `Filled` status
            // reporting materially less than the intended size is a venue
            // accounting inconsistency: treat it as partial so the residual
            // stays on the escalation path instead of silently trusting it.
            // On quote-notional futures (Deribit USD contracts), `intended_qty`
            // is computed at pre-submit mark while `filled_qty` is `usd / fill_px`,
            // so a 100%-filled venue order within conversion drift is full.
            if materially_short(resp.filled_qty, intended_qty) && !quote_future_full {
                LegPoll::PartialTerminal
            } else {
                LegPoll::Filled
            }
        }
        OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired => {
            if resp.filled_qty <= Decimal::ZERO {
                LegPoll::CancelledEmpty
            } else if resp.remaining_qty <= Decimal::ZERO
                && (!materially_short(resp.filled_qty, intended_qty) || quote_future_full)
            {
                LegPoll::Filled
            } else {
                LegPoll::PartialTerminal
            }
        }
        OrderStatus::PartiallyFilled => {
            // Kraken returns PartiallyFilled for a provably-filled-but-
            // unprovably-terminal order — keep it working so the deadline
            // escalation decides, rather than guessing terminal.
            LegPoll::Working
        }
        OrderStatus::New => LegPoll::Working,
    }
}

/// Derives the durable signal status from this pass's per-leg outcomes.
/// Pure so the full classification matrix is unit-testable: every input
/// (poll set, leg count, age, close-vs-entry, cancel notes) is explicit.
/// `is_flatten_child` marks `flat-{parent}-*` / `trim-{parent}-*` residual
/// children: for them a venue-proven terminal outcome resolves the ATTEMPT
/// (Failed when nothing filled, Closed otherwise) so the residual machinery
/// can redispatch within its attempt bound — an `unresolved` child would
/// count as open forever and stall every retry.
fn derive_signal_status(
    polls: &[LegPoll],
    leg_count: usize,
    age: i64,
    is_close_signal: bool,
    is_flatten_child: bool,
    cancel_notes: Option<&Vec<String>>,
) -> (Option<SignalStatus>, Option<String>) {
    // Every persisted leg must be accounted for this pass — a leg missing a
    // venue order id can never be proven, so the signal stays on the
    // escalation path instead of resolving off partial evidence.
    let all_known = polls.len() == leg_count && polls.iter().all(|p| *p != LegPoll::Unknown);
    let any_working = polls.contains(&LegPoll::Working);
    if !all_known || any_working {
        // Still working or unproven: hold current state until the deadline,
        // then escalate — never assume terminal.
        if age > NON_FINAL_DEADLINE_SECS {
            let mut note = format!(
                "Order state not fully resolved within {}s; manual reconciliation required",
                NON_FINAL_DEADLINE_SECS
            );
            if let Some(cn) = cancel_notes {
                if !cn.is_empty() {
                    note.push_str("; cancel: ");
                    note.push_str(&cn.join(", "));
                }
            }
            return (Some(SignalStatus::Unresolved), Some(note));
        }
        return (None, None);
    }
    if polls.iter().all(|p| *p == LegPoll::Filled) {
        return (
            Some(if is_close_signal {
                SignalStatus::Closed
            } else {
                SignalStatus::Balanced
            }),
            Some("All legs reached terminal filled state via venue poll".into()),
        );
    }
    if polls
        .iter()
        .all(|p| matches!(*p, LegPoll::Filled | LegPoll::CancelledEmpty))
        && polls.contains(&LegPoll::CancelledEmpty)
    {
        // Terminal zero-fill legs. A close whose order ended empty may still
        // hold the position — never done. An entry where EVERY leg ended
        // empty executed nothing — Failed is honest and releases the ticker.
        // An entry where SOME leg filled while another ended empty holds
        // residual one-sided exposure — that is Imbalanced, never Failed
        // (which would unblock the ticker over a live residual).
        // A flatten/trim child whose orders all ended venue-proven empty is
        // a failed ATTEMPT: Failed releases it from the open-child dedup so
        // the residual dispatcher can retry — the parent keeps its open
        // status and still tracks the underlying exposure.
        let all_empty = polls.iter().all(|p| *p == LegPoll::CancelledEmpty);
        return (
            Some(if is_flatten_child {
                if all_empty {
                    SignalStatus::Failed
                } else {
                    SignalStatus::Closed
                }
            } else if is_close_signal {
                SignalStatus::Unresolved
            } else if all_empty {
                SignalStatus::Failed
            } else {
                SignalStatus::Imbalanced
            }),
            Some(if all_empty {
                "Order(s) ended with zero fill per venue".into()
            } else {
                "Some leg(s) filled while other(s) ended empty per venue".into()
            }),
        );
    }
    (
        Some(if is_flatten_child {
            // Terminal with some venue-proven fill — the attempt is over.
            // Its covered quantity feeds residual math, and the parent
            // redispatches for the remainder; `unresolved` would strand the
            // child as open forever and block every retry.
            SignalStatus::Closed
        } else {
            SignalStatus::Unresolved
        }),
        Some("Leg(s) ended partially filled or with mixed terminal states".into()),
    )
}

/// Recover a leg's venue order id through the venue's client-order-id
/// lookup — the crash-recovery path for orders whose client id was
/// persisted before dispatch but whose ack never landed. On success the
/// venue id is persisted so later passes poll it like any other leg.
/// `None` means the locator could not be verified — never assumed terminal.
async fn adopt_venue_order_id(
    state: &AppEngineState,
    leg: &crate::state_store::SignalLeg,
    client_order_id: &str,
) -> Option<String> {
    let adapter = state.get_adapter(&leg.venue)?;
    if !adapter.has_live_credentials() {
        return None;
    }
    let resp = adapter
        .fetch_order(&leg.symbol, "", Some(client_order_id))
        .await
        .ok()?;
    // The response must identify THIS leg's order — venue and symbol exact,
    // and when the venue echoes a client id it must be ours.
    let oid = resp.order_id.trim();
    if oid.is_empty()
        || !resp.venue.as_str().eq_ignore_ascii_case(&leg.venue)
        || !symbols_match(&resp.symbol, &leg.symbol)
        || (!resp.client_order_id.trim().is_empty()
            && resp.client_order_id.trim() != client_order_id)
    {
        tracing::error!(
            "reconcile: {} client-id lookup returned mismatched order (id={} venue={} symbol={} cid={}) vs leg (venue={} symbol={} cid={})",
            leg.venue,
            resp.order_id,
            resp.venue.as_str(),
            resp.symbol,
            resp.client_order_id,
            leg.venue,
            leg.symbol,
            client_order_id
        );
        return None;
    }
    if let Err(e) =
        state
            .state_store
            .record_leg_order_id(&leg.signal_id, leg.side, oid, Some(client_order_id))
    {
        tracing::warn!(
            "reconcile: client-id recovery found order {oid} on {} but persist failed: {e}",
            leg.venue
        );
        return None;
    }
    tracing::warn!(
        "reconcile: recovered {} order {oid} via persisted client id — ack was lost post-submit",
        leg.venue
    );
    Some(oid.to_string())
}

/// One reconciliation pass. Returns counts for the monitor feed.
pub async fn reconcile_pending_orders(state: &AppEngineState) -> ReconcileSummary {
    let _guard = match state.reconcile_lock.try_lock() {
        Ok(g) => g,
        Err(_) => return ReconcileSummary::default(), // previous pass still running
    };
    let mut summary = ReconcileSummary::default();
    // Environment-scoped: legs poll through the active environment's
    // adapters, so the other environment's order ids must never reach its
    // venue endpoints. `unknown`-env legacy rows stay in the set.
    let pending = match state
        .state_store
        .list_pending_order_legs(Some(state.env().as_str()))
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("reconcile: failed to list pending order legs: {e}");
            summary.poll_errors += 1;
            return summary;
        }
    };
    summary.pending_legs = pending.len();
    if pending.is_empty() {
        return summary;
    }
    let manual_locked = {
        let mut vault = state.vault.write();
        // Initialized + locked + not-auto = operator panic-halt. An
        // uninitialized vault isn't a lock — adapters simply have no
        // credentials and per-venue poll failures report as usual.
        vault.is_initialized() && !vault.is_unlocked() && !vault.is_auto_locked()
    };
    // Under manual lock this clears adapter credentials; under auto-lock it
    // leaves them provisioned so polling continues.
    state.sync_vault_credentials_to_adapters();
    if manual_locked {
        // Fail closed: venue truth is unreachable without credentials, so
        // legs stay pending rather than guessing at order state.
        summary.vault_locked = true;
        return summary;
    }

    // Aggregate per-signal outcomes for the status pass below.
    let mut outcomes: HashMap<String, Vec<LegPoll>> = HashMap::new();
    let mut signal_meta: HashMap<String, (String, usize, i64)> = HashMap::new();
    // Cancel attempts made this pass, keyed by signal — merged into the
    // escalation note so the operator sees what was tried.
    let mut cancel_notes: HashMap<String, Vec<String>> = HashMap::new();

    for (exec, leg) in &pending {
        let age = signal_age_secs(&exec.created_at);
        signal_meta
            .entry(exec.id.clone())
            .or_insert_with(|| (exec.strategy_id.clone(), exec.legs.len(), age));
        let legs_out = outcomes.entry(exec.id.clone()).or_default();

        // Resolve the order locator. `venue_order_id` is preferred; when it
        // was never recorded but a client order id was persisted
        // pre-dispatch (crash between venue accept and ack write), the
        // venue's client-id lookup recovers the venue id — adopted durably
        // so this and later passes poll the venue id like any other leg.
        let adopted_id: String;
        let order_id: &str = match leg.venue_order_id.as_deref().filter(|s| !s.is_empty()) {
            Some(oid) => oid,
            None => match leg.client_order_id.as_deref().filter(|s| !s.is_empty()) {
                Some(cid) => match adopt_venue_order_id(state, leg, cid).await {
                    Some(oid) => {
                        adopted_id = oid;
                        adopted_id.as_str()
                    }
                    // A persisted client id the venue cannot resolve is
                    // unproven, not terminal-empty — hold for the deadline
                    // escalation rather than guessing the order never lived.
                    None => {
                        if age > NON_FINAL_DEADLINE_SECS {
                            legs_out.push(LegPoll::CancelledEmpty);
                        } else {
                            legs_out.push(LegPoll::Unknown);
                        }
                        continue;
                    }
                },
                None => {
                    // Order identity is persisted at submission. On
                    // post-submission signals (imbalanced/unresolved — the
                    // only states whose no-id legs the query returns) a
                    // missing id means the venue rejected the order
                    // outright: terminally empty, not merely unverifiable.
                    // Counting it as terminal-empty lets a filled sibling
                    // surface as Imbalanced so residual auto-flatten covers
                    // the naked leg.
                    legs_out.push(LegPoll::CancelledEmpty);
                    continue;
                }
            },
        };
        // A leg already fully recorded needs no venue call.
        if leg.filled_qty >= leg.intended_qty && leg.filled_qty > 0.0 {
            legs_out.push(LegPoll::Filled);
            continue;
        }
        let Some(adapter) = state.get_adapter(&leg.venue) else {
            legs_out.push(LegPoll::Unknown);
            summary.skipped += 1;
            continue;
        };
        if !adapter.has_live_credentials() {
            legs_out.push(LegPoll::Unknown);
            summary.skipped += 1;
            continue;
        }
        match adapter
            .fetch_order(&leg.symbol, order_id, leg.client_order_id.as_deref())
            .await
        {
            Ok(resp) => {
                summary.polled += 1;
                // Response identity must match the persisted leg exactly —
                // venue, symbol, and order id — before any field is trusted.
                if resp.order_id != order_id
                    || !resp.venue.as_str().eq_ignore_ascii_case(&leg.venue)
                    || !symbols_match(&resp.symbol, &leg.symbol)
                {
                    tracing::error!(
                        "reconcile: {} fetch_order identity mismatch (id={} venue={} symbol={}) vs leg (order_id={} venue={} symbol={})",
                        leg.venue,
                        resp.order_id,
                        resp.venue.as_str(),
                        resp.symbol,
                        order_id,
                        leg.venue,
                        leg.symbol
                    );
                    legs_out.push(LegPoll::Unknown);
                    continue;
                }
                let filled = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                let fee = resp
                    .fee
                    .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                // `fee_usd` only holds USD-stable denominations; a fee in a
                // non-stable currency is unaccounted, not convertible here.
                let fee_usd_ok = fee <= 0.0
                    || matches!(
                        resp.fee_currency.to_ascii_uppercase().as_str(),
                        "USD" | "USDT" | "USDC"
                    );
                if !fee_usd_ok {
                    tracing::error!(
                        "reconcile: {} order {} reports fee {} {} — non-USD-stable currency cannot be recorded as fee_usd",
                        leg.venue,
                        order_id,
                        fee,
                        resp.fee_currency
                    );
                    legs_out.push(LegPoll::Unknown);
                    continue;
                }
                let price = resp
                    .avg_fill_price
                    .and_then(|p| p.to_string().parse::<f64>().ok())
                    .and_then(|p| {
                        crate::execution::ledger_px_usd(state, &leg.venue, &leg.symbol, p)
                    });
                if filled < leg.filled_qty || fee < leg.fee_usd {
                    // Venue reports less than the recorded fill — the store
                    // keeps the cumulative max, but a venue disagreeing with
                    // the ledger is worth an operator-visible warning.
                    tracing::warn!(
                        "reconcile: {} order {} reports filled {} fee {} below recorded {}/{} — keeping ledger values",
                        leg.venue,
                        order_id,
                        filled,
                        fee,
                        leg.filled_qty,
                        leg.fee_usd
                    );
                }
                if filled > leg.filled_qty || fee > leg.fee_usd || price.is_some() {
                    match state
                        .state_store
                        .apply_leg_order_state(&exec.id, leg.side, filled, price, fee)
                    {
                        Ok(()) => {
                            summary.advanced += 1;
                            // A late/grown fill on a `trim-{parent}-*` child:
                            // shrink the open lot for the new increment —
                            // delta-idempotent via the durable per-leg mark,
                            // so repeat polls recount nothing. No open lot →
                            // the unlotted family sweep covers it instead.
                            if let Some(parent_id) = exec
                                .id
                                .strip_prefix("trim-")
                                .and_then(|s| s.rsplit_once('-').map(|(p, _)| p.to_string()))
                            {
                                if let Ok(Some(parent)) = state.state_store.get_signal(&parent_id) {
                                    use crate::state_store::LegSide;
                                    let venue_of = |side: LegSide| {
                                        parent
                                            .legs
                                            .iter()
                                            .find(|l| l.side == side)
                                            .map(|l| l.venue.clone())
                                    };
                                    if let (Some(lv), Some(sv)) =
                                        (venue_of(LegSide::Long), venue_of(LegSide::Short))
                                    {
                                        let _ = crate::execution::realize_liq_trim(
                                            state, &parent, &exec.id, &lv, &sv,
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "reconcile: failed to persist order state for {} leg {}: {e}",
                                exec.id,
                                leg.id
                            );
                            summary.poll_errors += 1;
                        }
                    }
                }
                let intended = Decimal::from_f64_retain(leg.intended_qty).unwrap_or(Decimal::ZERO);
                let mut poll = classify_response(&resp, intended);
                // A venue-terminated order whose fill lands short of the
                // intended size may still be COMPLETE: the residue can sit
                // below the venue's minimum order increment (Deribit's $10
                // contract step), unfillable by construction. Proving the
                // shortfall is sub-wire keeps the signal on the filled path
                // instead of stranding it unresolved over dust it can never
                // trade. Default wire_base_qty is identity → the residue
                // stays actionable → PartialTerminal stands (fail closed).
                if poll == LegPoll::PartialTerminal && resp.remaining_qty <= Decimal::ZERO {
                    let shortfall = intended - resp.filled_qty;
                    if shortfall > Decimal::ZERO {
                        if let Ok(wire) = adapter.wire_base_qty(&leg.symbol, shortfall).await {
                            if wire <= Decimal::ZERO {
                                poll = LegPoll::Filled;
                            }
                        }
                    }
                }
                // Terminal legs can never retry — drop the cancel-attempt
                // counter so resolved orders don't accumulate kv rows.
                if poll != LegPoll::Working {
                    let key = format!("recon_cancel:{}:{order_id}", leg.venue);
                    let _ = state.state_store.kv_delete(&key);
                }
                // A still-working order past the deadline is anomalous —
                // try a bounded cancel so the next poll sees a terminal
                // state instead of hanging forever.
                if poll == LegPoll::Working && age > NON_FINAL_DEADLINE_SECS {
                    attempt_deadline_cancel(
                        state,
                        adapter.as_ref(),
                        leg,
                        order_id,
                        cancel_notes.entry(exec.id.clone()).or_default(),
                    )
                    .await;
                }
                legs_out.push(poll);
            }
            Err(e) => {
                summary.poll_errors += 1;
                tracing::warn!(
                    "reconcile: {} fetch_order {} for {} failed: {e}",
                    leg.venue,
                    order_id,
                    leg.symbol
                );
                legs_out.push(LegPoll::Unknown);
            }
        }
    }

    // Derive signal status from the aggregate leg outcomes. Persisted legs
    // not touched this pass contribute their recorded fill state.
    for (signal_id, polls) in &outcomes {
        let Some((strategy_id, leg_count, age)) = signal_meta.get(signal_id) else {
            continue;
        };
        // Close-type signals: `external` closes plus `flat-{parent}-*` /
        // `trim-{parent}-*` children — all exist to reduce exposure, so an
        // empty terminal leg is never a clean "Failed". Flatten children
        // resolve terminal outcomes against the ATTEMPT (retryable), while
        // `external` closes stay unresolved for the operator.
        let is_flatten_child = signal_id.starts_with("flat-") || signal_id.starts_with("trim-");
        let is_close_signal = strategy_id == "external" || is_flatten_child;
        // Terminal is terminal: this same pass may have just resolved a
        // `flat-{id}-*` child and closed the parent above — a stale leg
        // poll must never reopen Closed/Failed (HashMap outcome order is
        // arbitrary). Unresolved/imbalanced stay revisable by design.
        // Evaluated before derive so the terminal guard also covers signals
        // whose pending legs are all-Working this pass (derive → None).
        let current = state.state_store.get_signal(signal_id).ok().flatten();
        let already_terminal = current
            .as_ref()
            .map(|s| matches!(s.status, SignalStatus::Closed | SignalStatus::Failed))
            .unwrap_or(false);
        let all_filled = polls.len() == *leg_count && polls.iter().all(|p| *p == LegPoll::Filled);
        if already_terminal
            && !(current.as_ref().map(|s| s.status) == Some(SignalStatus::Failed) && all_filled)
        {
            // A terminal flat/trim child can still have landed its fill this
            // pass (leg state persists before status derivation) — re-check
            // parent coverage so a failed close whose order filled late
            // still resolves the parent. Idempotent: the parent already
            // closed or uncovered → early return.
            if let Some(parent_id) = signal_id
                .strip_prefix("flat-")
                .or_else(|| signal_id.strip_prefix("trim-"))
                .and_then(|s| s.rsplit_once('-').map(|(p, _)| p.to_string()))
            {
                resolve_flattened_parent(state, &parent_id);
            }
            continue;
        }
        let (mut new_status, mut note): (Option<SignalStatus>, Option<String>) =
            derive_signal_status(
                polls,
                *leg_count,
                *age,
                is_close_signal,
                is_flatten_child,
                cancel_notes.get(signal_id),
            );
        // A 1-leg entry signal (options parity/package or triangle leg) that
        // is already marked `Imbalanced` was flagged by its multi-leg cycle
        // executor because a sibling leg failed. Keep it `Imbalanced` so it
        // never masquerades as a balanced trade and residual auto-flatten can
        // unwind it.
        if *leg_count == 1
            && !is_close_signal
            && current.as_ref().map(|s| s.status) == Some(SignalStatus::Imbalanced)
            && new_status == Some(SignalStatus::Balanced)
        {
            new_status = Some(SignalStatus::Imbalanced);
            note = current.as_ref().and_then(|s| s.note.clone());
        }
        if let Some(status) = new_status {
            // Idempotent escalation: rewriting identical status+note every
            // pass bumps `updated_at` and re-counts the escalation forever —
            // persist only real changes. The dispatch/resolve hooks still
            // run on a steady-state outcome: an Imbalanced parent must keep
            // retrying residual dispatch within its attempt bounds even
            // when the status word itself is unchanged.
            let changed = current
                .as_ref()
                .map(|s| s.status != status || s.note.as_deref() != note.as_deref())
                .unwrap_or(true);
            if changed
                && state
                    .state_store
                    .update_signal_status(signal_id, status, note.as_deref())
                    .is_ok()
            {
                if status == SignalStatus::Unresolved {
                    summary.escalated += 1;
                    // Out-of-band page: `unresolved` means the supervisor
                    // could not prove the order's terminal state — a human
                    // must look. Fires only on the transition (the persist
                    // above is change-gated), never repeatedly per pass.
                    crate::alerting::dispatch(
                        state.env().as_str(),
                        "signal_unresolved",
                        format!(
                            "{signal_id}: {}",
                            note.as_deref().unwrap_or("escalated to unresolved")
                        ),
                    );
                } else {
                    summary.resolved += 1;
                }
            }
            // Auto-compensation (unconditional): an ENTRY signal that
            // lands imbalanced holds naked one-sided exposure — submit
            // reduce-only flattens for the residual, persisted as
            // `flat-{id}-*` children this same supervisor reconciles to
            // terminal state. Exit manner follows the resolved passive
            // unwind policy (limit-first with market fallback).
            if status == SignalStatus::Imbalanced && !is_close_signal {
                dispatch_residual_flatten(state, signal_id).await;
            }
            // A `flat-{parent}-*` or `trim-{parent}-*` child that just
            // resolved means its fills are final. When ALL of a parent's
            // family children are Closed, the parent can leave
            // `unresolved`/`imbalanced` — never resurrect a parent that
            // already reached closed/failed. The trim prefix matters: a
            // trim child resolving last would otherwise never retrigger
            // the check, stranding the parent open forever.
            if status == SignalStatus::Closed {
                if let Some(parent_id) = signal_id
                    .strip_prefix("flat-")
                    .or_else(|| signal_id.strip_prefix("trim-"))
                    .and_then(|s| s.rsplit_once('-').map(|(p, _)| p.to_string()))
                {
                    resolve_flattened_parent(state, &parent_id);
                }
            }
        }
    }

    // Sweep open parent signals that have flatten/trim children so late fills
    // or sequencer-settled children close covered parents without requiring
    // an active child order in the poll set.
    if let Ok(open_sigs) = state
        .state_store
        .list_open_signals(Some(state.env().as_str()))
    {
        for sig in open_sigs {
            if !sig.id.starts_with("flat-") && !sig.id.starts_with("trim-") {
                resolve_flattened_parent(state, &sig.id);
            }
        }
    }

    // Fee enrichment: fills that landed inside the submit acknowledgement
    // recorded `fee_usd = 0` because venue submit responses never report
    // fees — realized P&L then overstates the round trip by the actual
    // taker fee. Poll each unpriced filled leg exactly once per process
    // (bounded by `fee_enrich_done`); a venue reporting 0 is truthful.
    if let Ok(unpriced) = state.state_store.list_fee_unaccounted_legs(7 * 24 * 3600) {
        // Pace fee enrichment across reconciliation cycles (max 5 legs per pass)
        // so startup sweeps don't burst venue rate limiters.
        for (exec, leg) in unpriced.into_iter().take(5) {
            let key = format!("{}:{}", exec.id, leg.side.as_str());
            {
                let mut done = state.fee_enrich_done.lock();
                if !done.insert(key) {
                    continue;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
            let Some(order_id) = leg
                .venue_order_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(adapter) = state.get_adapter(&leg.venue) else {
                continue;
            };
            if !adapter.has_live_credentials() {
                continue;
            }
            let Ok(resp) = adapter
                .fetch_order(&leg.symbol, order_id, leg.client_order_id.as_deref())
                .await
            else {
                continue;
            };
            if resp.order_id != order_id
                || !resp.venue.as_str().eq_ignore_ascii_case(&leg.venue)
                || !symbols_match(&resp.symbol, &leg.symbol)
            {
                tracing::warn!(
                    "reconcile fee-enrich: {} order {} identity mismatch — skipping",
                    leg.venue,
                    order_id
                );
                continue;
            }
            let fee = resp
                .fee
                .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            if fee <= 0.0 {
                continue;
            }
            if !matches!(
                resp.fee_currency.to_ascii_uppercase().as_str(),
                "USD" | "USDT" | "USDC"
            ) {
                tracing::warn!(
                    "reconcile fee-enrich: {} order {} reports fee {} {} — non-USD-stable currency unaccounted",
                    leg.venue,
                    order_id,
                    fee,
                    resp.fee_currency
                );
                continue;
            }
            let filled = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
            let price = resp
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .and_then(|p| crate::execution::ledger_px_usd(state, &leg.venue, &leg.symbol, p));
            if let Err(e) = state
                .state_store
                .apply_leg_order_state(&exec.id, leg.side, filled, price, fee)
            {
                tracing::warn!(
                    "reconcile fee-enrich: persist {} leg {} failed: {e}",
                    exec.id,
                    leg.id
                );
            }
        }
    }
    summary
}

/// If every `flat-{parent}-*` child signal reached `closed`, the parent's
/// residual exposure is covered — mark it `closed`. A parent already
/// `closed`/`failed` is never reopened.
fn resolve_flattened_parent(state: &AppEngineState, parent_id: &str) {
    let Ok(parent) = state.state_store.get_signal(parent_id) else {
        return;
    };
    let Some(parent) = parent else { return };
    if matches!(parent.status, SignalStatus::Closed | SignalStatus::Failed) {
        return;
    }
    let Ok(signals) = state.state_store.list_signals(None, None) else {
        return;
    };
    let children: Vec<SignalExecution> = signals
        .into_iter()
        .filter(|s| crate::state_store::is_flatten_child_id(&s.id, parent_id))
        .collect();

    // The parent closes when the residual is provably covered: no child
    // still open and no uncovered residual quantity. A child that failed
    // before submitting covers nothing — and blocks nothing.
    if children.is_empty()
        || children.iter().any(|c| c.status.counts_as_open())
        || !residual_leg_quantities(&parent, &children).is_empty()
    {
        return;
    }
    if let Err(e) = state.state_store.update_signal_status(
        parent_id,
        SignalStatus::Closed,
        Some("Residual exposure covered — flatten children resolved"),
    ) {
        tracing::warn!("reconcile: failed to close flattened parent {parent_id}: {e}");
        return;
    }
    tracing::info!("reconcile: parent {parent_id} closed — all flatten children resolved");
    // Realize the family's close fills — this path bypasses
    // `flatten_ticker_positions_impl`, so without it the residual unwind's
    // PnL and the entry's prediction resolution would drop entirely. A
    // matching open lot closes through the cycle path (auto-compensation
    // unwinds are adverse → StoppedOut); lotless/dead-lot families sweep
    // per-leg with durable marks.
    use arbitrageur_execution::virtual_ledger::CycleStatus;
    if let Some((usd, _, fee_basis)) = crate::execution::close_flattened_spatial_lot(
        state,
        &parent,
        &children,
        CycleStatus::StoppedOut,
    ) {
        let marked_fees = crate::execution::marked_family_fees(state, &children);
        crate::execution::accumulate_realized_profit(
            state,
            state.is_live(),
            usd + fee_basis + marked_fees,
        );
    } else {
        crate::execution::accumulate_unlotted_family_fills(state, &parent, &children, true);
    }
}

/// Bounded cancel for a stale working order, persisted in the kv store so
/// retries survive restart. The cancel result is observed on the NEXT poll —
/// this pass only records the attempt for the escalation note.
async fn attempt_deadline_cancel(
    state: &AppEngineState,
    adapter: &dyn ExchangeAdapter,
    leg: &SignalLeg,
    order_id: &str,
    notes: &mut Vec<String>,
) {
    let key = format!("recon_cancel:{}:{order_id}", leg.venue);
    let attempts = state
        .state_store
        .kv_get(&key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if attempts >= MAX_CANCEL_ATTEMPTS {
        notes.push("cancel retry budget exhausted".to_string());
        return;
    }
    let _ = state.state_store.kv_set(&key, &(attempts + 1).to_string());
    let req = CancelRequest {
        venue: adapter.venue_id(),
        symbol: leg.symbol.clone(),
        order_id: Some(order_id.to_string()),
        client_order_id: leg.client_order_id.clone(),
    };
    match adapter.cancel_order(req).await {
        Ok(()) => notes.push(format!(
            "cancel attempt {} accepted on {}",
            attempts + 1,
            leg.venue
        )),
        Err(e) => notes.push(format!(
            "cancel attempt {} on {} failed: {e}",
            attempts + 1,
            leg.venue
        )),
    }
}

/// Residual exposure per leg of `exec`: recorded fills not yet covered by
/// flatten-child fills on the same venue in the closing direction. Pure —
/// unit-tested without adapters.
fn residual_leg_quantities(
    exec: &SignalExecution,
    children: &[SignalExecution],
) -> Vec<(SignalLeg, f64)> {
    exec.legs
        .iter()
        .filter(|l| l.filled_qty > 0.0)
        .filter_map(|l| {
            let close_side = match l.side {
                LegSide::Long => LegSide::Short,
                LegSide::Short => LegSide::Long,
            };
            let covered: f64 = children
                .iter()
                .flat_map(|c| c.legs.iter())
                .filter(|cl| cl.side == close_side && cl.venue.eq_ignore_ascii_case(&l.venue))
                .map(|cl| cl.filled_qty)
                .sum();
            let residual = l.filled_qty - covered;
            (residual > 1e-9).then(|| (l.clone(), residual))
        })
        .collect()
}

/// Clamp a ledger-claimed residual to the exposure the venue actually
/// reports. Returns `Ok(None)` when the venue shows none — the residual is
/// phantom and submitting would create or destroy unrelated exposure.
/// `Err` on ambiguous venue mapping → caller fails closed.
fn observed_residual_qty(
    adapter: &Arc<dyn ExchangeAdapter>,
    acc: &arbitrageur_core::position::AccountSnapshot,
    leg: &SignalLeg,
    ledger_qty: f64,
) -> anyhow::Result<Option<f64>> {
    match adapter.account_position_for(acc, &leg.symbol)? {
        Some(pos) => {
            // A derivative position exists — its side must agree with the
            // residual direction. Opposite-side means the ledger and venue
            // disagree structurally; refusing beats guessing.
            let expected = match leg.side {
                LegSide::Long => Side::Buy,
                LegSide::Short => Side::Sell,
            };
            if pos.side != expected {
                anyhow::bail!(
                    "{} residual on {}: venue position is {:?}, ledger expects {:?}",
                    leg.symbol,
                    leg.venue,
                    pos.side,
                    leg.side
                );
            }
            let clamp = ledger_qty.min(pos.quantity.to_f64().unwrap_or(f64::MAX));
            Ok((clamp > 1e-9).then_some(clamp))
        }
        None => match leg.side {
            // Spot long residual lives in balances, not positions: a
            // sell-flatten is bounded by the free balance of the base asset.
            LegSide::Long => {
                let base = arbitrageur_core::symbol::canonical_base(&leg.symbol);
                let asset = base.strip_suffix("-SPOT").unwrap_or(&base);
                match acc.get_balance(asset) {
                    Some(b) if b.free > dec!(0) => {
                        let clamp = ledger_qty.min(b.free.to_f64().unwrap_or(f64::MAX));
                        Ok((clamp > 1e-9).then_some(clamp))
                    }
                    _ => Ok(None),
                }
            }
            // A short residual with no derivative position means a spot sell
            // already settled — the debt owed back isn't observable from
            // balances. The ledger quantity came from venue-confirmed fills,
            // so it stays the bound; the buy-back cannot exceed it.
            LegSide::Short => Ok(Some(ledger_qty)),
        },
    }
}

/// Auto-compensation for an `imbalanced` entry: persist a `flat-{id}-*`
/// child signal (durable intent BEFORE any submission), then submit
/// reduce-only market orders for each residual leg. The child joins the
/// pending poll set on the next pass — an accepted-but-working flatten is
/// reconciled like every other live order. Dedup: skips while any child is
/// still open; bounded by `MAX_RESIDUAL_ATTEMPTS` total children.
async fn dispatch_residual_flatten(state: &AppEngineState, signal_id: &str) {
    // Serialize against the manual flatten path: the dedup check and the
    // child-signal insert must be atomic with any other dispatcher touching
    // this family, or a manual flatten mid-pass could double-submit the
    // residual.
    let _exec_guard = state.execution_lock.lock().await;
    let exec = match state.state_store.get_signal(signal_id) {
        Ok(Some(e)) => e,
        _ => return,
    };
    let children: Vec<SignalExecution> = state
        .state_store
        .list_signals(None, None)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| crate::state_store::is_flatten_child_id(&s.id, signal_id))
        .collect();
    // The newest child's age throttles redispatch cadence — no cap on
    // total children: a vault lock or venue outage must never permanently
    // bar the residual from flattening once readiness returns.
    let newest_child_age_secs = children
        .iter()
        .filter_map(|c| chrono::DateTime::parse_from_rfc3339(&c.created_at).ok())
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds())
        .min();
    // A child whose dispatch produced no order ids and no fills never
    // reached a venue — it is failed, not "open": it must neither block
    // re-dispatch nor consume an attempt slot.
    let mut kept: Vec<SignalExecution> = Vec::with_capacity(children.len());
    for c in children {
        let submitted = c.legs.iter().any(|l| {
            l.venue_order_id
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
                || l.filled_qty > 0.0
        });
        if !submitted {
            if c.status.counts_as_open() {
                let _ = state.state_store.update_signal_status(
                    &c.id,
                    SignalStatus::Failed,
                    Some("residual flatten never dispatched an order"),
                );
            }
            continue;
        }
        kept.push(c);
    }
    let children = kept;
    // A still-open submitted child is already working the residual —
    // never double-submit.
    if children.iter().any(|c| c.status.counts_as_open()) {
        return;
    }
    // Venue truth before another dispatch: a terminal (`failed`/`closed`)
    // child's close order may still be live even though the ledger wrote it
    // off — a cancel-ack that raced a fill, or an ack that never arrived.
    // The ledger sees zero covered qty for it, so without this check the
    // dispatcher would stack a second close on top of a working one. Each
    // uncertain leg goes through `fetch_order`:
    //   Working        → suppress this dispatch (the order IS the close).
    //   Filled/partial → book the late fill; coverage may dissolve the
    //                    residual without any new order.
    //   Provably dead  → fall through; redispatch is safe.
    //   Unverifiable   → suppress this pass; aged ambiguity escalates the
    //                    parent instead of stacking on guesswork.
    for c in &children {
        if c.status.counts_as_open() {
            continue;
        }
        let child_age_secs = chrono::DateTime::parse_from_rfc3339(&c.created_at)
            .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds())
            .unwrap_or(i64::MAX);
        for leg in &c.legs {
            let order_id = leg
                .venue_order_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if (order_id.is_none() && leg.client_order_id.is_none())
                || leg.filled_qty + 1e-9 >= leg.intended_qty
            {
                continue;
            }
            let Some(adapter) = state.get_adapter(&leg.venue) else {
                tracing::warn!(
                    "reconcile: {signal_id} flatten child {} leg on {} unverifiable (no adapter) — suppressing dispatch",
                    c.id,
                    leg.venue
                );
                if child_age_secs > RESIDUAL_ORDER_GRACE_SECS {
                    let _ = state.state_store.update_signal_status(
                        signal_id,
                        SignalStatus::Unresolved,
                        Some("flatten child's close order unverifiable — adapter gone"),
                    );
                }
                return;
            };
            if !adapter.has_live_credentials() {
                tracing::warn!(
                    "reconcile: {signal_id} flatten child {} leg on {} unverifiable (credentials) — suppressing dispatch",
                    c.id,
                    leg.venue
                );
                if child_age_secs > RESIDUAL_ORDER_GRACE_SECS {
                    let _ = state.state_store.update_signal_status(
                        signal_id,
                        SignalStatus::Unresolved,
                        Some("flatten child's close order unverifiable — credentials missing"),
                    );
                }
                return;
            }
            let resp = adapter
                .fetch_order(
                    &leg.symbol,
                    order_id.unwrap_or(""),
                    leg.client_order_id.as_deref(),
                )
                .await;
            match resp {
                Ok(resp)
                    if (order_id.is_none() && !resp.order_id.is_empty()
                        || order_id == Some(resp.order_id.as_str()))
                        && resp.venue.as_str().eq_ignore_ascii_case(&leg.venue)
                        && symbols_match(&resp.symbol, &leg.symbol) =>
                {
                    if order_id.is_none() {
                        // Crash-window orphan: only the client id was ever
                        // persisted — adopt the venue order id now so the
                        // poll seam can keep watching it directly.
                        let _ = state.state_store.record_leg_order_id(
                            &c.id,
                            leg.side,
                            &resp.order_id,
                            leg.client_order_id.as_deref(),
                        );
                    }
                    match classify_response(
                        &resp,
                        Decimal::from_f64_retain(leg.intended_qty).unwrap_or_default(),
                    ) {
                        LegPoll::Working => {
                            tracing::warn!(
                                "reconcile: {signal_id} flatten child {} close order still working on {} — suppressing residual dispatch",
                                c.id,
                                leg.venue
                            );
                            if child_age_secs > RESIDUAL_ORDER_GRACE_SECS {
                                let _ = state.state_store.update_signal_status(
                                    signal_id,
                                    SignalStatus::Unresolved,
                                    Some("flatten child's close order still working past grace — manual check required"),
                                );
                            }
                            return;
                        }
                        LegPoll::Filled | LegPoll::PartialTerminal => {
                            let filled = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                            let fee = resp
                                .fee
                                .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                                .unwrap_or(0.0);
                            let price = resp
                                .avg_fill_price
                                .and_then(|p| p.to_string().parse::<f64>().ok())
                                .and_then(|p| {
                                    crate::execution::ledger_px_usd(
                                        state,
                                        &leg.venue,
                                        &leg.symbol,
                                        p,
                                    )
                                });
                            if filled > leg.filled_qty || fee > leg.fee_usd {
                                let _ = state
                                    .state_store
                                    .apply_leg_order_state(&c.id, leg.side, filled, price, fee);
                            }
                        }
                        LegPoll::CancelledEmpty | LegPoll::Unknown => {}
                    }
                }
                Ok(_) => {
                    // Identity mismatch — venue answered for a different
                    // order than the ledger recorded. Unverifiable.
                    tracing::error!(
                        "reconcile: {signal_id} flatten child {} order on {} returned mismatched identity — suppressing dispatch",
                        c.id,
                        leg.venue
                    );
                    if child_age_secs > RESIDUAL_ORDER_GRACE_SECS {
                        let _ = state.state_store.update_signal_status(
                            signal_id,
                            SignalStatus::Unresolved,
                            Some("flatten child's close order identity unverifiable"),
                        );
                    }
                    return;
                }
                Err(e) => {
                    tracing::warn!(
                        "reconcile: {signal_id} flatten child {} order check on {} failed ({e}) — suppressing dispatch",
                        c.id,
                        leg.venue
                    );
                    if child_age_secs > RESIDUAL_ORDER_GRACE_SECS {
                        let _ = state.state_store.update_signal_status(
                            signal_id,
                            SignalStatus::Unresolved,
                            Some("flatten child's close order unverifiable — venue read failing"),
                        );
                    }
                    return;
                }
            }
        }
    }
    // The attempt cap bounds residual-flatten retries only — `trim-`
    // children count toward coverage and the open-dedup above, but a prior
    // liquidation trim is not a failed residual attempt and must not consume
    // the budget.
    let residual_attempts = children
        .iter()
        .filter(|c| c.id.starts_with("flat-"))
        .count();
    if residual_attempts >= MAX_RESIDUAL_ATTEMPTS {
        tracing::error!(
            "reconcile: {signal_id} residual persists after {MAX_RESIDUAL_ATTEMPTS} submitted flatten attempts — manual resolution required"
        );
        let _ = state.state_store.update_signal_status(
            signal_id,
            SignalStatus::Unresolved,
            Some("residual exposure persists after bounded auto-flatten attempts"),
        );
        return;
    }
    if let Some(age) = newest_child_age_secs {
        if age < RESIDUAL_RETRY_SECS {
            return;
        }
    }
    let residuals = residual_leg_quantities(&exec, &children);
    if residuals.is_empty() {
        return;
    }

    // Pre-flight: every residual leg must be dispatchable BEFORE the child
    // is persisted — a vault lock or missing adapter must not create a
    // child that can never submit. Retried next pass when readiness returns.
    for (leg, _) in &residuals {
        let ready = !leg.symbol.is_empty()
            && state
                .get_adapter(&leg.venue)
                .map(|a| a.live_order_submission_implemented() && a.has_live_credentials())
                .unwrap_or(false);
        if !ready {
            tracing::warn!(
                "reconcile: {signal_id} residual on {} not dispatchable (symbol/submission/credentials) — retrying next pass",
                leg.venue
            );
            return;
        }
    }

    // Venue-side verification: the ledger residual is a claim, the venue's
    // account state is the truth. Each leg is clamped to observed exposure
    // and legs the venue reports flat are dropped — a stale or overstated
    // ledger must never send a real order against unrelated balances or
    // create fresh exposure. Fetch/mapping failures abort the pass without
    // consuming an attempt (retried next poll); a verified-empty residual
    // set closes the signal as a phantom.
    let mut accounts: HashMap<String, arbitrageur_core::position::AccountSnapshot> = HashMap::new();
    let mut verified: Vec<(SignalLeg, f64)> = Vec::with_capacity(residuals.len());
    for (leg, ledger_qty) in residuals {
        let adapter = match state.get_adapter(&leg.venue) {
            Some(a) => a,
            None => return, // pre-flighted above; missing now = config race
        };
        if !accounts.contains_key(&leg.venue) {
            match adapter.fetch_account().await {
                Ok(acc) => {
                    accounts.insert(leg.venue.clone(), acc);
                }
                Err(e) => {
                    tracing::warn!(
                        "reconcile: {signal_id} residual verification fetch on {} failed ({e}) — retrying next pass",
                        leg.venue
                    );
                    return;
                }
            }
        }
        match observed_residual_qty(&adapter, &accounts[&leg.venue], &leg, ledger_qty) {
            Ok(Some(clamped)) => verified.push((leg, clamped)),
            Ok(None) => {
                tracing::warn!(
                    "reconcile: {signal_id} {} residual on {} is phantom — venue shows no exposure; not submitting",
                    leg.symbol,
                    leg.venue
                );
            }
            Err(e) => {
                tracing::error!(
                    "reconcile: {signal_id} residual verification on {} failed: {e}",
                    leg.venue
                );
                let _ = state.state_store.update_signal_status(
                    signal_id,
                    SignalStatus::Unresolved,
                    Some("residual venue verification ambiguous — manual check required"),
                );
                return;
            }
        }
    }
    if verified.is_empty() {
        let _ = state.state_store.update_signal_status(
            signal_id,
            SignalStatus::Closed,
            Some("residual verified flat at venue — ledger overstated"),
        );
        return;
    }

    let close_id = format!("flat-{}-{:04x}", signal_id, rand::random::<u16>());
    let legs: Vec<NewSignalLeg> = verified
        .iter()
        .map(|(l, qty)| NewSignalLeg {
            account_id: l.account_id.clone(),
            venue: l.venue.clone(),
            side: match l.side {
                LegSide::Long => LegSide::Short,
                LegSide::Short => LegSide::Long,
            },
            symbol: (!l.symbol.is_empty()).then(|| l.symbol.clone()),
            intended_qty: *qty,
            intended_price: None,
            unit_scale: None,
        })
        .collect();
    if let Err(e) = state.state_store.record_signal_execution(
        &NewSignalExecution {
            id: close_id.clone(),
            strategy_id: exec.strategy_id.clone(),
            symbol: exec.symbol.clone(),
            route_id: exec.route_id.clone(),
            expected_net_usd: None,
            note: Some(format!("auto residual flatten of {signal_id}")),
            legs,
        },
        state.env().as_str(),
    ) {
        tracing::error!("reconcile: failed to persist residual flatten {close_id}: {e}");
        return;
    }
    tracing::warn!("reconcile: {signal_id} imbalanced — dispatching residual flatten {close_id}");

    // Residual flattens run the resolved passive-unwind policy (limit-peg
    // first, market fallback when the calm gates trip — too volatile /
    // too wide — or tries exhaust). A strategy whose mode is `off` keeps
    // the straight market path.
    let passive_cfg = passive_cfg_for(state, &exec.strategy_id);
    // The first dispatch failure is persisted into the child's note below —
    // "never dispatched" alone is undiagnosable from durable state once the
    // process log has rotated.
    let mut first_dispatch_err: Option<String> = None;
    let mut note_err = |e: String| {
        if first_dispatch_err.is_none() {
            first_dispatch_err = Some(e);
        }
    };
    for (leg, qty) in &verified {
        let child_side = match leg.side {
            LegSide::Long => LegSide::Short,
            LegSide::Short => LegSide::Long,
        };
        if leg.symbol.is_empty() {
            let e = format!("residual leg on {} carries no symbol", leg.venue);
            tracing::error!("reconcile: {e} — cannot flatten");
            note_err(e);
            continue;
        }
        let Some(adapter) = state.get_adapter(&leg.venue) else {
            let e = format!("no adapter admitted for residual leg on {}", leg.venue);
            tracing::error!("reconcile: {e}");
            note_err(e);
            continue;
        };
        if !adapter.live_order_submission_implemented() || !adapter.has_live_credentials() {
            let e = format!("{} not live-submission ready", leg.venue);
            tracing::error!("reconcile: {e} — residual leg not flattened");
            note_err(e);
            continue;
        }
        // Residuals are f64 arithmetic — from_f64_retain preserves binary
        // noise (0.005 → 0.0050000000000000001040834086) that venue wire
        // scales reject. Shortest round-trip + an 8dp bound (finer than any
        // admitted venue's size step) keeps the quantity representable.
        let Some(qty_dec) = Decimal::from_f64(*qty)
            .map(|d| d.round_dp(8).normalize())
            .filter(|q| *q > dec!(0))
        else {
            continue;
        };
        let order_side = match leg.side {
            LegSide::Long => Side::Sell,
            LegSide::Short => Side::Buy,
        };
        let (resp, client_order_id) = match &passive_cfg {
            Some(cfg) => {
                let refresher = FlattenLiveRefresh {
                    state,
                    venue_id: adapter.venue_id(),
                    symbol: leg.symbol.clone(),
                    adapter: adapter.clone(),
                };
                match execute_passive_reduce_live(
                    &adapter,
                    &refresher,
                    &leg.symbol,
                    order_side,
                    qty_dec,
                    cfg,
                    None,
                )
                .await
                {
                    Ok(report) => {
                        tracing::info!(
                            "reconcile: residual flatten {close_id} on {} via {:?} ({} limit tries)",
                            leg.venue,
                            report.outcome,
                            report.limit_tries
                        );
                        let cid = report.response.client_order_id.clone();
                        (report.response, cid)
                    }
                    Err(e) => {
                        tracing::error!(
                            "reconcile: passive residual flatten on {} failed: {e:#}",
                            leg.venue
                        );
                        note_err(format!("passive flatten on {} failed: {e:#}", leg.venue));
                        continue;
                    }
                }
            }
            None => {
                let mut req = OrderRequest::new_market(
                    adapter.venue_id(),
                    leg.symbol.clone(),
                    order_side,
                    qty_dec,
                );
                // Spot instruments carry no reduce-only flag — the venue
                // refuses an oversell itself, same guarantee.
                req.reduce_only = adapter.supports_reduce_only(&leg.symbol);
                let client_order_id = req.client_order_id.clone();
                // Persist the client order id BEFORE dispatch: a crash
                // between venue accept and ack write keeps the locator.
                if let Err(e) = state.state_store.record_leg_client_order_id(
                    &close_id,
                    child_side,
                    &client_order_id,
                ) {
                    tracing::error!(
                        "reconcile: residual flatten {close_id} client id persist failed — dispatch refused: {e}"
                    );
                    note_err(format!("client id persist failed: {e}"));
                    continue;
                }
                match adapter.place_order(req).await {
                    Ok(resp) => (resp, client_order_id),
                    Err(e) => {
                        tracing::error!(
                            "reconcile: residual flatten order on {} rejected: {e}",
                            leg.venue
                        );
                        note_err(format!("flatten order on {} rejected: {e}", leg.venue));
                        continue;
                    }
                }
            }
        };
        if !resp.order_id.trim().is_empty() {
            let cid = (!client_order_id.trim().is_empty()).then_some(client_order_id.as_str());
            if let Err(e) =
                state
                    .state_store
                    .record_leg_order_id(&close_id, child_side, &resp.order_id, cid)
            {
                tracing::error!("reconcile: persist order id for {close_id} failed: {e}");
            }
        }
        if resp.filled_qty > dec!(0) {
            let qty_f = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
            let fee_f = resp
                .fee
                .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let px_f = resp
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .and_then(|p| crate::execution::ledger_px_usd(state, &leg.venue, &leg.symbol, p));
            let oid = (!resp.order_id.trim().is_empty()).then_some(resp.order_id.as_str());
            match px_f.filter(|p| *p > 0.0) {
                Some(px_f) if qty_f > 0.0 => {
                    let _ = state
                        .state_store
                        .apply_leg_fill(&close_id, child_side, qty_f, px_f, fee_f, oid);
                }
                _ if qty_f > 0.0 => {
                    // Conversion required but no rate available — record the
                    // quantity, leave the price cell empty (never store a
                    // coin value under the ledger's USD label).
                    let _ = state
                        .state_store
                        .apply_leg_order_state(&close_id, child_side, qty_f, None, fee_f);
                }
                _ => {}
            }
        }
    }

    // A child whose dispatch produced no order ids and no fills never
    // reached a venue — mark it failed so it neither blocks re-dispatch
    // on the next pass nor consumes the attempt bound.
    if let Ok(Some(child)) = state.state_store.get_signal(&close_id) {
        let submitted = child.legs.iter().any(|l| {
            l.venue_order_id
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
                || l.filled_qty > 0.0
        });
        if !submitted {
            let note = match &first_dispatch_err {
                Some(e) => format!("residual flatten never dispatched an order: {e}"),
                None => "residual flatten never dispatched an order".to_string(),
            };
            let _ = state.state_store.update_signal_status(
                &close_id,
                SignalStatus::Failed,
                Some(note.as_str()),
            );
        }
    }
}

/// Spawn the reconciliation supervisor: an immediate startup pass (restart
/// recovery for orders accepted before shutdown) followed by a periodic
/// loop. The pass never overlaps itself (`reconcile_lock` try-lock) and
/// never converts a poll failure into a resolved state.
pub fn spawn_supervisor(state: Arc<AppEngineState>) {
    tauri::async_runtime::spawn(async move {
        let first = reconcile_pending_orders(&state).await;
        if first.pending_legs > 0 {
            tracing::info!(
                "order reconciliation startup pass: {} pending leg(s), {} polled, {} resolved, {} escalated, {} errors",
                first.pending_legs,
                first.polled,
                first.resolved,
                first.escalated,
                first.poll_errors
            );
        }
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(POLL_INTERVAL_SECS));
        let mut lock_warned = false;
        loop {
            interval.tick().await;
            let s = reconcile_pending_orders(&state).await;
            if s.vault_locked {
                if !lock_warned {
                    tracing::warn!(
                        "reconciliation paused: credential vault locked — {} pending leg(s) cannot be polled until re-unlock",
                        s.pending_legs
                    );
                    lock_warned = true;
                }
                continue;
            }
            lock_warned = false;
            if s.resolved > 0 || s.escalated > 0 || s.poll_errors > 0 {
                tracing::info!(
                    "order reconciliation pass: {} pending, {} resolved, {} escalated, {} errors",
                    s.pending_legs,
                    s.resolved,
                    s.escalated,
                    s.poll_errors
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::{dec, Decimal, VenueId};

    fn resp(status: OrderStatus, filled: Decimal, remaining: Decimal) -> OrderResponse {
        OrderResponse {
            order_id: "o1".into(),
            client_order_id: "c1".into(),
            venue: VenueId::new("kraken"),
            symbol: "BTC-USD".into(),
            status,
            original_qty: filled + remaining,
            filled_qty: filled,
            remaining_qty: remaining,
            avg_fill_price: None,
            fee: Some(dec!(0)),
            fee_currency: "USD".into(),
            raw_response: None,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn classify_terminal_and_working_states() {
        let intended = dec!(1);
        assert_eq!(
            classify_response(&resp(OrderStatus::Filled, dec!(1), dec!(0)), intended),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(0), dec!(1)), intended),
            LegPoll::CancelledEmpty
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Rejected, dec!(0), dec!(1)), intended),
            LegPoll::CancelledEmpty
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Expired, dec!(0), dec!(1)), intended),
            LegPoll::CancelledEmpty
        );
        // Cancelled-after-partial is terminal with nonzero residue — never
        // auto-resolvable.
        assert_eq!(
            classify_response(
                &resp(OrderStatus::Cancelled, dec!(0.4), dec!(0.6)),
                intended
            ),
            LegPoll::PartialTerminal
        );
        // Terminal with full fill but odd status spelling still counts.
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(1), dec!(0)), intended),
            LegPoll::Filled
        );
        // A terminal cancel that reports zero remaining but less than the
        // intended quantity is STILL partial exposure — the venue ended the
        // order early, not filled it.
        assert_eq!(
            classify_response(
                &resp(OrderStatus::Cancelled, dec!(0.999), dec!(0)),
                intended
            ),
            LegPoll::PartialTerminal
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Expired, dec!(0.5), dec!(0)), intended),
            LegPoll::PartialTerminal
        );
        // `Filled` status with materially short quantity is a venue
        // inconsistency — escalate rather than trust the status word.
        assert_eq!(
            classify_response(&resp(OrderStatus::Filled, dec!(0.4), dec!(0)), intended),
            LegPoll::PartialTerminal
        );
        // Wire-grid dust (sub-ppm) still counts as filled.
        assert_eq!(
            classify_response(
                &resp(OrderStatus::Cancelled, dec!(0.9999999995), dec!(0)),
                intended
            ),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::New, dec!(0), dec!(1)), intended),
            LegPoll::Working
        );
        assert_eq!(
            classify_response(
                &resp(OrderStatus::PartiallyFilled, dec!(0.4), dec!(0.6)),
                intended
            ),
            LegPoll::Working
        );
    }

    #[test]
    fn classify_overfill_is_filled_not_clamped() {
        // Regression (#30): a venue reporting MORE than the intended
        // quantity is a truthful surplus, never a shortfall — the fill must
        // classify Filled so the verbatim quantity lands in the ledger,
        // where `signal_delta` exposes the excess as real exposure. A clamp
        // at `intended` would hide it forever.
        let intended = dec!(1);
        assert_eq!(
            classify_response(&resp(OrderStatus::Filled, dec!(1.2), dec!(0)), intended),
            LegPoll::Filled
        );
        // Terminal-cancelled with remaining zero and a surplus fill: still
        // provably full, not partial.
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(1.2), dec!(0)), intended),
            LegPoll::Filled
        );
        // Boundary: exactly intended is filled; the ppm-epsilon edge is
        // filled; one real unit below the edge is residual exposure.
        assert_eq!(
            classify_response(&resp(OrderStatus::Filled, dec!(1), dec!(0)), intended),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(
                &resp(OrderStatus::Filled, dec!(0.999999), dec!(0)),
                intended
            ),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(
                &resp(OrderStatus::Filled, dec!(0.999998), dec!(0)),
                intended
            ),
            LegPoll::PartialTerminal
        );
    }

    #[test]
    fn deadline_uses_signal_creation_time() {
        assert_eq!(signal_age_secs("not-a-date"), 0);
        let fresh = Utc::now().to_rfc3339();
        assert!(signal_age_secs(&fresh) < NON_FINAL_DEADLINE_SECS);
        let old =
            (Utc::now() - chrono::Duration::seconds(NON_FINAL_DEADLINE_SECS + 60)).to_rfc3339();
        assert!(signal_age_secs(&old) > NON_FINAL_DEADLINE_SECS);
    }

    fn leg(venue: &str, side: LegSide, filled: f64) -> SignalLeg {
        SignalLeg {
            id: "l".into(),
            signal_id: "s".into(),
            account_id: None,
            venue: venue.into(),
            side,
            symbol: "BTC-USD".into(),
            intended_qty: 1.0,
            filled_qty: filled,
            fill_price: Some(100.0),
            fee_usd: 0.0,
            venue_order_id: Some("o".into()),
            client_order_id: None,
            intended_price: None,
            unit_scale: 1.0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn exec_with_legs(legs: Vec<SignalLeg>) -> SignalExecution {
        SignalExecution {
            id: "s".into(),
            strategy_id: "strat".into(),
            symbol: "BTC-USD".into(),
            status: SignalStatus::Imbalanced,
            environment: "testnet".into(),
            route_id: None,
            account_long_id: None,
            account_short_id: None,
            expected_net_usd: None,
            fees_usd: 0.0,
            note: None,
            legs,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            closed_at: None,
        }
    }

    #[test]
    fn residual_quantities_uncovered_then_covered() {
        let exec = exec_with_legs(vec![
            leg("kraken", LegSide::Long, 1.0),
            leg("hyperliquid", LegSide::Short, 0.0),
        ]);
        // No children: the filled long leg is fully residual; the empty leg
        // contributes nothing.
        let r = residual_leg_quantities(&exec, &[]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, 1.0);
        assert_eq!(r[0].0.venue, "kraken");

        // A flatten child that closed 0.4 of the long leg (recorded as a
        // Short leg on the same venue) leaves 0.6 residual.
        let mut child = exec_with_legs(vec![leg("kraken", LegSide::Short, 0.4)]);
        child.id = "flat-s-0001".into();
        let r = residual_leg_quantities(&exec, &[child]);
        assert_eq!(r.len(), 1);
        assert!((r[0].1 - 0.6).abs() < 1e-9);

        // Full coverage: nothing left to flatten.
        let mut child2 = exec_with_legs(vec![leg("kraken", LegSide::Short, 1.0)]);
        child2.id = "flat-s-0002".into();
        assert!(residual_leg_quantities(&exec, &[child2]).is_empty());
    }

    #[test]
    fn residual_quantities_require_matching_venue_and_closing_side() {
        let exec = exec_with_legs(vec![leg("kraken", LegSide::Long, 1.0)]);
        // A child on a DIFFERENT venue must not cover this leg's residual.
        let mut wrong_venue = exec_with_legs(vec![leg("aster", LegSide::Short, 1.0)]);
        wrong_venue.id = "flat-s-0001".into();
        let r = residual_leg_quantities(&exec, &[wrong_venue]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, 1.0);
        // A child in the SAME direction (re-buy) must not cover either.
        let mut same_side = exec_with_legs(vec![leg("kraken", LegSide::Long, 1.0)]);
        same_side.id = "flat-s-0002".into();
        let r = residual_leg_quantities(&exec, &[same_side]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, 1.0);
    }

    #[test]
    fn derive_status_all_filled_entry_balances() {
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::Filled],
            2,
            10,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Balanced));
    }

    #[test]
    fn derive_status_all_filled_close_resolves() {
        let (st, _) = derive_signal_status(&[LegPoll::Filled], 1, 10, true, false, None);
        assert_eq!(st, Some(SignalStatus::Closed));
    }

    #[test]
    fn derive_status_all_empty_entry_fails_clean() {
        let (st, _) = derive_signal_status(
            &[LegPoll::CancelledEmpty, LegPoll::CancelledEmpty],
            2,
            10,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Failed));
    }

    #[test]
    fn derive_status_mixed_fill_and_empty_entry_is_imbalanced() {
        // The core residual-exposure case: one leg filled, the other ended
        // empty — the ticker must stay blocked.
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::CancelledEmpty],
            2,
            10,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Imbalanced));
    }

    #[test]
    fn derive_status_empty_close_is_never_clean() {
        // A close that ended empty may still hold the position.
        let (st, _) = derive_signal_status(&[LegPoll::CancelledEmpty], 1, 10, true, false, None);
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_partial_terminal_is_unresolved() {
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::PartialTerminal],
            2,
            10,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_flatten_child_empty_fails_retryable() {
        // A `flat-{parent}-*` child whose orders all ended venue-proven
        // empty is a failed ATTEMPT — Failed releases it from the
        // open-child dedup so residual dispatch can retry (bounded).
        let (st, _) = derive_signal_status(
            &[LegPoll::CancelledEmpty],
            1,
            10,
            true, // is_close_signal
            true, // is_flatten_child
            None,
        );
        assert_eq!(st, Some(SignalStatus::Failed));
        // Same zero-fill terminal outcome on an `external` close stays
        // unresolved — the operator owns it, no auto-retry machinery.
        let (st, _) = derive_signal_status(
            &[LegPoll::CancelledEmpty],
            1,
            10,
            true,  // is_close_signal
            false, // is_flatten_child
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_flatten_child_partial_closes() {
        // A flatten child that ended terminal with some fill is done —
        // its covered quantity feeds residual math and the parent
        // redispatches for the remainder; `unresolved` would strand the
        // child open and block every retry.
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::PartialTerminal],
            2,
            10,
            true, // is_close_signal
            true, // is_flatten_child
            None,
        );
        assert_eq!(st, Some(SignalStatus::Closed));

        // Mixed filled and cancelled empty on flatten child also resolves Closed for residual math
        let (st, note) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::CancelledEmpty],
            2,
            10,
            true,
            true,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Closed));
        assert!(note.unwrap().contains("Some leg(s) filled"));
    }

    #[test]
    fn derive_status_working_holds_then_escalates() {
        // Before the deadline: no status change.
        let (st, note) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::Working],
            2,
            10,
            false,
            false,
            None,
        );
        assert_eq!(st, None);
        assert!(note.is_none());
        // Past the deadline: escalate.
        let (st, note) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::Working],
            2,
            NON_FINAL_DEADLINE_SECS + 1,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
        assert!(note.unwrap().contains("manual reconciliation"));
    }

    #[test]
    fn derive_status_unknown_or_missing_leg_holds_then_escalates() {
        // Fewer polls than legs — an unproven leg can never resolve early.
        let (st, _) = derive_signal_status(&[LegPoll::Filled], 2, 10, false, false, None);
        assert_eq!(st, None);
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled],
            2,
            NON_FINAL_DEADLINE_SECS + 1,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
        // Unknown poll outcome behaves identically.
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::Unknown],
            2,
            NON_FINAL_DEADLINE_SECS + 1,
            false,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_cancel_notes_surface_in_escalation() {
        let notes = vec!["kraken ord-1 attempt 1/3: sent".to_string()];
        let (_, note) = derive_signal_status(
            &[LegPoll::Working],
            1,
            NON_FINAL_DEADLINE_SECS + 1,
            false,
            false,
            Some(&notes),
        );
        assert!(note.unwrap().contains("attempt 1/3"));
    }

    // ---- end-to-end reconciler passes with a scripted venue adapter ----

    struct MockVenue {
        venue: &'static str,
        responses: std::sync::Mutex<std::collections::VecDeque<Result<OrderResponse, String>>>,
        /// Per-order-id scripted answers — two legs sharing one venue
        /// (e.g. an entry leg and a later close) can answer differently.
        /// Checked before the shared queue.
        by_order: std::sync::Mutex<HashMap<String, Result<OrderResponse, String>>>,
        cancels: std::sync::Mutex<Vec<String>>,
        cancel_error: std::sync::Mutex<Option<String>>,
        placed: std::sync::Mutex<Vec<OrderRequest>>,
        place_response: std::sync::Mutex<Option<OrderResponse>>,
        fetch_calls: std::sync::Mutex<usize>,
        no_reduce_only: bool,
        /// When false, an empty `order_id` fetch (client-id lookup) bails —
        /// mirrors venues that cannot resolve orders by client id.
        cid_lookup: bool,
        positions: Vec<arbitrageur_core::position::Position>,
        balances: Vec<arbitrageur_core::position::Balance>,
    }

    impl MockVenue {
        fn new(venue: &'static str, responses: Vec<Result<OrderResponse, String>>) -> Self {
            Self {
                venue,
                responses: std::sync::Mutex::new(responses.into()),
                by_order: std::sync::Mutex::new(HashMap::new()),
                cancels: std::sync::Mutex::new(Vec::new()),
                cancel_error: std::sync::Mutex::new(None),
                placed: std::sync::Mutex::new(Vec::new()),
                place_response: std::sync::Mutex::new(None),
                fetch_calls: std::sync::Mutex::new(0),
                no_reduce_only: false,
                cid_lookup: true,
                positions: Vec::new(),
                balances: Vec::new(),
            }
        }

        fn spot(venue: &'static str, responses: Vec<Result<OrderResponse, String>>) -> Self {
            Self {
                no_reduce_only: true,
                ..Self::new(venue, responses)
            }
        }

        /// Pin an order-id-keyed response — wins over the shared queue.
        fn with_order_response(self, order_id: &str, r: Result<OrderResponse, String>) -> Self {
            self.by_order
                .lock()
                .unwrap()
                .insert(order_id.to_string(), r);
            self
        }

        /// Seed a derivative position the residual-flatten verification can
        /// observe — the venue-side truth the ledger residual is checked
        /// against before any order goes out.
        fn with_position(mut self, symbol: &str, side: Side, qty: Decimal) -> Self {
            self.positions.push(arbitrageur_core::position::Position {
                venue: VenueId::new(self.venue),
                symbol: symbol.to_string(),
                side,
                quantity: qty,
                entry_price: dec!(100),
                mark_price: dec!(100),
                unrealized_pnl: dec!(0),
                realized_pnl: dec!(0),
                liquidation_price: None,
                leverage: dec!(1),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
            self
        }

        /// Seed a spot balance — a sell-side residual on a venue with no
        /// position model verifies against the free base balance.
        fn with_balance(mut self, currency: &str, free: Decimal) -> Self {
            self.balances.push(arbitrageur_core::position::Balance {
                venue: VenueId::new(self.venue),
                currency: currency.to_string(),
                total: free,
                free,
                locked: dec!(0),
            });
            self
        }
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for MockVenue {
        fn venue_id(&self) -> VenueId {
            VenueId::new(self.venue)
        }
        fn name(&self) -> &str {
            self.venue
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            unimplemented!()
        }
        async fn fetch_orderbook(
            &self,
            s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
            // Tight ~2bps spread so the passive-unwind calm gates pass and
            // the residual flatten exercises its limit-first path.
            let mut b =
                arbitrageur_core::quote::OrderBook::new(VenueId::new(self.venue), s.to_string());
            b.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(99.99),
                dec!(10),
            ));
            b.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(100.01),
                dec!(10),
            ));
            Ok(b)
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            unimplemented!()
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            unimplemented!()
        }
        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            Ok(arbitrageur_core::position::AccountSnapshot {
                venue: VenueId::new(self.venue),
                equity: dec!(10000),
                free_margin: dec!(10000),
                maintenance_margin: dec!(0),
                margin_usage_pct: dec!(0),
                balances: self.balances.clone(),
                positions: self.positions.clone(),
                timestamp: Utc::now(),
            })
        }
        async fn place_order(&self, o: OrderRequest) -> anyhow::Result<OrderResponse> {
            self.placed.lock().unwrap().push(o);
            match self.place_response.lock().unwrap().clone() {
                Some(mut r) => {
                    r.venue = VenueId::new(self.venue);
                    Ok(r)
                }
                None => anyhow::bail!("no scripted place response"),
            }
        }
        fn live_order_submission_implemented(&self) -> bool {
            true
        }
        fn supports_reduce_only(&self, _symbol: &str) -> bool {
            !self.no_reduce_only
        }
        async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
            self.cancels
                .lock()
                .unwrap()
                .push(req.order_id.clone().unwrap_or_default());
            if let Some(e) = self.cancel_error.lock().unwrap().clone() {
                anyhow::bail!(e);
            }
            Ok(())
        }
        async fn fetch_order(
            &self,
            symbol: &str,
            order_id: &str,
            _cid: Option<&str>,
        ) -> anyhow::Result<OrderResponse> {
            *self.fetch_calls.lock().unwrap() += 1;
            // Client-id-only lookup (crash recovery): venues that support it
            // resolve the order id; others reject.
            if order_id.is_empty() {
                let Some(cid) = _cid.filter(|c| !c.is_empty()) else {
                    anyhow::bail!("order id required");
                };
                if !self.cid_lookup {
                    anyhow::bail!("client-id lookup unsupported");
                }
                let mut r = resp(OrderStatus::Filled, dec!(1), dec!(0));
                r.order_id = format!("oid-resolved-{cid}");
                r.client_order_id = cid.to_string();
                r.symbol = symbol.to_string();
                r.venue = VenueId::new(self.venue);
                return Ok(r);
            }
            if let Some(r) = self.by_order.lock().unwrap().get(order_id) {
                let mut r = r.clone().map_err(|e| anyhow::anyhow!(e))?;
                r.order_id = order_id.to_string();
                r.symbol = symbol.to_string();
                r.venue = VenueId::new(self.venue);
                return Ok(r);
            }
            let mut q = self.responses.lock().unwrap();
            let r = if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                // Keep returning the last scripted response so repeated
                // polls are deterministic.
                match q.front() {
                    Some(Ok(r)) => Ok(r.clone()),
                    Some(Err(e)) => Err(e.clone()),
                    None => Err("no scripted response".to_string()),
                }
            };
            let mut r = r.map_err(|e| anyhow::anyhow!(e))?;
            r.order_id = order_id.to_string();
            r.symbol = symbol.to_string();
            r.venue = VenueId::new(self.venue);
            Ok(r)
        }
        fn has_live_credentials(&self) -> bool {
            true
        }
    }

    /// Tempdir vault path: `AppEngineState::new()` would point at the real
    /// `~/.arbitrageur/vault.enc`, making vault-state-dependent behavior
    /// (e.g. the manual-lock reconcile pause) environment-sensitive.
    fn test_state() -> AppEngineState {
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_recon_vault_{}", rand::random::<u64>()));
        AppEngineState::new_with_vault_path(dir.join("vault.enc"))
    }

    fn seeded_state(
        strategy_id: &str,
        legs: Vec<(&'static str, LegSide)>,
    ) -> (AppEngineState, String) {
        let state = test_state();
        // Passive unwind is default-on; zero the attempt wait so the
        // limit/cancel/fallback sequence runs instantly in tests.
        state.settings.write().passive_unwind.timeout_secs = 0;
        let sid = format!("sig-{}", rand::random::<u32>());
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: sid.clone(),
                    strategy_id: strategy_id.into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: legs
                        .iter()
                        .map(|(v, side)| crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: (*v).into(),
                            side: *side,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        })
                        .collect(),
                },
                "testnet",
            )
            .unwrap();
        for (v, side) in &legs {
            state
                .state_store
                .record_leg_order_id(&sid, *side, &format!("oid-{v}"), Some(&format!("cid-{v}")))
                .unwrap();
        }
        (state, sid)
    }

    fn mock_filled() -> OrderResponse {
        let mut r = resp(OrderStatus::Filled, dec!(1), dec!(0));
        r.avg_fill_price = Some(dec!(100));
        r.fee = Some(dec!(0.5));
        r
    }

    #[tokio::test]
    async fn e2e_filled_legs_resolve_balanced() {
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        for v in ["kraken", "aster"] {
            state.test_adapters.write().unwrap().insert(
                v.to_string(),
                Arc::new(MockVenue::new(v, vec![Ok(mock_filled())])),
            );
        }
        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.polled, 2);
        assert_eq!(sum.resolved, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Balanced);
        assert!(sig.legs.iter().all(|l| l.filled_qty == 1.0));
    }

    /// Crash window: the client order id was persisted BEFORE dispatch and
    /// the venue accepted the order, but the process died before the ack's
    /// venue id was recorded. The leg is still pollable — the reconciler
    /// resolves the venue id through the client-id lookup and settles it.
    #[tokio::test]
    async fn e2e_client_id_only_leg_recovers_via_client_lookup() {
        let state = test_state();
        state.settings.write().passive_unwind.timeout_secs = 0;
        let sid = format!("sig-{}", rand::random::<u32>());
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: sid.clone(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "kraken".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "aster".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        // Orphan: client id persisted pre-dispatch, ack never landed.
        state
            .state_store
            .record_leg_client_order_id(&sid, LegSide::Long, "cid-kraken")
            .unwrap();
        state
            .state_store
            .record_leg_order_id(&sid, LegSide::Short, "oid-aster", Some("cid-aster"))
            .unwrap();

        let kraken = MockVenue::new("kraken", vec![Ok(mock_filled())]);
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".to_string(), Arc::new(kraken));
        state.test_adapters.write().unwrap().insert(
            "aster".to_string(),
            Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())])),
        );

        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.resolved, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Balanced);
        // The orphan leg's recovered venue id is now durable — later passes
        // poll it directly, no client-id lookup needed.
        let long_leg = sig.legs.iter().find(|l| l.side == LegSide::Long).unwrap();
        assert_eq!(
            long_leg.venue_order_id.as_deref(),
            Some("oid-resolved-cid-kraken")
        );
        assert_eq!(long_leg.filled_qty, 1.0);
    }

    /// A persisted client id on a venue that cannot look orders up by it:
    /// the leg is unproven, not terminally empty — it must stay pending for
    /// escalation rather than resolving the signal off a guess.
    #[tokio::test]
    async fn e2e_unresolvable_client_id_stays_pending() {
        let state = test_state();
        state.settings.write().passive_unwind.timeout_secs = 0;
        let sid = format!("sig-{}", rand::random::<u32>());
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: sid.clone(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "kraken".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "aster".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_client_order_id(&sid, LegSide::Long, "cid-kraken")
            .unwrap();
        state
            .state_store
            .record_leg_order_id(&sid, LegSide::Short, "oid-aster", Some("cid-aster"))
            .unwrap();

        let mut kraken = MockVenue::new("kraken", vec![Ok(mock_filled())]);
        kraken.cid_lookup = false;
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".to_string(), Arc::new(kraken));
        state.test_adapters.write().unwrap().insert(
            "aster".to_string(),
            Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())])),
        );

        let sum = reconcile_pending_orders(&state).await;
        // The aster leg polled+filled; the kraken leg is unproven, so the
        // signal cannot resolve — it stays in_flight for the deadline
        // escalation, never assumed empty.
        assert_eq!(sum.resolved, 0);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::InFlight);
        assert_eq!(
            sig.legs
                .iter()
                .find(|l| l.side == LegSide::Long)
                .unwrap()
                .filled_qty,
            0.0
        );
    }

    #[tokio::test]
    async fn e2e_manual_lock_pauses_polling_auto_lock_does_not() {
        let (state, _sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        for v in ["kraken", "aster"] {
            state.test_adapters.write().unwrap().insert(
                v.to_string(),
                Arc::new(MockVenue::new(v, vec![Ok(mock_filled())])),
            );
        }

        // Timer expiry → auto-lock: polling continues on provisioned creds.
        state.vault.write().initialize("test-password-123").unwrap();
        state.vault.write().force_timer_expiry();
        let sum = reconcile_pending_orders(&state).await;
        assert!(!sum.vault_locked);
        assert_eq!(sum.polled, 2);

        // Explicit operator lock → panic-halt: legs cannot be polled.
        let (state, _sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state.vault.write().initialize("test-password-123").unwrap();
        state.vault.write().lock();
        let sum = reconcile_pending_orders(&state).await;
        assert!(sum.vault_locked);
        assert_eq!(sum.polled, 0);
    }

    #[tokio::test]
    async fn e2e_fill_plus_cancel_empty_is_imbalanced() {
        // Scope is imbalance detection: the parent stays Imbalanced after
        // pass 1 even though the residual flatten dispatches (it resolves
        // only once the flat-* child reaches a terminal state).
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(
                MockVenue::new("kraken", vec![Ok(mock_filled())]).with_position(
                    "BTC-USD",
                    Side::Buy,
                    dec!(1),
                ),
            ),
        );
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new(
                "aster",
                vec![Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1)))],
            )),
        );
        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.resolved, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        // Residual one-sided exposure — never Failed (which would unblock
        // the ticker), never Balanced (which would claim neutrality).
        assert_eq!(sig.status, SignalStatus::Imbalanced);
    }

    #[tokio::test]
    async fn e2e_poll_error_preserves_state_then_escalates() {
        let (state, sid) = seeded_state("futures", vec![("kraken", LegSide::Long)]);
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new("kraken", vec![Err("timeout".to_string())])),
        );
        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.poll_errors, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::InFlight); // never assumed

        // Past the deadline, an unreachable order escalates — still never
        // assumed terminal.
        let old =
            (Utc::now() - chrono::Duration::seconds(NON_FINAL_DEADLINE_SECS + 60)).to_rfc3339();
        state.state_store.force_signal_age_for_test(&sid, &old);
        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.escalated, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Unresolved);
    }

    #[tokio::test]
    async fn e2e_stale_working_order_attempts_bounded_cancel() {
        let (state, sid) = seeded_state("futures", vec![("kraken", LegSide::Long)]);
        let mock = Arc::new(MockVenue::new(
            "kraken",
            vec![Ok(resp(OrderStatus::New, dec!(0), dec!(1)))],
        ));
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), mock.clone());
        let old =
            (Utc::now() - chrono::Duration::seconds(NON_FINAL_DEADLINE_SECS + 60)).to_rfc3339();
        state.state_store.force_signal_age_for_test(&sid, &old);

        // Each pass while Working past the deadline tries a cancel, bounded
        // by MAX_CANCEL_ATTEMPTS persisted in kv.
        for _ in 0..MAX_CANCEL_ATTEMPTS + 2 {
            reconcile_pending_orders(&state).await;
        }
        assert_eq!(
            mock.cancels.lock().unwrap().len() as u32,
            MAX_CANCEL_ATTEMPTS
        );
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Unresolved);
        assert!(sig.note.unwrap().contains("cancel"));
    }

    #[tokio::test]
    async fn e2e_close_signal_filled_resolves_closed() {
        let (state, sid) = seeded_state("external", vec![("kraken", LegSide::Short)]);
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())])),
        );
        reconcile_pending_orders(&state).await;
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Closed);
    }

    #[tokio::test]
    async fn e2e_identity_mismatch_never_applies() {
        // A venue response echoing a different order id must not touch the
        // ledger — the leg stays in_flight, nothing is written.
        let (state, sid) = seeded_state("futures", vec![("kraken", LegSide::Long)]);
        struct Mismatch;
        #[async_trait::async_trait]
        impl ExchangeAdapter for Mismatch {
            fn venue_id(&self) -> VenueId {
                VenueId::new("kraken")
            }
            fn name(&self) -> &str {
                "kraken"
            }
            async fn fetch_markets(
                &self,
            ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
                unimplemented!()
            }
            async fn fetch_orderbook(
                &self,
                _s: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
                unimplemented!()
            }
            async fn fetch_ticker(
                &self,
                _s: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
                unimplemented!()
            }
            async fn fetch_funding_rate(
                &self,
                _s: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
                unimplemented!()
            }
            async fn fetch_account(
                &self,
            ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
                unimplemented!()
            }
            async fn place_order(&self, _o: OrderRequest) -> anyhow::Result<OrderResponse> {
                unimplemented!()
            }
            async fn cancel_order(&self, _r: CancelRequest) -> anyhow::Result<()> {
                Ok(())
            }
            async fn fetch_order(
                &self,
                _s: &str,
                _o: &str,
                _c: Option<&str>,
            ) -> anyhow::Result<OrderResponse> {
                let mut r = mock_filled();
                r.order_id = "attacker-order".into(); // wrong id
                Ok(r)
            }
            fn has_live_credentials(&self) -> bool {
                true
            }
        }
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), Arc::new(Mismatch));
        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.polled, 1);
        assert_eq!(sum.advanced, 0);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::InFlight);
        assert_eq!(sig.legs[0].filled_qty, 0.0);
    }

    #[tokio::test]
    async fn e2e_imbalanced_dispatches_residual_flatten_and_parent_closes() {
        // Entry: kraken long fills, aster short ends cancelled-empty →
        // imbalanced. The reconciler submits a reduce-only flatten for the
        // kraken residual; when that child resolves filled, the parent
        // closes.
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );

        let kraken = Arc::new(
            MockVenue::new("kraken", vec![Ok(mock_filled())]).with_position(
                "BTC-USD",
                Side::Buy,
                dec!(1),
            ),
        );
        let mut place = mock_filled();
        place.order_id = "flat-oid-1".into();
        *kraken.place_response.lock().unwrap() = Some(place);
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), kraken.clone());
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new(
                "aster",
                vec![Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1)))],
            )),
        );

        // Pass 1: entry imbalanced → residual flatten dispatched + filled.
        reconcile_pending_orders(&state).await;
        let placed = kraken.placed.lock().unwrap().clone();
        assert_eq!(placed.len(), 1);
        assert!(placed[0].reduce_only);
        assert_eq!(placed[0].side, Side::Sell);
        assert_eq!(placed[0].quantity, dec!(1));

        let signals = state.state_store.list_signals(None, None).unwrap();
        let child = signals
            .iter()
            .find(|s| crate::state_store::is_flatten_child_id(s.id.as_str(), &sid))
            .expect("flatten child persisted");
        let child = state.state_store.get_signal(&child.id).unwrap().unwrap();
        assert_eq!(child.legs[0].venue_order_id.as_deref(), Some("flat-oid-1"));
        assert_eq!(child.legs[0].filled_qty, 1.0);

        // Pass 2: the fully-filled child resolves closed without a venue
        // call, which resolves the parent.
        reconcile_pending_orders(&state).await;
        let child = state.state_store.get_signal(&child.id).unwrap().unwrap();
        assert_eq!(child.status, SignalStatus::Closed);
        let parent = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(parent.status, SignalStatus::Closed);
    }

    #[tokio::test]
    async fn e2e_spot_residual_flatten_omits_reduce_only() {
        // Spot venues have no position model — a reduce-only flag is
        // rejected outright (Coinbase bails), which would strand the
        // residual forever. The flatten must go out as a plain market
        // order; the venue itself refuses an oversell, which is the same
        // guarantee the flag buys on perps.
        let (state, _sid) = seeded_state(
            "funding",
            vec![("coinbase", LegSide::Long), ("kraken", LegSide::Short)],
        );

        let coinbase = Arc::new(
            MockVenue::spot("coinbase", vec![Ok(mock_filled())]).with_balance("BTC", dec!(1)),
        );
        let mut place = mock_filled();
        place.order_id = "flat-oid-spot".into();
        *coinbase.place_response.lock().unwrap() = Some(place);
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("coinbase".into(), coinbase.clone());
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new(
                "kraken",
                vec![Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1)))],
            )),
        );

        reconcile_pending_orders(&state).await;
        let placed = coinbase.placed.lock().unwrap().clone();
        assert_eq!(placed.len(), 1);
        assert!(
            !placed[0].reduce_only,
            "spot residual flatten must not carry reduce_only"
        );
        assert_eq!(placed[0].side, Side::Sell);
        assert_eq!(placed[0].quantity, dec!(1));
    }

    #[tokio::test]
    async fn e2e_rejected_leg_without_order_id_resolves_imbalanced_and_flattens() {
        // The canary-shaped failure: leg A rejected at submit time (no
        // venue order id exists — identity is persisted at submission, so a
        // post-submission leg with no id is terminally empty); leg B
        // accepted and later polled filled. The signal must surface
        // Imbalanced — not park Unresolved forever — so residual
        // auto-flatten covers the naked leg.
        let state = test_state();
        state.settings.write().passive_unwind.timeout_secs = 0;
        let sid = format!("sig-{}", rand::random::<u32>());
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: sid.clone(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "kraken".into(), // the rejected leg — no order id
                            side: LegSide::Long,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "aster".into(), // accepted, fills via poll
                            side: LegSide::Short,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        // Only the accepted leg carries an order id; the coordinator has
        // already finished and parked the signal at unresolved.
        state
            .state_store
            .record_leg_order_id(&sid, LegSide::Short, "oid-aster", Some("cid-aster"))
            .unwrap();
        state
            .state_store
            .update_signal_status(
                &sid,
                SignalStatus::Unresolved,
                Some("leg rejected; sibling working"),
            )
            .unwrap();
        // The aster poll lands the fill (as the reconciler records it).
        state
            .state_store
            .apply_leg_order_state(&sid, LegSide::Short, 1.0, Some(100.0), 0.5)
            .unwrap();

        let aster = Arc::new(
            MockVenue::new("aster", vec![Ok(mock_filled())]).with_position(
                "BTC-USD",
                Side::Sell,
                dec!(1),
            ),
        );
        let mut place = mock_filled();
        place.order_id = "flat-oid-x".into();
        *aster.place_response.lock().unwrap() = Some(place);
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("aster".into(), aster.clone());
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), Arc::new(MockVenue::new("kraken", vec![])));

        reconcile_pending_orders(&state).await;
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Imbalanced);
        // Residual = aster short → reduce-only BUY flatten on aster.
        let placed = aster.placed.lock().unwrap().clone();
        assert_eq!(placed.len(), 1);
        assert!(placed[0].reduce_only);
        assert_eq!(placed[0].side, Side::Buy);
        assert_eq!(placed[0].quantity, dec!(1));
    }

    #[tokio::test]
    async fn e2e_stuck_flatten_child_without_order_id_fails_and_redispatches() {
        // Restart-after-lockout shape: a flat-* child persisted but never
        // dispatched (vault locked at submit time) sits in_flight with no
        // order id. It must fail out — not count as open, not burn the
        // attempt bound — and the pass must dispatch a real child that
        // carries the parent to closed once it fills.
        let state = test_state();
        state.settings.write().passive_unwind.timeout_secs = 0;
        let sid = format!("sig-{}", rand::random::<u32>());
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: sid.clone(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "kraken".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "aster".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_order_id(&sid, LegSide::Short, "oid-aster", Some("cid-aster"))
            .unwrap();
        state
            .state_store
            .apply_leg_order_state(&sid, LegSide::Short, 1.0, Some(100.0), 0.5)
            .unwrap();
        state
            .state_store
            .update_signal_status(&sid, SignalStatus::Imbalanced, None)
            .unwrap();
        // The stuck child: persisted in_flight, never dispatched.
        let stuck_id = format!("flat-{}-dead", sid);
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: stuck_id.clone(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: Some(format!("auto residual flatten of {sid}")),
                    legs: vec![crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "aster".into(),
                        side: LegSide::Long,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                        unit_scale: None,
                    }],
                },
                "testnet",
            )
            .unwrap();
        // Backdate past the residual-retry throttle so this pass redispatches.
        state.state_store.force_signal_age_for_test(
            &stuck_id,
            &(Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
        );

        let aster = Arc::new(
            MockVenue::new("aster", vec![Ok(mock_filled())]).with_position(
                "BTC-USD",
                Side::Sell,
                dec!(1),
            ),
        );
        let mut place = mock_filled();
        place.order_id = "flat-oid-y".into();
        *aster.place_response.lock().unwrap() = Some(place);
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("aster".into(), aster.clone());
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), Arc::new(MockVenue::new("kraken", vec![])));

        // Pass 1: stuck child fails out, a fresh child dispatches.
        reconcile_pending_orders(&state).await;
        let stuck = state.state_store.get_signal(&stuck_id).unwrap().unwrap();
        assert_eq!(stuck.status, SignalStatus::Failed);
        let placed = aster.placed.lock().unwrap().clone();
        assert_eq!(placed.len(), 1);
        assert!(placed[0].reduce_only);
        assert_eq!(placed[0].side, Side::Buy);

        // Pass 2: the fresh child's poll lands the fill; parent closes.
        reconcile_pending_orders(&state).await;
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Closed);
    }

    #[tokio::test]
    async fn e2e_overfill_records_venue_truth_verbatim() {
        // Regression (#30): a venue reporting filled 1.2 against intended
        // 1.0 must land in the ledger as 1.2 — the +0.2 surplus is real
        // exposure `signal_delta` exposes. Clamping to `intended` would
        // hide it behind a clean Balanced status forever.
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        let mut over = resp(OrderStatus::Filled, dec!(1.2), dec!(0));
        over.avg_fill_price = Some(dec!(100));
        over.fee = Some(dec!(0.5));
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new("kraken", vec![Ok(over)])),
        );
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())])),
        );

        let sum = reconcile_pending_orders(&state).await;
        assert_eq!(sum.resolved, 1);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        assert_eq!(sig.status, SignalStatus::Balanced);
        let long = sig.legs.iter().find(|l| l.side == LegSide::Long).unwrap();
        assert_eq!(
            long.filled_qty, 1.2,
            "over-fill must persist verbatim, not clamp to intended 1.0"
        );
        let delta = state.state_store.signal_delta(&sid).unwrap();
        assert!(
            (delta - 0.2).abs() < 1e-9,
            "surplus must surface as nonzero delta, got {delta}"
        );
    }

    #[tokio::test]
    async fn e2e_manual_lock_skips_fee_enrichment() {
        // Regression (#45): a locked-at-boot vault must never leak venue
        // polls — the fee-enrichment path shares the same credential gate
        // as order polling, so a manual lock means ZERO fetch_order calls
        // and no fabricated fee writes.
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        // Short leg already filled by the submit ack with fee_usd = 0 —
        // exactly the unpriced row fee enrichment targets.
        state
            .state_store
            .apply_leg_fill(&sid, LegSide::Short, 1.0, 100.0, 0.0, Some("oid-aster"))
            .unwrap();
        let kraken = Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())]));
        let aster = Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())]));
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), kraken.clone());
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("aster".into(), aster.clone());

        state.vault.write().initialize("test-password-123").unwrap();
        state.vault.write().lock();
        let sum = reconcile_pending_orders(&state).await;
        assert!(sum.vault_locked);
        assert_eq!(
            *kraken.fetch_calls.lock().unwrap(),
            0,
            "manual lock must not poll venue truth"
        );
        assert_eq!(
            *aster.fetch_calls.lock().unwrap(),
            0,
            "fee enrichment must not poll under manual lock"
        );
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        let short = sig.legs.iter().find(|l| l.side == LegSide::Short).unwrap();
        assert_eq!(short.fee_usd, 0.0, "locked boot must not fabricate fees");
    }

    #[tokio::test]
    async fn e2e_fee_enrichment_polls_unpriced_leg_once_per_boot() {
        // The same unpriced leg, unlocked: exactly one enrichment
        // `fetch_order` — `fee_enrich_done` bounds the poll per boot, and
        // the persisted fee then excludes the row from later passes.
        let (state, sid) = seeded_state(
            "futures",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state
            .state_store
            .apply_leg_fill(&sid, LegSide::Short, 1.0, 100.0, 0.0, Some("oid-aster"))
            .unwrap();
        let kraken = Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())]));
        let mut fee_resp = resp(OrderStatus::Filled, dec!(1), dec!(0));
        fee_resp.avg_fill_price = Some(dec!(100));
        fee_resp.fee = Some(dec!(0.75));
        fee_resp.fee_currency = "USDC".into();
        let aster = Arc::new(MockVenue::new("aster", vec![Ok(fee_resp)]));
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), kraken.clone());
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("aster".into(), aster.clone());

        let sum = reconcile_pending_orders(&state).await;
        assert!(!sum.vault_locked);
        let sig = state.state_store.get_signal(&sid).unwrap().unwrap();
        let short = sig.legs.iter().find(|l| l.side == LegSide::Short).unwrap();
        assert_eq!(short.fee_usd, 0.75, "venue-reported fee must persist");
        assert_eq!(
            *aster.fetch_calls.lock().unwrap(),
            1,
            "enrichment polls the unpriced leg exactly once"
        );

        // A second pass must not re-poll — the durable bound holds.
        reconcile_pending_orders(&state).await;
        assert_eq!(
            *aster.fetch_calls.lock().unwrap(),
            1,
            "enrichment is one-shot per boot"
        );
    }

    /// Settle-gap on a FAILED close: a `flat-*` child marked Failed while
    /// its cancel/fill state was unverifiable can leave a live venue order
    /// the ledger stopped watching. The zero-fill leg must stay pollable —
    /// when the venue reports the late fill, coverage completes and the
    /// parent resolves, while the child's own status stays terminal.
    #[tokio::test]
    async fn e2e_failed_flat_child_late_fill_resolves_parent() {
        let (state, parent) = seeded_state(
            "funding",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        // Parent entry fully filled → balanced.
        for side in [LegSide::Long, LegSide::Short] {
            state
                .state_store
                .apply_leg_order_state(&parent, side, 1.0, Some(100.0), 0.5)
                .unwrap();
        }
        state
            .state_store
            .update_signal_status(&parent, SignalStatus::Balanced, None)
            .unwrap();

        // Failed flat child: aster close filled at dispatch, kraken close
        // order accepted-but-unverifiable (live order id, zero recorded
        // fill) — exactly the "order may still be working" case.
        let child = format!("flat-{parent}-0001");
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: child.clone(),
                    strategy_id: "funding".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "kraken".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "aster".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USD".into()),
                            intended_qty: 1.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_order_id(
                &child,
                LegSide::Short,
                "oid-kraken-close",
                Some("cid-kraken-close"),
            )
            .unwrap();
        state
            .state_store
            .apply_leg_order_state(&child, LegSide::Long, 1.0, Some(100.0), 0.5)
            .unwrap();
        state
            .state_store
            .update_signal_status(
                &child,
                SignalStatus::Failed,
                Some("cancel/fill state unverifiable"),
            )
            .unwrap();

        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())])),
        );

        let sum = reconcile_pending_orders(&state).await;
        // Only the failed child's empty kraken leg is pending — parent legs
        // are filled, the child's filled aster leg self-excludes.
        assert_eq!(sum.polled, 1);
        let child_sig = state.state_store.get_signal(&child).unwrap().unwrap();
        // Terminal is never reopened — but the late fill still books.
        assert_eq!(child_sig.status, SignalStatus::Failed);
        let kraken_leg = child_sig.legs.iter().find(|l| l.venue == "kraken").unwrap();
        assert_eq!(kraken_leg.filled_qty, 1.0);
        // Coverage complete → the parent resolves instead of hanging
        // `balanced` over a venue-flat position.
        let parent_sig = state.state_store.get_signal(&parent).unwrap().unwrap();
        assert_eq!(parent_sig.status, SignalStatus::Closed);
    }

    /// Issue #82: a `failed` flatten child whose venue close order is still
    /// WORKING must suppress any new residual dispatch — the ledger counts
    /// zero covered qty for it, so without the venue-truth check the
    /// dispatcher would stack a second close on top of the live one.
    #[tokio::test]
    async fn e2e_failed_flat_child_working_order_suppresses_dispatch() {
        // Parent: kraken entry filled, aster entry empty → imbalanced, one
        // naked long side to close.
        let (state, parent) = seeded_state(
            "funding",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state
            .state_store
            .apply_leg_order_state(&parent, LegSide::Long, 1.0, Some(100.0), 0.5)
            .unwrap();
        state
            .state_store
            .update_signal_status(&parent, SignalStatus::Imbalanced, None)
            .unwrap();

        // Failed flat child: a single kraken close leg with a live order id.
        let child = format!("flat-{parent}-0002");
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: child.clone(),
                    strategy_id: "funding".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "kraken".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                        unit_scale: None,
                    }],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_order_id(
                &child,
                LegSide::Short,
                "oid-kraken-close",
                Some("cid-kraken-close"),
            )
            .unwrap();
        state
            .state_store
            .update_signal_status(
                &child,
                SignalStatus::Failed,
                Some("cancel/fill state unverifiable"),
            )
            .unwrap();

        // kraken says the close order is still working; aster stays
        // working so the parent remains imbalanced.
        state.test_adapters.write().unwrap().insert(
            "kraken".into(),
            Arc::new(MockVenue::new(
                "kraken",
                vec![Ok(resp(OrderStatus::New, dec!(0), dec!(1)))],
            )),
        );
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new(
                "aster",
                vec![Ok(resp(OrderStatus::New, dec!(0), dec!(1)))],
            )),
        );

        reconcile_pending_orders(&state).await;

        let siblings = state
            .state_store
            .list_signals(None, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &parent))
            .count();
        assert_eq!(siblings, 1, "working close order must suppress redispatch");
        let parent_sig = state.state_store.get_signal(&parent).unwrap().unwrap();
        assert_eq!(
            parent_sig.status,
            SignalStatus::Imbalanced,
            "suppression inside grace window keeps the parent open, not escalated"
        );
    }

    /// Issue #82: the same `failed` child whose close order is provably
    /// dead (cancelled empty) must NOT block redispatch — the residual is
    /// still uncovered, so a fresh flatten submits.
    #[tokio::test]
    async fn e2e_failed_flat_child_dead_order_allows_dispatch() {
        let (state, parent) = seeded_state(
            "funding",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state
            .state_store
            .apply_leg_order_state(&parent, LegSide::Long, 1.0, Some(100.0), 0.5)
            .unwrap();
        state
            .state_store
            .update_signal_status(&parent, SignalStatus::Imbalanced, None)
            .unwrap();

        let child = format!("flat-{parent}-0003");
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: child.clone(),
                    strategy_id: "funding".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "kraken".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                        unit_scale: None,
                    }],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_order_id(
                &child,
                LegSide::Short,
                "oid-kraken-close",
                Some("cid-kraken-close"),
            )
            .unwrap();
        state
            .state_store
            .update_signal_status(
                &child,
                SignalStatus::Failed,
                Some("cancel/fill state unverifiable"),
            )
            .unwrap();
        // Backdate past the residual-retry throttle so this pass
        // redispatches; still inside the grace bound so no escalation.
        state.state_store.force_signal_age_for_test(
            &child,
            &(Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
        );

        // kraken: the old close order is cancelled-empty (dead) — pinned
        // per order id so the parent's entry-leg poll stays filled while
        // the child's close order reads dead for both the settle-gap poll
        // and the dispatcher's venue-truth check. The residual clamp needs
        // the venue-side long position to verify the redispatch quantity;
        // place_order accepts the new close.
        let kraken = MockVenue::new("kraken", vec![Ok(mock_filled())])
            .with_order_response(
                "oid-kraken-close",
                Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1))),
            )
            .with_position("BTC-USD", Side::Buy, dec!(1));
        *kraken.place_response.lock().unwrap() = Some(resp(OrderStatus::New, dec!(0), dec!(1)));
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), Arc::new(kraken));
        // aster's empty entry leg reads cancelled — (filled, empty) entry
        // derives imbalanced, the residual-dispatch precondition.
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new(
                "aster",
                vec![Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1)))],
            )),
        );

        reconcile_pending_orders(&state).await;

        let siblings = state
            .state_store
            .list_signals(None, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &parent))
            .count();
        assert_eq!(
            siblings, 2,
            "a provably dead close order must not block the residual retry"
        );
    }

    /// Issue #82: an unverifiable child order suppresses dispatch, and once
    /// the child ages past the grace bound the parent escalates to
    /// `unresolved` — bounded silence, never a stacked close on guesswork.
    #[tokio::test]
    async fn e2e_failed_flat_child_unverifiable_order_escalates_after_grace() {
        let (state, parent) = seeded_state(
            "funding",
            vec![("kraken", LegSide::Long), ("aster", LegSide::Short)],
        );
        state
            .state_store
            .apply_leg_order_state(&parent, LegSide::Long, 1.0, Some(100.0), 0.5)
            .unwrap();
        state
            .state_store
            .update_signal_status(&parent, SignalStatus::Imbalanced, None)
            .unwrap();

        let child = format!("flat-{parent}-0004");
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: child.clone(),
                    strategy_id: "funding".into(),
                    symbol: "BTC-USD".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "kraken".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                        unit_scale: None,
                    }],
                },
                "testnet",
            )
            .unwrap();
        state
            .state_store
            .record_leg_order_id(
                &child,
                LegSide::Short,
                "oid-kraken-close",
                Some("cid-kraken-close"),
            )
            .unwrap();
        state
            .state_store
            .update_signal_status(
                &child,
                SignalStatus::Failed,
                Some("cancel/fill state unverifiable"),
            )
            .unwrap();
        // Older than the grace bound: suppression must escalate, not keep
        // mute-waiting forever on a venue that cannot prove the order dead.
        state.state_store.force_signal_age_for_test(
            &child,
            &(Utc::now() - chrono::Duration::seconds(RESIDUAL_ORDER_GRACE_SECS + 60)).to_rfc3339(),
        );

        // The child's close order id returns only errors — provably
        // unverifiable — while the entry leg still polls filled.
        let kraken = MockVenue::new("kraken", vec![Ok(mock_filled())])
            .with_order_response("oid-kraken-close", Err("venue read timeout".to_string()));
        state
            .test_adapters
            .write()
            .unwrap()
            .insert("kraken".into(), Arc::new(kraken));
        state.test_adapters.write().unwrap().insert(
            "aster".into(),
            Arc::new(MockVenue::new(
                "aster",
                vec![Ok(resp(OrderStatus::Cancelled, dec!(0), dec!(1)))],
            )),
        );

        reconcile_pending_orders(&state).await;

        let siblings = state
            .state_store
            .list_signals(None, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &parent))
            .count();
        assert_eq!(siblings, 1, "unverifiable order must suppress redispatch");
        let parent_sig = state.state_store.get_signal(&parent).unwrap().unwrap();
        assert_eq!(
            parent_sig.status,
            SignalStatus::Unresolved,
            "aged unverifiable close order must surface, not sit mute"
        );
    }

    #[tokio::test]
    async fn account_position_for_bare_base_matches_quoted_row() {
        // Radar legs persist bare-base symbols ("BTC") while venue account
        // rows normalize to quoted spellings ("BTC-USDC"). A miss reads live
        // exposure as flat and "no residual" verifiers close families over
        // real positions — this is what stranded the sig-…08d269ae legs.
        let venue =
            MockVenue::new("mock", vec![]).with_position("BTC-USDC", Side::Buy, dec!(0.00119));
        let acc = venue.fetch_account().await.unwrap();
        let pos = venue
            .account_position_for(&acc, "BTC")
            .unwrap()
            .expect("bare BTC must match the sole normalized BTC-USDC row");
        assert_eq!(pos.quantity, dec!(0.00119));
        assert_eq!(pos.side, Side::Buy);
    }

    #[tokio::test]
    async fn account_position_for_exact_spelling_wins_over_base_twin() {
        // Exact spelling resolves its own row even when a same-base twin
        // exists — a USDC-quoted request must pick the USDC row, not fail
        // ambiguous against the USDT twin.
        let venue = MockVenue::new("mock", vec![])
            .with_position("BTC-USDC", Side::Buy, dec!(0.1))
            .with_position("BTC-USDT", Side::Sell, dec!(0.2));
        let acc = venue.fetch_account().await.unwrap();
        let pos = venue
            .account_position_for(&acc, "BTC-USDC")
            .unwrap()
            .expect("exact USDC row resolves despite the USDT twin");
        assert_eq!(pos.quantity, dec!(0.1));
        // …but a bare base spanning both rows is ambiguous — fail closed.
        assert!(venue.account_position_for(&acc, "BTC").is_err());
    }

    #[test]
    fn test_symbols_match_canonical_equivalence() {
        assert!(symbols_match("BTC/USD", "BTC-USD"));
        assert!(symbols_match("BTC/USD", "BTC"));
        assert!(symbols_match("BTCUSD", "BTC/USD"));
        assert!(symbols_match("PERP_ADA_USDC", "ADA"));
        assert!(symbols_match("ETH-USDT", "ETH"));
        assert!(!symbols_match("BTC", "ETH"));
        assert!(!symbols_match("SOL/USD", "ADA/USD"));
    }
}
