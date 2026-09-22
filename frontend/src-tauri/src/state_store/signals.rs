//! Signal lifecycle, legs, order reconciliation state, predictions.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Persist a selected candidate as `in_flight` before any order is
    /// dispatched. Because the row exists before execution, an interrupted
    /// run still blocks the symbol on restart until the legs reconcile.
    pub fn record_signal_execution(
        &self,
        signal: &NewSignalExecution,
    ) -> anyhow::Result<SignalExecution> {
        let id = normalize_id(&signal.id)?;
        let strategy_id = normalize_strategy_id(&signal.strategy_id)?;
        let symbol = normalize_symbol(&signal.symbol)?;
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
                 account_short_id, expected_net_usd, fees_usd, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'in_flight', ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9)",
            params![
                id,
                strategy_id,
                symbol,
                signal.route_id,
                account_long,
                account_short,
                signal.expected_net_usd,
                signal.note,
                now
            ],
        )?;
        for leg in &signal.legs {
            let leg_id = normalize_id(&format!("{id}-{}", leg.side.as_str()))?;
            let leg_symbol = match &leg.symbol {
                Some(sym) => normalize_symbol(sym)?,
                None => symbol.clone(),
            };
            tx.execute(
                "INSERT INTO signal_legs
                    (id, signal_id, account_id, venue, side, intended_qty,
                     filled_qty, fill_price, fee_usd, created_at, updated_at, symbol,
                     intended_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, 0, ?7, ?7, ?8, ?9)",
                params![
                    leg_id,
                    id,
                    leg.account_id,
                    leg.venue,
                    leg.side.as_str(),
                    leg.intended_qty,
                    now,
                    leg_symbol,
                    leg.intended_price
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
        let changed = if terminal {
            conn.execute(
                "UPDATE signal_executions
                 SET status = ?2, note = COALESCE(?3, note), updated_at = ?4, closed_at = ?4
                 WHERE id = ?1",
                params![signal_id, status.as_str(), note, now],
            )?
        } else {
            conn.execute(
                "UPDATE signal_executions
                 SET status = ?2, note = COALESCE(?3, note), updated_at = ?4
                 WHERE id = ?1",
                params![signal_id, status.as_str(), note, now],
            )?
        };
        if changed == 0 {
            anyhow::bail!("signal execution {signal_id} does not exist");
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
        let conn = self.conn.lock();
        // Monotonic write: a stale dispatch-time ack must never regress a
        // fill the reconciler already recorded from an authoritative poll.
        // Price/fee follow only when this write advances the cumulative
        // quantity, so a lagging ack cannot pair new qty with old price.
        let changed = conn.execute(
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
        conn.execute(
            "UPDATE signal_executions
             SET fees_usd = (SELECT COALESCE(SUM(fee_usd), 0) FROM signal_legs WHERE signal_id = ?1),
                 updated_at = ?2
             WHERE id = ?1",
            params![signal_id, now],
        )?;
        Ok(())
    }

    /// Attach the venue's order identity to a submitted leg before any fill is
    /// known. Accepted-but-working live closes need this durable handle for
    /// operator reconciliation and future restart recovery.
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
        let conn = self.conn.lock();
        let changed = conn.execute(
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
        conn.execute(
            "UPDATE signal_executions SET updated_at = ?2 WHERE id = ?1",
            params![signal_id, now],
        )?;
        Ok(())
    }

    /// One persisted leg that carries a venue order id on a signal that has
    /// not reached `closed`/`failed` — the reconciliation poll set.
    /// `sim-%` order ids are SimulatedExchange synthetic ids — paper fills
    /// must never be polled against a real venue.
    pub fn list_pending_order_legs(&self) -> anyhow::Result<Vec<(SignalExecution, SignalLeg)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT l.id FROM signal_legs l
             JOIN signal_executions e ON e.id = l.signal_id
             WHERE (
                 (l.venue_order_id IS NOT NULL AND l.venue_order_id != ''
                  AND l.venue_order_id NOT LIKE 'sim-%')
                 OR e.status IN ('imbalanced', 'unresolved')
               )
               AND e.status IN ('in_flight', 'imbalanced', 'unresolved')",
        )?;
        let leg_ids: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
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
        let conn = self.conn.lock();
        let changed = conn.execute(
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
        conn.execute(
            "UPDATE signal_executions
             SET fees_usd = (SELECT COALESCE(SUM(fee_usd), 0) FROM signal_legs WHERE signal_id = ?1),
                 updated_at = ?2
             WHERE id = ?1",
            params![signal_id, now],
        )?;
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
                    created_at, updated_at, closed_at
             FROM signal_executions WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(s) = symbol {
            sql.push_str(" AND symbol = ?");
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

    /// Signals that block new orders on a symbol: in-flight, imbalanced, or
    /// unresolved legs must reconcile before another candidate may dispatch.
    pub fn blocking_signals(&self, symbol: &str) -> anyhow::Result<Vec<SignalExecution>> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at
             FROM signal_executions
             WHERE symbol = ?1
               AND status IN ('in_flight', 'imbalanced', 'unresolved')
             ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map([&symbol], signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    pub fn ticker_blocked(&self, symbol: &str) -> anyhow::Result<bool> {
        Ok(!self.blocking_signals(symbol)?.is_empty())
    }

    /// Signed net delta of one signal's filled legs (long +, short −).
    /// Delta ≈ 0 means the signal hedged itself; anything else is residual
    /// directional exposure belonging to that signal alone.
    pub fn signal_delta(&self, signal_id: &str) -> anyhow::Result<f64> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE side WHEN 'long' THEN filled_qty ELSE -filled_qty END), 0)
             FROM signal_legs WHERE signal_id = ?1",
            [&signal_id],
            |r| r.get(0),
        )?)
    }

    /// Signed net position for a symbol across every signal whose filled legs
    /// still count as open (excludes closed and failed signals).
    pub fn ticker_net_position(&self, symbol: &str) -> anyhow::Result<f64> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE s.symbol = ?1 AND s.status NOT IN ('closed', 'failed')",
            [&symbol],
            |r| r.get(0),
        )?)
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
             WHERE l.account_id = ?1 AND l.symbol = ?2
               AND s.status NOT IN ('closed', 'failed')",
            params![account_id, symbol],
            |r| r.get(0),
        )?)
    }

    /// Per-venue net position on an exact execution symbol across all
    /// accounts/signals (see [`Self::account_symbol_net`]).
    pub fn venue_symbol_net(&self, venue: &str, symbol: &str) -> anyhow::Result<f64> {
        let venue = normalize_venue(venue)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE l.venue = ?1 AND l.symbol = ?2
               AND s.status NOT IN ('closed', 'failed')",
            params![venue, symbol],
            |r| r.get(0),
        )?)
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

    fn legs_for(conn: &Connection, signal_id: &str) -> rusqlite::Result<Vec<SignalLeg>> {
        let mut stmt = conn.prepare(
            "SELECT id, signal_id, account_id, venue, side, intended_qty,
                    filled_qty, fill_price, fee_usd, venue_order_id,
                    created_at, updated_at, symbol, client_order_id, intended_price
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
