//! Margin snapshots and equity expectation/events.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Append one margin-utilisation observation for a paper venue. The
    /// writer enforces the cadence cap (see `MARGIN_SNAPSHOT_MIN_INTERVAL_MS`
    /// in commands); this method validates and persists unconditionally.
    pub fn record_margin_snapshot(
        &self,
        venue: &str,
        account_id: &str,
        ts_ms: i64,
        equity: f64,
        used_margin: f64,
        rate_pct: f64,
    ) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        let account_id = account_id.trim().to_lowercase();
        if account_id.len() > 128 {
            anyhow::bail!("margin snapshot account id must fit in 128 characters");
        }
        if ts_ms < 0 {
            anyhow::bail!("margin snapshot timestamp must be non-negative");
        }
        for (name, v) in [
            ("equity", equity),
            ("used margin", used_margin),
            ("rate", rate_pct),
        ] {
            if !v.is_finite() || v < 0.0 {
                anyhow::bail!("margin snapshot {name} must be finite and non-negative");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO margin_snapshots
                 (venue, account_id, ts_ms, equity, used_margin, rate_pct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![venue, account_id, ts_ms, equity, used_margin, rate_pct],
        )?;
        Ok(())
    }

    /// `(ts_ms, rate_pct)` points for one venue at or after `since_ms`,
    /// oldest first. Callers cap the window; the table itself is bounded by
    /// the shared retention-days prune.
    pub fn list_margin_snapshots(
        &self,
        venue: &str,
        since_ms: i64,
    ) -> anyhow::Result<Vec<MarginSnapshot>> {
        let venue = normalize_venue(venue)?;
        if since_ms < 0 {
            anyhow::bail!("margin snapshot window start must be non-negative");
        }
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT venue, account_id, ts_ms, equity, used_margin, rate_pct
             FROM margin_snapshots WHERE venue = ?1 AND ts_ms >= ?2
             ORDER BY ts_ms ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(params![venue, since_ms], |r| {
                Ok(MarginSnapshot {
                    venue: r.get(0)?,
                    account_id: r.get(1)?,
                    ts_ms: r.get(2)?,
                    equity: r.get(3)?,
                    used_margin: r.get(4)?,
                    rate_pct: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete snapshots older than `before_ms` (the shared retention-days
    /// cutoff). Returns rows removed.
    pub fn prune_margin_snapshots(&self, before_ms: i64) -> anyhow::Result<usize> {
        if before_ms < 0 {
            anyhow::bail!("margin snapshot prune cutoff must be non-negative");
        }
        let conn = self.conn.lock();
        Ok(conn.execute("DELETE FROM margin_snapshots WHERE ts_ms < ?1", [before_ms])?)
    }

    /// Stored baseline behind one paper account, if any.
    pub fn get_equity_expectation(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<(f64, String, i64)>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT expected, last_pos_sig, last_poll_ms
                 FROM paper_equity_expectations WHERE account_id = ?1",
                [&account_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// Persist (or refresh) one paper account's expected-equity baseline.
    pub fn set_equity_expectation(
        &self,
        account_id: &str,
        expected: f64,
        pos_sig: &str,
        poll_ms: i64,
    ) -> anyhow::Result<()> {
        let account_id = normalize_id(account_id)?;
        if !expected.is_finite() || expected < 0.0 {
            anyhow::bail!("expected equity must be finite and non-negative");
        }
        if poll_ms < 0 {
            anyhow::bail!("poll timestamp must be non-negative");
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO paper_equity_expectations
                 (account_id, expected, last_pos_sig, last_poll_ms, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
                 expected = excluded.expected,
                 last_pos_sig = excluded.last_pos_sig,
                 last_poll_ms = excluded.last_poll_ms,
                 updated_at = excluded.updated_at",
            params![account_id, expected, pos_sig, poll_ms, now],
        )?;
        Ok(())
    }

    fn insert_equity_event(&self, draft: &EquityEventDraft) -> anyhow::Result<AccountEquityEvent> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO account_equity_events
                 (account_id, ts_ms, expected, actual, delta, kind, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                draft.account_id,
                draft.ts_ms,
                draft.expected,
                draft.actual,
                draft.delta,
                draft.kind,
                draft.note
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(AccountEquityEvent {
            id,
            account_id: draft.account_id.clone(),
            ts_ms: draft.ts_ms,
            expected: draft.expected,
            actual: draft.actual,
            delta: draft.delta,
            kind: draft.kind.clone(),
            note: draft.note.clone(),
        })
    }

    /// Compare one poll of actual equity against the stored baseline and
    /// persist the outcome. A nonzero delta records an external-delta event
    /// and rebases the baseline to actual so total PnL never absorbs manual
    /// flows; a comparison gap records no event, only a coverage note.
    pub fn check_account_equity(
        &self,
        account_id: &str,
        actual: f64,
        pos_sig: &str,
        now_ms: i64,
    ) -> anyhow::Result<EquityCheckReport> {
        let account_id = normalize_id(account_id)?;
        if now_ms < 0 {
            anyhow::bail!("poll timestamp must be non-negative");
        }
        let prev = self.get_equity_expectation(&account_id)?;
        let gap_ms = prev
            .as_ref()
            .map(|(_, _, last_poll)| now_ms.saturating_sub(*last_poll));
        let expected = prev.as_ref().map(|(e, _, _)| *e);
        let positions_changed = prev
            .as_ref()
            .is_some_and(|(_, sig, _)| sig.as_str() != pos_sig);
        let positions_open = !pos_sig.is_empty();
        let outcome = crate::equity_attribution::check_equity_delta(
            expected,
            actual,
            positions_changed,
            positions_open,
            gap_ms,
        );
        if outcome.rebased && actual.is_finite() {
            self.set_equity_expectation(&account_id, actual, pos_sig, now_ms)?;
        } else if prev.is_some() {
            let keep = expected.unwrap_or(actual);
            if keep.is_finite() {
                self.set_equity_expectation(&account_id, keep, pos_sig, now_ms)?;
            }
        }
        let event = match outcome.event_kind {
            Some(kind) => Some(self.insert_equity_event(&EquityEventDraft {
                account_id: account_id.clone(),
                ts_ms: now_ms,
                expected: expected.unwrap_or(actual),
                actual,
                delta: outcome.delta,
                kind,
                note: outcome.note.clone().unwrap_or_default(),
            })?),
            None => None,
        };
        Ok(EquityCheckReport {
            event,
            note: outcome.note,
        })
    }

    /// Recent external-delta events, newest first. `limit` clamps to 1..=500.
    pub fn list_equity_events(
        &self,
        account_id: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<AccountEquityEvent>> {
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn.lock();
        let mut out = Vec::new();
        if let Some(raw) = account_id {
            let account_id = normalize_id(raw)?;
            let mut stmt = conn.prepare(
                "SELECT id, account_id, ts_ms, expected, actual, delta, kind, note
                 FROM account_equity_events WHERE account_id = ?1
                 ORDER BY ts_ms DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![account_id, limit], equity_event_from_row)?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, account_id, ts_ms, expected, actual, delta, kind, note
                 FROM account_equity_events
                 ORDER BY ts_ms DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], equity_event_from_row)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }
}
