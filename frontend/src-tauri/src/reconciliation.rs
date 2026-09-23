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
use rust_decimal::prelude::FromPrimitive;
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

/// Map an authoritative venue snapshot onto a poll outcome for ledger math.
fn classify_response(resp: &OrderResponse) -> LegPoll {
    match resp.status {
        OrderStatus::Filled => LegPoll::Filled,
        OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired => {
            if resp.filled_qty > arbitrageur_core::types::dec!(0)
                && resp.remaining_qty > arbitrageur_core::types::dec!(0)
            {
                LegPoll::PartialTerminal
            } else if resp.filled_qty > arbitrageur_core::types::dec!(0) {
                LegPoll::Filled
            } else {
                LegPoll::CancelledEmpty
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
fn derive_signal_status(
    polls: &[LegPoll],
    leg_count: usize,
    age: i64,
    is_close_signal: bool,
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
        return (
            Some(if is_close_signal {
                SignalStatus::Unresolved
            } else if polls.iter().all(|p| *p == LegPoll::CancelledEmpty) {
                SignalStatus::Failed
            } else {
                SignalStatus::Imbalanced
            }),
            Some("Order(s) ended with zero fill per venue".into()),
        );
    }
    (
        Some(SignalStatus::Unresolved),
        Some("Leg(s) ended partially filled or with mixed terminal states".into()),
    )
}

/// One reconciliation pass. Returns counts for the monitor feed.
pub async fn reconcile_pending_orders(state: &AppEngineState) -> ReconcileSummary {
    let _guard = match state.reconcile_lock.try_lock() {
        Ok(g) => g,
        Err(_) => return ReconcileSummary::default(), // previous pass still running
    };
    let mut summary = ReconcileSummary::default();
    let pending = match state.state_store.list_pending_order_legs() {
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

        let Some(order_id) = leg.venue_order_id.as_deref().filter(|s| !s.is_empty()) else {
            // Order identity is persisted at submission. On post-submission
            // signals (imbalanced/unresolved — the only states whose no-id
            // legs the query returns) a missing id means the venue rejected
            // the order outright: terminally empty, not merely unverifiable.
            // Counting it as terminal-empty lets a filled sibling surface as
            // Imbalanced so residual auto-flatten covers the naked leg.
            legs_out.push(LegPoll::CancelledEmpty);
            continue;
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
                    || !resp.symbol.eq_ignore_ascii_case(&leg.symbol)
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
                let fee = resp.fee.to_string().parse::<f64>().unwrap_or(0.0);
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
                    .and_then(|p| p.to_string().parse::<f64>().ok());
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
                let poll = classify_response(&resp);
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
        // empty terminal leg is never a clean "Failed".
        let is_close_signal = strategy_id == "external"
            || signal_id.starts_with("flat-")
            || signal_id.starts_with("trim-");
        let (new_status, note): (Option<SignalStatus>, Option<String>) = derive_signal_status(
            polls,
            *leg_count,
            *age,
            is_close_signal,
            cancel_notes.get(signal_id),
        );
        if let Some(status) = new_status {
            // Terminal is terminal: this same pass may have just resolved a
            // `flat-{id}-*` child and closed the parent above — a stale leg
            // poll must never reopen Closed/Failed (HashMap outcome order is
            // arbitrary). Unresolved/imbalanced stay revisable by design.
            let already_terminal = state
                .state_store
                .get_signal(signal_id)
                .ok()
                .flatten()
                .map(|s| matches!(s.status, SignalStatus::Closed | SignalStatus::Failed))
                .unwrap_or(false);
            if already_terminal {
                continue;
            }
            if state
                .state_store
                .update_signal_status(signal_id, status, note.as_deref())
                .is_ok()
            {
                if status == SignalStatus::Unresolved {
                    summary.escalated += 1;
                } else {
                    summary.resolved += 1;
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
    }

    // Fee enrichment: fills that landed inside the submit acknowledgement
    // recorded `fee_usd = 0` because venue submit responses never report
    // fees — realized P&L then overstates the round trip by the actual
    // taker fee. Poll each unpriced filled leg exactly once per process
    // (bounded by `fee_enrich_done`); a venue reporting 0 is truthful.
    if let Ok(unpriced) = state.state_store.list_fee_unaccounted_legs(7 * 24 * 3600) {
        for (exec, leg) in unpriced {
            let key = format!("{}:{}", exec.id, leg.side.as_str());
            {
                let mut done = state.fee_enrich_done.lock();
                if !done.insert(key) {
                    continue;
                }
            }
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
                || !resp.symbol.eq_ignore_ascii_case(&leg.symbol)
            {
                tracing::warn!(
                    "reconcile fee-enrich: {} order {} identity mismatch — skipping",
                    leg.venue,
                    order_id
                );
                continue;
            }
            let fee = resp.fee.to_string().parse::<f64>().unwrap_or(0.0);
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
                .and_then(|p| p.to_string().parse::<f64>().ok());
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
    if children.len() >= MAX_RESIDUAL_ATTEMPTS {
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

    let close_id = format!("flat-{}-{:04x}", signal_id, rand::random::<u16>());
    let legs: Vec<NewSignalLeg> = residuals
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
        })
        .collect();
    if let Err(e) = state
        .state_store
        .record_signal_execution(&NewSignalExecution {
            id: close_id.clone(),
            strategy_id: exec.strategy_id.clone(),
            symbol: exec.symbol.clone(),
            route_id: exec.route_id.clone(),
            expected_net_usd: None,
            note: Some(format!("auto residual flatten of {signal_id}")),
            legs,
        })
    {
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
    for (leg, qty) in &residuals {
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
            let fee_f = resp.fee.to_string().parse::<f64>().unwrap_or(0.0);
            let px_f = resp
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .unwrap_or(0.0);
            let oid = (!resp.order_id.trim().is_empty()).then_some(resp.order_id.as_str());
            if qty_f > 0.0 && px_f > 0.0 {
                let _ = state
                    .state_store
                    .apply_leg_fill(&close_id, child_side, qty_f, px_f, fee_f, oid);
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
            fee: dec!(0),
            fee_currency: "USD".into(),
            raw_response: None,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn classify_terminal_and_working_states() {
        assert_eq!(
            classify_response(&resp(OrderStatus::Filled, dec!(1), dec!(0))),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(0), dec!(1))),
            LegPoll::CancelledEmpty
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Rejected, dec!(0), dec!(1))),
            LegPoll::CancelledEmpty
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::Expired, dec!(0), dec!(1))),
            LegPoll::CancelledEmpty
        );
        // Cancelled-after-partial is terminal with nonzero residue — never
        // auto-resolvable.
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(0.4), dec!(0.6))),
            LegPoll::PartialTerminal
        );
        // Terminal with full fill but odd status spelling still counts.
        assert_eq!(
            classify_response(&resp(OrderStatus::Cancelled, dec!(1), dec!(0))),
            LegPoll::Filled
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::New, dec!(0), dec!(1))),
            LegPoll::Working
        );
        assert_eq!(
            classify_response(&resp(OrderStatus::PartiallyFilled, dec!(0.4), dec!(0.6))),
            LegPoll::Working
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
        let (st, _) = derive_signal_status(&[LegPoll::Filled, LegPoll::Filled], 2, 10, false, None);
        assert_eq!(st, Some(SignalStatus::Balanced));
    }

    #[test]
    fn derive_status_all_filled_close_resolves() {
        let (st, _) = derive_signal_status(&[LegPoll::Filled], 1, 10, true, None);
        assert_eq!(st, Some(SignalStatus::Closed));
    }

    #[test]
    fn derive_status_all_empty_entry_fails_clean() {
        let (st, _) = derive_signal_status(
            &[LegPoll::CancelledEmpty, LegPoll::CancelledEmpty],
            2,
            10,
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
            None,
        );
        assert_eq!(st, Some(SignalStatus::Imbalanced));
    }

    #[test]
    fn derive_status_empty_close_is_never_clean() {
        // A close that ended empty may still hold the position.
        let (st, _) = derive_signal_status(&[LegPoll::CancelledEmpty], 1, 10, true, None);
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_partial_terminal_is_unresolved() {
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::PartialTerminal],
            2,
            10,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
    }

    #[test]
    fn derive_status_working_holds_then_escalates() {
        // Before the deadline: no status change.
        let (st, note) =
            derive_signal_status(&[LegPoll::Filled, LegPoll::Working], 2, 10, false, None);
        assert_eq!(st, None);
        assert!(note.is_none());
        // Past the deadline: escalate.
        let (st, note) = derive_signal_status(
            &[LegPoll::Filled, LegPoll::Working],
            2,
            NON_FINAL_DEADLINE_SECS + 1,
            false,
            None,
        );
        assert_eq!(st, Some(SignalStatus::Unresolved));
        assert!(note.unwrap().contains("manual reconciliation"));
    }

    #[test]
    fn derive_status_unknown_or_missing_leg_holds_then_escalates() {
        // Fewer polls than legs — an unproven leg can never resolve early.
        let (st, _) = derive_signal_status(&[LegPoll::Filled], 2, 10, false, None);
        assert_eq!(st, None);
        let (st, _) = derive_signal_status(
            &[LegPoll::Filled],
            2,
            NON_FINAL_DEADLINE_SECS + 1,
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
            Some(&notes),
        );
        assert!(note.unwrap().contains("attempt 1/3"));
    }

    // ---- end-to-end reconciler passes with a scripted venue adapter ----

    struct MockVenue {
        venue: &'static str,
        responses: std::sync::Mutex<std::collections::VecDeque<Result<OrderResponse, String>>>,
        cancels: std::sync::Mutex<Vec<String>>,
        cancel_error: std::sync::Mutex<Option<String>>,
        placed: std::sync::Mutex<Vec<OrderRequest>>,
        place_response: std::sync::Mutex<Option<OrderResponse>>,
        no_reduce_only: bool,
    }

    impl MockVenue {
        fn new(venue: &'static str, responses: Vec<Result<OrderResponse, String>>) -> Self {
            Self {
                venue,
                responses: std::sync::Mutex::new(responses.into()),
                cancels: std::sync::Mutex::new(Vec::new()),
                cancel_error: std::sync::Mutex::new(None),
                placed: std::sync::Mutex::new(Vec::new()),
                place_response: std::sync::Mutex::new(None),
                no_reduce_only: false,
            }
        }

        fn spot(venue: &'static str, responses: Vec<Result<OrderResponse, String>>) -> Self {
            Self {
                no_reduce_only: true,
                ..Self::new(venue, responses)
            }
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
            unimplemented!()
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
            .record_signal_execution(&crate::state_store::NewSignalExecution {
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
                    })
                    .collect(),
            })
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
        r.fee = dec!(0.5);
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
            Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())])),
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

        let kraken = Arc::new(MockVenue::new("kraken", vec![Ok(mock_filled())]));
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

        let coinbase = Arc::new(MockVenue::spot("coinbase", vec![Ok(mock_filled())]));
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
            .record_signal_execution(&crate::state_store::NewSignalExecution {
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
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "aster".into(), // accepted, fills via poll
                        side: LegSide::Short,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                    },
                ],
            })
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

        let aster = Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())]));
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
            .record_signal_execution(&crate::state_store::NewSignalExecution {
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
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "aster".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USD".into()),
                        intended_qty: 1.0,
                        intended_price: None,
                    },
                ],
            })
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
            .record_signal_execution(&crate::state_store::NewSignalExecution {
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
                }],
            })
            .unwrap();
        // Backdate past the residual-retry throttle so this pass redispatches.
        state.state_store.force_signal_age_for_test(
            &stuck_id,
            &(Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
        );

        let aster = Arc::new(MockVenue::new("aster", vec![Ok(mock_filled())]));
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
}
