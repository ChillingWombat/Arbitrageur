//! Radar bot positions/decisions, spatial baselines, monitor signal history.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    pub fn count_bot_decisions_since(
        &self,
        since_ts_ms: i64,
    ) -> anyhow::Result<Vec<(String, i64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT new_status, COUNT(*) FROM monitor_signal_events
             WHERE trigger = 'bot_decision' AND ts_ms >= ?1
             GROUP BY new_status",
        )?;
        let rows = stmt.query_map([since_ts_ms], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_bot_position(&self, row: &BotPositionRow) -> anyhow::Result<()> {
        let symbol = normalize_symbol(&row.symbol)?;
        let buy_venue = normalize_venue(&row.buy_venue)?;
        let sell_venue = normalize_venue(&row.sell_venue)?;
        if row.entry_ts_ms < 0 || row.deadline_ms < 0 || row.last_seen_ms < 0 {
            anyhow::bail!("negative bot position timestamp");
        }
        if let Some(u) = row.unwatched_since_ms {
            if u < 0 {
                anyhow::bail!("negative bot position unwatched timestamp");
            }
        }
        for (name, v) in [
            ("entry_dev", row.entry_dev_bps),
            ("friction_band", row.friction_band_bps),
            ("size_usd", row.size_usd),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite bot position {name}");
            }
        }
        if row.friction_band_bps < 0.0 || row.size_usd <= 0.0 {
            anyhow::bail!("invalid bot position band/size");
        }
        if let Some(b) = row.baseline_bps {
            if !b.is_finite() {
                anyhow::bail!("non-finite bot position baseline");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO radar_bot_positions
                 (symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps,
                  baseline_bps, friction_band_bps, size_usd, deadline_ms,
                  unwatched_since_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(symbol, buy_venue, sell_venue) DO UPDATE SET
                 entry_ts_ms = ?4, entry_dev_bps = ?5, baseline_bps = ?6,
                 friction_band_bps = ?7, size_usd = ?8, deadline_ms = ?9,
                 unwatched_since_ms = ?10, last_seen_ms = ?11",
            params![
                symbol,
                buy_venue,
                sell_venue,
                row.entry_ts_ms,
                row.entry_dev_bps,
                row.baseline_bps,
                row.friction_band_bps,
                row.size_usd,
                row.deadline_ms,
                row.unwatched_since_ms,
                row.last_seen_ms
            ],
        )?;
        Ok(())
    }

    pub fn delete_bot_position(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM radar_bot_positions WHERE symbol = ?1 AND buy_venue = ?2 AND sell_venue = ?3",
            params![symbol, buy_venue, sell_venue],
        )?;
        Ok(())
    }

    pub fn list_bot_positions(&self) -> anyhow::Result<Vec<BotPositionRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps,
                    baseline_bps, friction_band_bps, size_usd, deadline_ms,
                    unwatched_since_ms, last_seen_ms
             FROM radar_bot_positions ORDER BY entry_ts_ms, symbol",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(BotPositionRow {
                symbol: r.get(0)?,
                buy_venue: r.get(1)?,
                sell_venue: r.get(2)?,
                entry_ts_ms: r.get(3)?,
                entry_dev_bps: r.get(4)?,
                baseline_bps: r.get(5)?,
                friction_band_bps: r.get(6)?,
                size_usd: r.get(7)?,
                deadline_ms: r.get(8)?,
                unwatched_since_ms: r.get(9)?,
                last_seen_ms: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn record_spatial_baseline(&self, row: &SpatialEntryBaseline) -> anyhow::Result<()> {
        let signal_id = normalize_id(&row.signal_id)?;
        let symbol = normalize_symbol(&row.symbol)?;
        let buy_venue = normalize_venue(&row.buy_venue)?;
        let sell_venue = normalize_venue(&row.sell_venue)?;
        if !row.entry_net_bps.is_finite() {
            anyhow::bail!("non-finite spatial entry edge");
        }
        if let Some(b) = row.baseline_bps {
            if !b.is_finite() {
                anyhow::bail!("non-finite spatial entry baseline");
            }
        }
        if row.entry_ts_ms < 0 {
            anyhow::bail!("negative spatial entry timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO spatial_entry_baseline
                 (signal_id, symbol, buy_venue, sell_venue,
                  entry_net_bps, baseline_bps, entry_ts_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(signal_id) DO UPDATE SET
                 symbol = ?2, buy_venue = ?3, sell_venue = ?4,
                 entry_net_bps = ?5, baseline_bps = ?6, entry_ts_ms = ?7",
            params![
                signal_id,
                symbol,
                buy_venue,
                sell_venue,
                row.entry_net_bps,
                row.baseline_bps,
                row.entry_ts_ms
            ],
        )?;
        Ok(())
    }

    pub fn spatial_baseline_for_signal(
        &self,
        signal_id: &str,
    ) -> anyhow::Result<Option<SpatialEntryBaseline>> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT signal_id, symbol, buy_venue, sell_venue,
                    entry_net_bps, baseline_bps, entry_ts_ms
             FROM spatial_entry_baseline WHERE signal_id = ?1",
            [&signal_id],
            |r| {
                Ok(SpatialEntryBaseline {
                    signal_id: r.get(0)?,
                    symbol: r.get(1)?,
                    buy_venue: r.get(2)?,
                    sell_venue: r.get(3)?,
                    entry_net_bps: r.get(4)?,
                    baseline_bps: r.get(5)?,
                    entry_ts_ms: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
    }

    /// Upsert one pair signal's lifecycle state.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_monitor_signal(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        status: &str,
        first_seen_ms: i64,
        last_seen_ms: i64,
        last_edge_bps: Option<f64>,
        last_dev_bps: Option<f64>,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        match status {
            "preparing" | "tracking" | "triggered" | "ready" | "no-data" => {}
            other => anyhow::bail!("invalid monitor signal status '{other}'"),
        }
        if first_seen_ms < 0 || last_seen_ms < 0 {
            anyhow::bail!("negative signal timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_signals
                 (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
                  last_edge_bps, last_dev_bps, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(symbol, buy_venue, sell_venue) DO UPDATE SET
                 status = ?4, first_seen_ms = ?5, last_seen_ms = ?6,
                 last_edge_bps = ?7, last_dev_bps = ?8, updated_at = ?9",
            params![
                symbol,
                buy_venue,
                sell_venue,
                status,
                first_seen_ms,
                last_seen_ms,
                last_edge_bps,
                last_dev_bps,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Load every persisted pair signal's lifecycle state.
    pub fn monitor_signals(&self) -> anyhow::Result<Vec<MonitorSignalRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
                    last_edge_bps, last_dev_bps
             FROM monitor_signals",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MonitorSignalRow {
                symbol: r.get(0)?,
                buy_venue: r.get(1)?,
                sell_venue: r.get(2)?,
                status: r.get(3)?,
                first_seen_ms: r.get(4)?,
                last_seen_ms: r.get(5)?,
                last_edge_bps: r.get(6)?,
                last_dev_bps: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Drop signal rows whose pair is no longer tracked (venue or symbol
    /// left the watchlist) so stale lifecycle state can't resurface.
    pub fn prune_monitor_signals(&self, keep: &[(String, String, String)]) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        let mut all: Vec<(String, String, String)> = {
            let mut s =
                conn.prepare("SELECT symbol, buy_venue, sell_venue FROM monitor_signals")?;
            let rows = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        all.retain(|k| {
            !keep
                .iter()
                .any(|(s, b, sv)| s == &k.0 && b == &k.1 && sv == &k.2)
        });
        let mut del = conn.prepare(
            "DELETE FROM monitor_signals WHERE symbol = ?1 AND buy_venue = ?2 AND sell_venue = ?3",
        )?;
        for k in all {
            del.execute(params![k.0, k.1, k.2])?;
        }
        Ok(())
    }

    /// Append one lifecycle audit event. `old_status` is `None` on first
    /// sighting; `trigger` is `transition` when the status changed and
    /// `heartbeat` for the minute-bucketed still-active re-assertion.
    /// Non-finite metric snapshots are rejected rather than persisted as
    /// plausible numbers.
    #[allow(clippy::too_many_arguments)]
    pub fn record_monitor_signal_event(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        ts_ms: i64,
        old_status: Option<&str>,
        new_status: &str,
        edge_bps: Option<f64>,
        dev_bps: Option<f64>,
        p_win: Option<f64>,
        economics_json: Option<&str>,
        trigger: &str,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        if ts_ms < 0 {
            anyhow::bail!("negative signal event timestamp");
        }
        const LIFECYCLE: [&str; 5] = ["preparing", "tracking", "triggered", "ready", "no-data"];
        const DECISIONS: [&str; 3] = ["bot_enter", "bot_skip", "bot_exit"];
        if let Some(old) = old_status {
            if !LIFECYCLE.contains(&old) {
                anyhow::bail!("invalid old monitor signal status '{old}'");
            }
        }
        if !(LIFECYCLE.contains(&new_status) || DECISIONS.contains(&new_status)) {
            anyhow::bail!("invalid new monitor signal status '{new_status}'");
        }
        if !matches!(trigger, "transition" | "heartbeat" | "bot_decision") {
            anyhow::bail!("invalid signal event trigger '{trigger}'");
        }
        for (name, v) in [("edge", edge_bps), ("dev", dev_bps), ("p_win", p_win)] {
            if let Some(x) = v {
                if !x.is_finite() {
                    anyhow::bail!("non-finite signal event {name}");
                }
            }
        }
        if let Some(p) = p_win {
            if !(0.0..=1.0).contains(&p) {
                anyhow::bail!("signal event p_win outside 0..=1");
            }
        }
        if let Some(j) = economics_json {
            if j.len() > 8192 {
                anyhow::bail!("signal event economics snapshot too large");
            }
            if !j.is_empty() && serde_json::from_str::<serde_json::Value>(j).is_err() {
                anyhow::bail!("signal event economics snapshot is not valid JSON");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_signal_events
                 (symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
                  edge_bps, dev_bps, p_win, economics_json, trigger)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                symbol,
                buy_venue,
                sell_venue,
                ts_ms,
                old_status,
                new_status,
                edge_bps,
                dev_bps,
                p_win,
                economics_json,
                trigger
            ],
        )?;
        Ok(())
    }

    /// Recent audit events for one pair (or every pair when the key is
    /// `None`), newest first, bounded by `limit`. Fail-closed read path for
    /// the future bot + debugging IPC.
    pub fn recent_monitor_signal_events(
        &self,
        symbol: Option<&str>,
        buy_venue: Option<&str>,
        sell_venue: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<MonitorSignalEventRow>> {
        if limit <= 0 {
            anyhow::bail!("event limit must be positive");
        }
        let limit = limit.min(10_000);
        let mut sql = String::from(
            "SELECT id, symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
                    edge_bps, dev_bps, p_win, economics_json, trigger
             FROM monitor_signal_events WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        if let Some(s) = symbol {
            sql.push_str(" AND symbol = ?");
            binds.push(normalize_symbol(s)?);
        }
        if let Some(b) = buy_venue {
            sql.push_str(" AND buy_venue = ?");
            binds.push(normalize_venue(b)?);
        }
        if let Some(s) = sell_venue {
            sql.push_str(" AND sell_venue = ?");
            binds.push(normalize_venue(s)?);
        }
        sql.push_str(" ORDER BY ts_ms DESC, id DESC LIMIT ?");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds
            .iter()
            .map(|b| b as &dyn rusqlite::types::ToSql)
            .collect();
        let mut all_refs: Vec<&dyn rusqlite::types::ToSql> = refs;
        all_refs.push(&limit);
        let rows = stmt.query_map(rusqlite::params_from_iter(all_refs), |r| {
            Ok(MonitorSignalEventRow {
                id: r.get(0)?,
                symbol: r.get(1)?,
                buy_venue: r.get(2)?,
                sell_venue: r.get(3)?,
                ts_ms: r.get(4)?,
                old_status: r.get(5)?,
                new_status: r.get(6)?,
                edge_bps: r.get(7)?,
                dev_bps: r.get(8)?,
                p_win: r.get(9)?,
                economics_json: r.get(10)?,
                trigger: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn monitor_signal_event_count(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM monitor_signal_events", [], |r| {
            r.get(0)
        })?;
        Ok(n.max(0) as u64)
    }

    /// Bound the audit log with the shared spread-sample day cutoff plus the
    /// independent `MONITOR_SIGNAL_EVENTS_MAX_ROWS` cap (newest rows survive).
    /// Derived session profiles are never touched here.
    pub fn prune_monitor_signal_events(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM monitor_signal_events WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM monitor_signal_events WHERE id IN (
                 SELECT id FROM monitor_signal_events
                 ORDER BY ts_ms DESC, id DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }
}
