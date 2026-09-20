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

use crate::state::AppEngineState;
use crate::state_store::SignalStatus;
use arbitrageur_core::order::OrderResponse;
use arbitrageur_core::types::OrderStatus;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

/// Working orders older than this escalate their signal to `unresolved`.
/// Live flows only submit market/IOC/post-only closes and entries, so a
/// genuinely working order past 15 minutes indicates a venue anomaly.
const NON_FINAL_DEADLINE_SECS: i64 = 15 * 60;

/// Supervisor cadence.
const POLL_INTERVAL_SECS: u64 = 5;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileSummary {
    pub pending_legs: usize,
    pub polled: usize,
    pub advanced: usize,
    pub resolved: usize,
    pub escalated: usize,
    pub poll_errors: usize,
    pub skipped: usize,
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
    state.sync_vault_credentials_to_adapters();

    // Aggregate per-signal outcomes for the status pass below.
    let mut outcomes: HashMap<String, Vec<LegPoll>> = HashMap::new();
    let mut signal_meta: HashMap<String, (String, usize, i64)> = HashMap::new();

    for (exec, leg) in &pending {
        let age = signal_age_secs(&exec.created_at);
        signal_meta
            .entry(exec.id.clone())
            .or_insert_with(|| (exec.strategy_id.clone(), exec.legs.len(), age));
        let legs_out = outcomes.entry(exec.id.clone()).or_default();

        let Some(order_id) = leg.venue_order_id.as_deref() else {
            legs_out.push(LegPoll::Unknown);
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
                        Ok(()) => summary.advanced += 1,
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
                legs_out.push(classify_response(&resp));
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
        // Close-type signals: `external` closes plus `flat-{parent}-*`
        // flatten children — both exist to reduce exposure, so an empty
        // terminal leg is never a clean "Failed".
        let is_close_signal = strategy_id == "external" || signal_id.starts_with("flat-");
        // Every persisted leg must be accounted for this pass — a leg
        // missing a venue order id can never be proven, so the signal stays
        // on the escalation path instead of resolving off partial evidence.
        let all_known = polls.len() == *leg_count && polls.iter().all(|p| *p != LegPoll::Unknown);
        let any_working = polls.contains(&LegPoll::Working);
        let (new_status, note): (Option<SignalStatus>, Option<String>) =
            if !all_known || any_working {
                // Still working or unproven: hold current state until the
                // deadline, then escalate — never assume terminal.
                if *age > NON_FINAL_DEADLINE_SECS {
                    (
                        Some(SignalStatus::Unresolved),
                        Some(format!(
                        "Order state not fully resolved within {}s; manual reconciliation required",
                        NON_FINAL_DEADLINE_SECS
                    )),
                    )
                } else {
                    (None, None)
                }
            } else if polls.iter().all(|p| *p == LegPoll::Filled) {
                (
                    Some(if is_close_signal {
                        SignalStatus::Closed
                    } else {
                        SignalStatus::Balanced
                    }),
                    Some("All legs reached terminal filled state via venue poll".into()),
                )
            } else if polls
                .iter()
                .all(|p| matches!(*p, LegPoll::Filled | LegPoll::CancelledEmpty))
                && polls.contains(&LegPoll::CancelledEmpty)
            {
                // Terminal zero-fill legs. A close whose order ended empty may
                // still hold the position — never done. An entry where EVERY
                // leg ended empty executed nothing — Failed is honest and
                // releases the ticker. An entry where SOME leg filled while
                // another ended empty holds residual one-sided exposure —
                // that is Imbalanced, never Failed (which would unblock the
                // ticker over a live residual).
                (
                    Some(if is_close_signal {
                        SignalStatus::Unresolved
                    } else if polls.iter().all(|p| *p == LegPoll::CancelledEmpty) {
                        SignalStatus::Failed
                    } else {
                        SignalStatus::Imbalanced
                    }),
                    Some("Order(s) ended with zero fill per venue".into()),
                )
            } else {
                (
                    Some(SignalStatus::Unresolved),
                    Some("Leg(s) ended partially filled or with mixed terminal states".into()),
                )
            };
        if let Some(status) = new_status {
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
                // A `flat-{parent}-*` close that just resolved filled means
                // the residual it was dispatched to cover is gone. When ALL
                // of a parent's flatten children are Closed, the parent can
                // leave `unresolved`/`imbalanced` — never resurrect a parent
                // that already reached closed/failed.
                if status == SignalStatus::Closed {
                    if let Some(parent_id) = signal_id
                        .strip_prefix("flat-")
                        .and_then(|s| s.rsplit_once('-').map(|(p, _)| p.to_string()))
                    {
                        resolve_flattened_parent(state, &parent_id);
                    }
                }
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
    let prefix = format!("flat-{parent_id}-");
    let Ok(signals) = state.state_store.list_signals(None, None) else {
        return;
    };
    let children: Vec<_> = signals
        .iter()
        .filter(|s| s.id.starts_with(&prefix))
        .collect();
    if children.is_empty() || !children.iter().all(|c| c.status == SignalStatus::Closed) {
        return;
    }
    if let Err(e) = state.state_store.update_signal_status(
        parent_id,
        SignalStatus::Closed,
        Some("All flatten orders resolved terminal-filled via venue poll"),
    ) {
        tracing::warn!("reconcile: failed to close flattened parent {parent_id}: {e}");
    } else {
        tracing::info!("reconcile: parent {parent_id} closed — all flatten children resolved");
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
        loop {
            interval.tick().await;
            let s = reconcile_pending_orders(&state).await;
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
}
