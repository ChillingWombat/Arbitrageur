//! Signal lifecycle, legs, order reconciliation state, predictions.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

/// Per-table row counts removed by `clear_transaction_history` — surfaced
/// to the operator so the reset reports exactly what it erased.
#[derive(Debug, Clone, Default)]
pub struct HistoryClearCounts {
    pub signals: i64,
    pub legs: i64,
    pub lots: i64,
    pub logs: i64,
}

impl StateStore {
    /// Persist a selected candidate as `in_flight` before any order is
    /// dispatched. Because the row exists before execution, an interrupted
    /// run still blocks the symbol on restart until the legs reconcile.
    /// `environment` is the active trading environment (`testnet`|`live`)
    /// — durable on the row so testnet and live exposure/performance can
    /// never blend in aggregation or be polled through the wrong adapter.
    pub fn record_signal_execution(
        &self,
        signal: &NewSignalExecution,
        environment: &str,
    ) -> anyhow::Result<SignalExecution> {
        let id = normalize_id(&signal.id)?;
        let strategy_id = normalize_strategy_id(&signal.strategy_id)?;
        let symbol = normalize_symbol(&signal.symbol)?;
        let environment = match environment.trim().to_ascii_lowercase().as_str() {
            "testnet" | "live" => environment.trim().to_ascii_lowercase(),
            other => anyhow::bail!("invalid signal environment '{other}'"),
        };
        if signal.legs.is_empty() || signal.legs.len() > 2 {
            anyhow::bail!("signal executions require 1 or 2 legs");
        }
        let mut account_long: Option<String> = None;
        let mut account_short: Option<String> = None;
        for leg in &signal.legs {
            if !leg.intended_qty.is_finite() || leg.intended_qty <= 0.0 {
                anyhow::bail!("leg intended quantity must be finite and positive");
            }
            normalize_venue(&leg.venue)?;
            if let Some(aid) = &leg.account_id {
                normalize_id(aid)?;
            }
            if let Some(sym) = &leg.symbol {
                normalize_symbol(sym)?;
            }
            match leg.side {
                LegSide::Long => account_long = leg.account_id.clone(),
                LegSide::Short => account_short = leg.account_id.clone(),
            }
        }
        if let Some(rid) = &signal.route_id {
            normalize_id(rid)?;
        }
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, route_id, account_long_id,
                 account_short_id, expected_net_usd, fees_usd, note, created_at,
                 updated_at, environment)
             VALUES (?1, ?2, ?3, 'in_flight', ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9, ?10)",
            params![
                id,
                strategy_id,
                symbol,
                signal.route_id,
                account_long,
                account_short,
                signal.expected_net_usd,
                signal.note,
                now,
                environment
            ],
        )?;
        for leg in &signal.legs {
            let leg_id = normalize_id(&format!("{id}-{}", leg.side.as_str()))?;
            let leg_symbol = match &leg.symbol {
                Some(sym) => normalize_symbol(sym)?,
                None => symbol.clone(),
            };
            let unit_scale = leg.unit_scale.unwrap_or(1.0);
            if !unit_scale.is_finite() || unit_scale <= 0.0 {
                anyhow::bail!("leg unit_scale must be finite and positive");
            }
            tx.execute(
                "INSERT INTO signal_legs
                    (id, signal_id, account_id, venue, side, intended_qty,
                     filled_qty, fill_price, fee_usd, created_at, updated_at, symbol,
                     intended_price, unit_scale)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, 0, ?7, ?7, ?8, ?9, ?10)",
                params![
                    leg_id,
                    id,
                    leg.account_id,
                    leg.venue,
                    leg.side.as_str(),
                    leg.intended_qty,
                    now,
                    leg_symbol,
                    leg.intended_price,
                    unit_scale
                ],
            )?;
        }
        tx.commit()?;
        drop(conn);
        Ok(self
            .get_signal(&id)?
            .expect("signal execution must exist after insert"))
    }

    /// Transition a signal's status; terminal states stamp `closed_at`.
    /// Terminal is enforced in the UPDATE itself, not just by callers: a
    /// `closed`/`failed` row accepts only an idempotent rewrite of its own
    /// status (note may still be annotated), so a late reconciliation poll
    /// or a racing flatten can never reopen a resolved signal.
    pub fn update_signal_status(
        &self,
        signal_id: &str,
        status: SignalStatus,
        note: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, SignalStatus::Closed | SignalStatus::Failed);
        let conn = self.conn.lock();
        let is_flatten_child = signal_id.starts_with("flat-") || signal_id.starts_with("trim-");
        // `status NOT IN ('closed','failed') OR status = ?2` — a terminal row
        // only matches when the new status equals the stored one, except for
        // flatten/trim children where venue-authoritative fills can promote a
        // premature Failed attempt to Closed.
        let changed = if terminal {
            if status == SignalStatus::Closed && is_flatten_child {
                conn.execute(
                    "UPDATE signal_executions
                     SET status = ?2, note = COALESCE(?3, note), updated_at = ?4,
                         closed_at = COALESCE(closed_at, ?4)
                     WHERE id = ?1
                       AND (status != 'closed')",
                    params![signal_id, status.as_str(), note, now],
                )?
            } else {
                conn.execute(
                    "UPDATE signal_executions
                     SET status = ?2, note = COALESCE(?3, note), updated_at = ?4,
                         closed_at = COALESCE(closed_at, ?4)
                     WHERE id = ?1
                       AND (status NOT IN ('closed','failed') OR status = ?2)",
                    params![signal_id, status.as_str(), note, now],
                )?
            }
        } else {
            conn.execute(
                "UPDATE signal_executions
                 SET status = ?2, note = COALESCE(?3, note), updated_at = ?4
                 WHERE id = ?1
                   AND (status NOT IN ('closed','failed') OR status = ?2)",
                params![signal_id, status.as_str(), note, now],
            )?
        };
        if changed == 0 {
            // Distinguish a missing row from a terminal-guard refusal so the
            // caller sees why the transition was rejected.
            let current: Option<String> = conn
                .query_row(
                    "SELECT status FROM signal_executions WHERE id = ?1",
                    [&signal_id],
                    |r| r.get(0),
                )
                .optional()?;
            match current.as_deref() {
                None => anyhow::bail!("signal execution {signal_id} does not exist"),
                Some(cur) => anyhow::bail!(
                    "signal execution {signal_id} is already {cur}; terminal signals cannot reopen"
                ),
            }
        }
        Ok(())
    }

    /// Persist a forecast probability against a freshly-recorded signal.
    /// Called at dispatch time so the prediction exists before any fill —
    /// the row predates the outcome it will be scored against by design.
    /// `kind` must match the v26 CHECK list; `predicted_p` must be a real
    /// probability in [0, 1]. Upserts per (signal, kind) so a re-dispatch
    /// refreshes the forecast rather than duplicating it.
    #[allow(clippy::too_many_arguments)]
    pub fn record_signal_prediction(
        &self,
        signal_id: &str,
        kind: &str,
        predicted_p: f64,
        expected_net_bps: Option<f64>,
        strategy_id: &str,
        symbol: &str,
        venue_pair: &str,
        is_live: bool,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        if !(predicted_p.is_finite() && (0.0..=1.0).contains(&predicted_p)) {
            anyhow::bail!("predicted_p must be a finite probability in [0, 1]");
        }
        if let Some(v) = expected_net_bps {
            if !v.is_finite() {
                anyhow::bail!("expected_net_bps must be finite");
            }
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO signal_predictions
                (signal_id, kind, predicted_p, expected_net_bps, strategy_id,
                 symbol, venue_pair, is_live, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(signal_id, kind) DO UPDATE SET
                predicted_p = excluded.predicted_p,
                expected_net_bps = excluded.expected_net_bps,
                venue_pair = excluded.venue_pair,
                is_live = excluded.is_live,
                created_at = excluded.created_at,
                realized_net_bps = NULL,
                resolved_at = NULL",
            params![
                signal_id,
                kind,
                predicted_p,
                expected_net_bps,
                strategy_id,
                symbol,
                venue_pair,
                is_live as i64,
                now
            ],
        )?;
        Ok(())
    }

    /// Resolve every prediction on a signal with its reconciled realized
    /// net P&L (bps). Rows already resolved keep their first outcome — a
    /// re-close must never rewrite a scored observation.
    pub fn resolve_signal_predictions(
        &self,
        signal_id: &str,
        realized_net_bps: f64,
    ) -> anyhow::Result<usize> {
        let signal_id = normalize_id(signal_id)?;
        if !realized_net_bps.is_finite() {
            anyhow::bail!("realized_net_bps must be finite");
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE signal_predictions
             SET realized_net_bps = ?2, resolved_at = ?3
             WHERE signal_id = ?1 AND resolved_at IS NULL",
            params![signal_id, realized_net_bps, now],
        )?;
        Ok(n)
    }

    /// List prediction rows, optionally filtered. `resolved_only` keeps
    /// just the scored subset (realized outcome present).
    pub fn list_signal_predictions(
        &self,
        kind: Option<&str>,
        strategy_id: Option<&str>,
        is_live: Option<bool>,
        resolved_only: bool,
    ) -> anyhow::Result<Vec<SignalPredictionRow>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT signal_id, kind, predicted_p, expected_net_bps, strategy_id,
                    symbol, venue_pair, is_live, created_at, realized_net_bps, resolved_at
             FROM signal_predictions WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            args.push(Box::new(k.to_string()));
        }
        if let Some(s) = strategy_id {
            sql.push_str(" AND strategy_id = ?");
            args.push(Box::new(s.to_string()));
        }
        if let Some(l) = is_live {
            sql.push_str(" AND is_live = ?");
            args.push(Box::new(l as i64));
        }
        if resolved_only {
            sql.push_str(" AND resolved_at IS NOT NULL");
        }
        sql.push_str(" ORDER BY created_at DESC");
        let params: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params.as_slice(), |r| {
                Ok(SignalPredictionRow {
                    signal_id: r.get(0)?,
                    kind: r.get(1)?,
                    predicted_p: r.get(2)?,
                    expected_net_bps: r.get(3)?,
                    strategy_id: r.get(4)?,
                    symbol: r.get(5)?,
                    venue_pair: r.get(6)?,
                    is_live: r.get::<_, i64>(7)? != 0,
                    created_at: r.get(8)?,
                    realized_net_bps: r.get(9)?,
                    resolved_at: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Test-only: backdate a signal's created_at so reconciliation tests
    /// can exercise the escalation deadline without waiting.
    #[cfg(test)]
    pub fn force_signal_age_for_test(&self, signal_id: &str, created_at: &str) {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE signal_executions SET created_at = ?1 WHERE id = ?2",
            params![created_at, signal_id],
        )
        .expect("age signal for test");
    }

    /// Record an actual fill on one leg of a signal (matched by side).
    pub fn apply_leg_fill(
        &self,
        signal_id: &str,
        side: LegSide,
        filled_qty: f64,
        fill_price: f64,
        fee_usd: f64,
        venue_order_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        if !filled_qty.is_finite() || filled_qty < 0.0 {
            anyhow::bail!("filled quantity must be finite and nonnegative");
        }
        if !fill_price.is_finite() || fill_price <= 0.0 {
            anyhow::bail!("fill price must be finite and positive");
        }
        if !fee_usd.is_finite() || fee_usd < 0.0 {
            anyhow::bail!("fee must be finite and nonnegative");
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock();
        // Leg write + parent fee aggregate must commit atomically — a crash
        // between them would pair a new fill with a stale fee total.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Monotonic write: a stale dispatch-time ack must never regress a
        // fill the reconciler already recorded from an authoritative poll.
        // Price/fee follow only when this write advances the cumulative
        // quantity, so a lagging ack cannot pair new qty with old price.
        let changed = tx.execute(
            "UPDATE signal_legs
             SET fill_price = CASE WHEN ?3 >= filled_qty THEN ?4 ELSE fill_price END,
                 fee_usd = CASE WHEN ?3 >= filled_qty THEN MAX(fee_usd, ?5) ELSE fee_usd END,
                 filled_qty = MAX(filled_qty, ?3),
                 venue_order_id = COALESCE(?6, venue_order_id), updated_at = ?7
             WHERE signal_id = ?1 AND side = ?2",
            params![
                signal_id,
                side.as_str(),
                filled_qty,
                fill_price,
                fee_usd,
                venue_order_id,
                now
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("signal {signal_id} has no {side:?} leg");
        }
        tx.execute(
            "UPDATE signal_executions
             SET fees_usd = (SELECT COALESCE(SUM(fee_usd), 0) FROM signal_legs WHERE signal_id = ?1),
                 updated_at = ?2
             WHERE id = ?1",
            params![signal_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Attach the venue's order identity to a submitted leg before any fill is
    /// known. Accepted-but-working live closes need this durable handle for
    /// operator reconciliation and future restart recovery.
    /// Repair helper for the unwind-trail backfill: rewrite a close-family
    /// child's leg side. Children written before the order-direction
    /// convention was fixed carry the closed position side; the caller
    /// supplies the corrected side. Returns rows changed.
    pub fn repair_leg_side(&self, signal_id: &str, side: LegSide) -> anyhow::Result<usize> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE signal_legs SET side = ?2, updated_at = ?3 WHERE signal_id = ?1",
            params![signal_id, side.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(changed)
    }

    /// Delete every persisted transaction record belonging to one
    /// environment in a single transaction: signal executions (legs,
    /// predictions, and fixed-yield maturities cascade), cycle lots, and
    /// the execution journal. `environment` scopes the delete to the
    /// active env; `'unknown'` rows are included because env-scoped reads
    /// surface them in every environment — removing only tagged rows
    /// would leave "past transactions" visibly behind. The other env's
    /// rows are untouched. Callers must refuse while any signal counts as
    /// open or an in-scope lot is open — this method performs no
    /// exposure checks of its own.
    pub fn clear_transaction_history(
        &self,
        environment: &str,
    ) -> anyhow::Result<HistoryClearCounts> {
        let env = environment.trim().to_ascii_lowercase();
        if !matches!(env.as_str(), "testnet" | "live") {
            anyhow::bail!("environment must be testnet or live");
        }
        let is_live = (env == "live") as i64;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let count = |sql: &str, p: &[&dyn rusqlite::types::ToSql]| -> anyhow::Result<i64> {
            Ok(tx.query_row(sql, p, |r| r.get::<_, i64>(0))?)
        };
        let counts = HistoryClearCounts {
            signals: count(
                "SELECT COUNT(*) FROM signal_executions WHERE environment IN ('unknown', ?1)",
                &[&env],
            )?,
            legs: count(
                "SELECT COUNT(*) FROM signal_legs l JOIN signal_executions e ON e.id = l.signal_id
                 WHERE e.environment IN ('unknown', ?1)",
                &[&env],
            )?,
            lots: count(
                "SELECT COUNT(*) FROM cycle_lots WHERE environment IN ('unknown', ?1)",
                &[&env],
            )?,
            logs: count(
                "SELECT COUNT(*) FROM execution_log WHERE is_live = ?1",
                &[&is_live],
            )?,
        };
        tx.execute(
            "DELETE FROM signal_executions WHERE environment IN ('unknown', ?1)",
            params![env],
        )?;
        tx.execute(
            "DELETE FROM cycle_lots WHERE environment IN ('unknown', ?1)",
            params![env],
        )?;
        tx.execute(
            "DELETE FROM execution_log WHERE is_live = ?1",
            params![is_live],
        )?;
        tx.commit()?;
        Ok(counts)
    }

    pub fn record_leg_order_id(
        &self,
        signal_id: &str,
        side: LegSide,
        venue_order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let oid = venue_order_id.trim();
        if oid.is_empty() || oid.len() > 256 || oid.chars().any(|c| c.is_control()) {
            anyhow::bail!("invalid venue order id");
        }
        let cid = client_order_id.map(|c| c.trim()).filter(|c| !c.is_empty());
        if let Some(c) = cid {
            if c.len() > 256 || c.chars().any(|ch| ch.is_control()) {
                anyhow::bail!("invalid client order id");
            }
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE signal_legs
             SET venue_order_id = ?3,
                 client_order_id = COALESCE(?5, client_order_id),
                 updated_at = ?4
             WHERE signal_id = ?1 AND side = ?2",
            params![signal_id, side.as_str(), oid, now, cid],
        )?;
        if changed == 0 {
            anyhow::bail!("signal {signal_id} has no {side:?} leg");
        }
        tx.execute(
            "UPDATE signal_executions SET updated_at = ?2 WHERE id = ?1",
            params![signal_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Persist the engine-owned client order id BEFORE dispatch. The order
    /// request carries this id onto the venue wire, so writing it here closes
    /// the crash window between venue acceptance and ack persistence: after a
    /// restart the reconciler can still locate the order by client id even
    /// when no venue order id was ever recorded. Does not overwrite an
    /// existing value — the first persisted locator is authoritative.
    pub fn record_leg_client_order_id(
        &self,
        signal_id: &str,
        side: LegSide,
        client_order_id: &str,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let cid = client_order_id.trim();
        if cid.is_empty() || cid.len() > 256 || cid.chars().any(|c| c.is_control()) {
            anyhow::bail!("invalid client order id");
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE signal_legs
             SET client_order_id = COALESCE(NULLIF(client_order_id, ''), ?3),
                 updated_at = ?4
             WHERE signal_id = ?1 AND side = ?2",
            params![signal_id, side.as_str(), cid, now],
        )?;
        if changed == 0 {
            anyhow::bail!("signal {signal_id} has no {side:?} leg");
        }
        tx.execute(
            "UPDATE signal_executions SET updated_at = ?2 WHERE id = ?1",
            params![signal_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Clear a pre-persisted client order id when the coordinator proves the
    /// leg has no remaining live order or exposure (rejected outright,
    /// cancelled empty, or filled and already unwound). Without clearing the
    /// crash-recovery locator, `reconcile_pending_orders` would attempt
    /// `adopt_venue_order_id` on the dead/unwound leg instead of classifying
    /// it as `CancelledEmpty`.
    pub fn clear_leg_client_order_id(&self, signal_id: &str, side: LegSide) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE signal_legs
             SET client_order_id = NULL, updated_at = ?3
             WHERE signal_id = ?1 AND side = ?2 AND (venue_order_id IS NULL OR venue_order_id = '')",
            params![signal_id, side.as_str(), now],
        )?;
        Ok(())
    }

    /// One persisted leg that carries a venue order id — or a client order
    /// id persisted pre-dispatch — on a signal that has not reached
    /// `closed`/`failed`: the reconciliation poll set. A client-id-only leg
    /// is a crash-window orphan (venue may hold the order even though the
    /// ack was never recorded), so it stays pollable.
    /// `sim-%` order ids are SimulatedExchange synthetic ids — paper fills
    /// must never be polled against a real venue.
    /// `environment` scopes the set to one trading environment: legs are
    /// polled through the active environment's adapters, so a testnet order
    /// id must never reach a live venue endpoint (and vice versa). `None`
    /// returns every environment's pending legs; `Some(env)` additionally
    /// includes `unknown`-environment rows — legacy rows that cannot be
    /// attributed stay fail-closed-visible to both environments.
    pub fn list_pending_order_legs(
        &self,
        environment: Option<&str>,
    ) -> anyhow::Result<Vec<(SignalExecution, SignalLeg)>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT l.id FROM signal_legs l
             JOIN signal_executions e ON e.id = l.signal_id
             WHERE (
                 (l.venue_order_id IS NOT NULL AND l.venue_order_id != ''
                  AND l.venue_order_id NOT LIKE 'sim-%')
                 OR (l.client_order_id IS NOT NULL AND l.client_order_id != '')
                 OR e.status IN ('imbalanced', 'unresolved')
                 OR (e.status = 'in_flight' AND datetime(e.created_at) < datetime('now', '-30 seconds'))
               )
               AND (
                 e.status IN ('in_flight', 'imbalanced', 'unresolved')
                 OR (
                   -- Settle-gap fills: a `flat-*` close child marked Closed
                   -- or Failed while its order was still working can leave a
                   -- live venue order the ledger stopped watching — the leg
                   -- keeps an order id but zero qty. Poll it for a bounded
                   -- window so the recorded fill lands (or the venue proves
                   -- it dead); once `filled_qty > 0` the row self-excludes.
                   e.status IN ('closed', 'failed')
                   AND (e.id LIKE 'flat-%' OR e.id LIKE 'trim-%')
                   AND l.filled_qty < l.intended_qty
                   AND (
                     (l.venue_order_id IS NOT NULL AND l.venue_order_id != ''
                      AND l.venue_order_id NOT LIKE 'sim-%')
                     OR (l.client_order_id IS NOT NULL AND l.client_order_id != '')
                   )
                   AND datetime(e.created_at) > datetime('now', '-30 minutes')
                 )
               )",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(env) = environment {
            sql.push_str(" AND e.environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let leg_ids: Vec<String> = stmt
            .query_map(refs.as_slice(), |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        drop(conn);
        let mut out = Vec::with_capacity(leg_ids.len());
        for leg_id in leg_ids {
            let conn = self.conn.lock();
            let legs = conn.query_row(
                "SELECT signal_id FROM signal_legs WHERE id = ?1",
                [&leg_id],
                |r| r.get::<_, String>(0),
            );
            drop(conn);
            let Ok(signal_id) = legs else { continue };
            if let Some(exec) = self.get_signal(&signal_id)? {
                let leg = exec.legs.iter().find(|l| l.id == leg_id).cloned();
                if let Some(leg) = leg {
                    out.push((exec, leg));
                }
            }
        }
        Ok(out)
    }

    /// Terminal legs that filled with no recorded venue fee. Fills landing
    /// inside the submit acknowledgement carry `fee_usd = 0` because venue
    /// submit responses never report fees — the reconciler polls each once
    /// per process so the ledger reflects the real taker cost instead of a
    /// phantom zero. Bounded to recent legs: the poll list refreshes on
    /// every pass, and very old legs are not worth a venue round-trip.
    pub fn list_fee_unaccounted_legs(
        &self,
        since_secs: i64,
    ) -> anyhow::Result<Vec<(SignalExecution, SignalLeg)>> {
        let cutoff = (Utc::now() - chrono::Duration::seconds(since_secs)).to_rfc3339();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT l.id FROM signal_legs l
             WHERE l.filled_qty > 0 AND l.fee_usd <= 0
               AND l.venue_order_id IS NOT NULL AND l.venue_order_id != ''
               AND l.venue_order_id NOT LIKE 'sim-%'
               AND l.created_at > ?1",
        )?;
        let leg_ids: Vec<String> = stmt
            .query_map([cutoff], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        drop(conn);
        let mut out = Vec::with_capacity(leg_ids.len());
        for leg_id in leg_ids {
            let conn = self.conn.lock();
            let legs = conn.query_row(
                "SELECT signal_id FROM signal_legs WHERE id = ?1",
                [&leg_id],
                |r| r.get::<_, String>(0),
            );
            drop(conn);
            let Ok(signal_id) = legs else { continue };
            if let Some(exec) = self.get_signal(&signal_id)? {
                let leg = exec.legs.iter().find(|l| l.id == leg_id).cloned();
                if let Some(leg) = leg {
                    out.push((exec, leg));
                }
            }
        }
        Ok(out)
    }

    /// Apply an authoritative venue order-state snapshot to a leg.
    /// `filled_qty` and `fee_usd` are cumulative venue values — they only
    /// ever advance (a venue response reporting less than the recorded fill
    /// keeps the larger recorded value rather than regressing the ledger).
    /// `fill_price` replaces the stored price when provided.
    pub fn apply_leg_order_state(
        &self,
        signal_id: &str,
        side: LegSide,
        filled_qty: f64,
        fill_price: Option<f64>,
        fee_usd: f64,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        if !filled_qty.is_finite() || filled_qty < 0.0 {
            anyhow::bail!("filled quantity must be finite and nonnegative");
        }
        if let Some(p) = fill_price {
            if !p.is_finite() || p <= 0.0 {
                anyhow::bail!("fill price must be finite and positive when provided");
            }
        }
        if !fee_usd.is_finite() || fee_usd < 0.0 {
            anyhow::bail!("fee must be finite and nonnegative");
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock();
        // Same atomicity rule as apply_leg_fill: leg snapshot and the
        // parent's fee aggregate commit together or not at all.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE signal_legs
             SET filled_qty = MAX(filled_qty, ?3),
                 fill_price = COALESCE(?4, fill_price),
                 fee_usd = MAX(fee_usd, ?5),
                 updated_at = ?6
             WHERE signal_id = ?1 AND side = ?2",
            params![
                signal_id,
                side.as_str(),
                filled_qty,
                fill_price,
                fee_usd,
                now
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("signal {signal_id} has no {side:?} leg");
        }
        tx.execute(
            "UPDATE signal_executions
             SET fees_usd = (SELECT COALESCE(SUM(fee_usd), 0) FROM signal_legs WHERE signal_id = ?1),
                 updated_at = ?2
             WHERE id = ?1",
            params![signal_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_signal(&self, signal_id: &str) -> anyhow::Result<Option<SignalExecution>> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        let exec = conn
            .query_row(SIGNAL_BY_ID_SQL, [&signal_id], signal_exec_from_row)
            .optional()?;
        match exec {
            Some(mut e) => {
                e.legs = Self::legs_for(&conn, &e.id)?;
                Ok(Some(e))
            }
            None => Ok(None),
        }
    }

    pub fn list_signals(
        &self,
        symbol: Option<&str>,
        status: Option<SignalStatus>,
    ) -> anyhow::Result<Vec<SignalExecution>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at, environment
             FROM signal_executions WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(s) = symbol {
            sql.push_str(" AND symbol = ? COLLATE NOCASE");
            binds.push(Box::new(normalize_symbol(s)?));
        }
        if let Some(st) = status {
            sql.push_str(" AND status = ?");
            binds.push(Box::new(st.as_str().to_string()));
        }
        sql.push_str(" ORDER BY created_at DESC, id");
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(refs.as_slice(), signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    /// Signals whose fills still count as open exposure (`counts_as_open`):
    /// in-flight, balanced, imbalanced, unresolved — everything not closed
    /// or failed. Boot reconciliation and lifecycle gates use this to avoid
    /// hydrating closed history. `environment` scopes to one environment —
    /// `Some(env)` also includes `unknown` rows (unattributable exposure
    /// stays fail-closed-visible); `None` returns every environment.
    pub fn list_open_signals(
        &self,
        environment: Option<&str>,
    ) -> anyhow::Result<Vec<SignalExecution>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at, environment
             FROM signal_executions
             WHERE status IN ('in_flight','balanced','imbalanced','unresolved')",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(env) = environment {
            sql.push_str(" AND environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        sql.push_str(" ORDER BY created_at DESC, id");
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    /// Signals that block new orders on a symbol: in-flight, imbalanced, or
    /// unresolved legs must reconcile before another candidate may dispatch.
    /// `environment` scopes blocking to one environment's exposure —
    /// `Some(env)` also includes `unknown` rows so unattributable legacy
    /// exposure still blocks (fail-closed); `None` blocks on any env.
    pub fn blocking_signals(
        &self,
        symbol: &str,
        environment: Option<&str>,
    ) -> anyhow::Result<Vec<SignalExecution>> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at, environment
             FROM signal_executions
             WHERE symbol = ?1 COLLATE NOCASE
               AND status IN ('in_flight', 'imbalanced', 'unresolved')",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(symbol)];
        if let Some(env) = environment {
            sql.push_str(" AND environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        sql.push_str(" ORDER BY created_at, id");
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    pub fn ticker_blocked(&self, symbol: &str, environment: Option<&str>) -> anyhow::Result<bool> {
        Ok(!self.blocking_signals(symbol, environment)?.is_empty())
    }

    /// Set or clear the durable exits-only flag for (environment, strategy,
    /// symbol). Entry gates consult [`Self::ticker_reduce_only`]; exits are
    /// never gated by it — that is the entire point of the flag.
    pub fn set_ticker_reduce_only(
        &self,
        symbol: &str,
        strategy_id: &str,
        environment: &str,
        enabled: bool,
        ts_ms: i64,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let strategy_id = normalize_id(strategy_id)?;
        let environment = environment.trim().to_ascii_lowercase();
        let conn = self.conn.lock();
        if enabled {
            conn.execute(
                "INSERT OR REPLACE INTO ticker_reduce_only
                    (environment, strategy_id, symbol, set_ts_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![environment, strategy_id, symbol, ts_ms],
            )?;
        } else {
            conn.execute(
                "DELETE FROM ticker_reduce_only
                 WHERE environment = ?1 AND strategy_id = ?2 AND symbol = ?3 COLLATE NOCASE",
                params![environment, strategy_id, symbol],
            )?;
        }
        Ok(())
    }

    /// Whether (environment, strategy, symbol) is exits-only. Read errors
    /// propagate — entry callers must fail closed, never treat an unreadable
    /// flag as "not reduce-only".
    pub fn ticker_reduce_only(
        &self,
        symbol: &str,
        strategy_id: &str,
        environment: &str,
    ) -> anyhow::Result<bool> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ticker_reduce_only
             WHERE environment = ?1 AND strategy_id = ?2 AND symbol = ?3 COLLATE NOCASE",
            params![
                environment.trim().to_ascii_lowercase(),
                strategy_id.trim().to_ascii_lowercase(),
                symbol
            ],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// All exits-only rows for one environment — powers the positions panel
    /// badge and the frontend's auto-execution skip list.
    pub fn list_reduce_only_tickers(
        &self,
        environment: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT strategy_id, symbol FROM ticker_reduce_only WHERE environment = ?1
             ORDER BY strategy_id, symbol",
        )?;
        let rows = stmt.query_map([environment.trim().to_ascii_lowercase()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Count of matched open pairs on a symbol — signals whose legs both
    /// filled and remain open (`balanced`). This is the grid ladder's
    /// accumulated-unit count: derived from the durable ledger, so it is
    /// exact after restarts and can never drift from venue truth the way a
    /// cached counter can. in_flight/imbalanced/unresolved signals are not
    /// units — they hard-block via `blocking_signals` instead.
    pub fn open_balanced_signal_count(
        &self,
        symbol: &str,
        environment: Option<&str>,
    ) -> anyhow::Result<i64> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT COUNT(*) FROM signal_executions WHERE symbol = ?1 COLLATE NOCASE AND status = 'balanced'",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(symbol)];
        if let Some(env) = environment {
            sql.push_str(" AND environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        Ok(conn.query_row(&sql, refs.as_slice(), |r| r.get(0))?)
    }

    /// Signed net delta of one signal's filled legs (long +, short −).
    /// Delta ≈ 0 means the signal hedged itself; anything else is residual
    /// directional exposure belonging to that signal alone.
    /// Signed filled delta between a signal's legs in shared units (leg
    /// qty ÷ `unit_scale`). Cross-instrument tracked-group legs fill in
    /// their own instrument's units — dividing by the recorded scale
    /// compares them in the parent symbol's units the way same-unit
    /// legs always did.
    pub fn signal_delta(&self, signal_id: &str) -> anyhow::Result<f64> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE side WHEN 'long' THEN filled_qty ELSE -filled_qty END / unit_scale), 0)
             FROM signal_legs WHERE signal_id = ?1",
            [&signal_id],
            |r| r.get(0),
        )?)
    }

    /// Signed intended (dispatch-quantity) delta in shared units — the
    /// asymmetry baked in by per-venue wire-grid snapping. A scaled pair's
    /// snapped leg quantities can differ in shared units by up to one
    /// venue step per leg; comparing the filled delta against this bound
    /// keeps construction dust from reading as an imbalance while a real
    /// fill-time asymmetry still flags.
    pub fn signal_intended_delta(&self, signal_id: &str) -> anyhow::Result<f64> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE side WHEN 'long' THEN intended_qty ELSE -intended_qty END / unit_scale), 0)
             FROM signal_legs WHERE signal_id = ?1",
            [&signal_id],
            |r| r.get(0),
        )?)
    }

    /// Signed net position for a symbol across every signal whose filled legs
    /// still count as open (excludes closed and failed signals).
    /// `environment` scopes to one environment's exposure — `Some(env)`
    /// includes `unknown` rows; `None` nets across every environment.
    pub fn ticker_net_position(
        &self,
        symbol: &str,
        environment: Option<&str>,
    ) -> anyhow::Result<f64> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END / l.unit_scale), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE s.symbol = ?1 COLLATE NOCASE AND s.status NOT IN ('closed', 'failed')",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(symbol)];
        if let Some(env) = environment {
            sql.push_str(" AND s.environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        Ok(conn.query_row(&sql, refs.as_slice(), |r| r.get(0))?)
    }

    /// Per-account net position on an exact execution symbol — the
    /// attribution exchanges lose when they only report aggregate positions.
    /// Pre-v22 legs backfilled their parent symbol, so carry rows keep their
    /// original behavior while funding spot/perp legs remain distinct.
    pub fn account_symbol_net(&self, account_id: &str, symbol: &str) -> anyhow::Result<f64> {
        let account_id = normalize_id(account_id)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE l.account_id = ?1 AND l.symbol = ?2 COLLATE NOCASE
               AND s.status NOT IN ('closed', 'failed')",
            params![account_id, symbol],
            |r| r.get(0),
        )?)
    }

    /// Per-venue net position on an exact execution symbol across all
    /// accounts/signals (see [`Self::account_symbol_net`]). `environment`
    /// scopes to one environment — `Some(env)` includes `unknown` rows;
    /// `None` nets across every environment.
    pub fn venue_symbol_net(
        &self,
        venue: &str,
        symbol: &str,
        environment: Option<&str>,
    ) -> anyhow::Result<f64> {
        let venue = normalize_venue(venue)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE l.venue = ?1 COLLATE NOCASE AND l.symbol = ?2 COLLATE NOCASE
               AND s.status NOT IN ('closed', 'failed')",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(venue), Box::new(symbol)];
        if let Some(env) = environment {
            sql.push_str(" AND s.environment IN ('unknown', ?)");
            binds.push(Box::new(env.trim().to_ascii_lowercase()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        Ok(conn.query_row(&sql, refs.as_slice(), |r| r.get(0))?)
    }

    /// Realized per-leg slippage samples for a venue, direction-aware bps:
    /// longs pay `fill − intended`, shorts pay `intended − fill` (positive =
    /// adverse). Only legs that recorded both prices (schema v27+) qualify.
    /// Returns `(slippage_bps, filled_qty)` pairs newest-first.
    pub fn leg_slippage_samples_bps(
        &self,
        venue: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(f64, f64)>> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT side, intended_price, fill_price, filled_qty
             FROM signal_legs
             WHERE venue = ?1 AND intended_price IS NOT NULL AND fill_price IS NOT NULL
               AND intended_price > 0 AND fill_price > 0 AND filled_qty > 0
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![venue, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(side, intended, fill, qty)| {
                let slip = match side.as_str() {
                    "long" => fill - intended,
                    "short" => intended - fill,
                    _ => return None,
                };
                Some((slip / intended * 10_000.0, qty))
            })
            .collect())
    }

    /// Whether any signal leg references the account — recorded position
    /// history blocks account deletion (disable the account instead).
    pub fn account_has_signal_legs(&self, account_id: &str) -> anyhow::Result<bool> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_legs WHERE account_id = ?1)",
            [&account_id],
            |r| r.get::<_, i64>(0),
        )? == 1)
    }

    /// Persist the maturity tracker for a `-yu` sibling signal at entry.
    /// `lock_until_ms` comes from the opportunity DTO (venue-derived), the
    /// market ids let settlement detection survive the market's delisting
    /// from active feeds. Rows start `locked` — nothing here asserts
    /// settlement from wall-clock time alone.
    pub fn record_fixed_yield_maturity(
        &self,
        signal_id: &str,
        parent_signal_id: &str,
        lock_until_ms: i64,
        yu_market_ids: &[u64],
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let parent_signal_id = normalize_id(parent_signal_id)?;
        if lock_until_ms < 0 {
            anyhow::bail!("lock_until_ms must be non-negative");
        }
        let ids = yu_market_ids
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO fixed_yield_maturities
                 (signal_id, parent_signal_id, lock_until_ms, yu_market_ids,
                  maturity_state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'locked', ?5, ?5)
             ON CONFLICT(signal_id) DO UPDATE SET
                 parent_signal_id = excluded.parent_signal_id,
                 lock_until_ms = excluded.lock_until_ms,
                 yu_market_ids = excluded.yu_market_ids,
                 updated_at = excluded.updated_at",
            rusqlite::params![signal_id, parent_signal_id, lock_until_ms, ids, now],
        )?;
        Ok(())
    }

    /// All fixed-yield maturity rows, newest first.
    pub fn list_fixed_yield_maturities(&self) -> anyhow::Result<Vec<FixedYieldMaturity>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT signal_id, parent_signal_id, lock_until_ms, yu_market_ids,
                    maturity_state, resolved_action, venue_observation,
                    created_at, updated_at, resolved_at
             FROM fixed_yield_maturities ORDER BY lock_until_ms",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let ids: String = r.get(3)?;
                Ok(FixedYieldMaturity {
                    signal_id: r.get(0)?,
                    parent_signal_id: r.get(1)?,
                    lock_until_ms: r.get(2)?,
                    yu_market_ids: ids
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse::<u64>().ok())
                        .collect(),
                    maturity_state: r.get(4)?,
                    resolved_action: r.get(5)?,
                    venue_observation: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    resolved_at: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Venue-truth state transition. `observation` records what the venue
    /// reported (or the read error) — never cleared by a later transition
    /// unless a new observation replaces it.
    pub fn update_fixed_yield_maturity_state(
        &self,
        signal_id: &str,
        state: &str,
        observation: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        match state {
            "locked" | "due" | "settled" | "settlement_unverified" | "closed_early" => {}
            other => anyhow::bail!("invalid maturity state '{other}'"),
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE fixed_yield_maturities
             SET maturity_state = ?2,
                 venue_observation = COALESCE(?3, venue_observation),
                 updated_at = ?4
             WHERE signal_id = ?1",
            rusqlite::params![signal_id, state, observation, now],
        )?;
        Ok(())
    }

    /// Record the operator's explicit resolution. Only a venue-confirmed
    /// `settled` row may resolve — acting on `due`/`settlement_unverified`
    /// would pretend settlement that the venue has not reported.
    pub fn resolve_fixed_yield_maturity(
        &self,
        signal_id: &str,
        action: &str,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        match action {
            "close_pair" | "keep_carry" => {}
            other => anyhow::bail!("invalid maturity action '{other}'"),
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE fixed_yield_maturities
             SET resolved_action = ?2, resolved_at = ?3, updated_at = ?3
             WHERE signal_id = ?1
               AND maturity_state = 'settled'
               AND resolved_action IS NULL",
            rusqlite::params![signal_id, action, now],
        )?;
        if changed == 0 {
            anyhow::bail!(
                "maturity row for {signal_id} is not venue-confirmed settled or already resolved"
            );
        }
        Ok(())
    }

    /// Terminal marker for a completed priced early exit: the YU legs
    /// closed before maturity, so settlement no longer needs venue
    /// confirmation. Records `close_pair` as the resolution — the exit
    /// path always unwinds the hedged perp pair too.
    pub fn mark_fixed_yield_closed_early(
        &self,
        signal_id: &str,
        observation: &str,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE fixed_yield_maturities
             SET maturity_state = 'closed_early',
                 resolved_action = 'close_pair',
                 venue_observation = ?2,
                 resolved_at = ?3,
                 updated_at = ?3
             WHERE signal_id = ?1
               AND maturity_state IN ('locked','due','settlement_unverified')
               AND resolved_action IS NULL",
            rusqlite::params![signal_id, observation, now],
        )?;
        if changed == 0 {
            anyhow::bail!("maturity row for {signal_id} is already terminal or resolved");
        }
        Ok(())
    }

    fn legs_for(conn: &Connection, signal_id: &str) -> rusqlite::Result<Vec<SignalLeg>> {
        let mut stmt = conn.prepare(
            "SELECT id, signal_id, account_id, venue, side, intended_qty,
                    filled_qty, fill_price, fee_usd, venue_order_id,
                    created_at, updated_at, symbol, client_order_id, intended_price,
                    unit_scale
             FROM signal_legs WHERE signal_id = ?1
             ORDER BY CASE side WHEN 'long' THEN 0 ELSE 1 END",
        )?;
        let legs = stmt
            .query_map([&signal_id], leg_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        Ok(legs)
    }
}
