//! Execution-log persistence and DB size.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Persist one execution-log row (the Log Monitor source). Successes,
    /// partials, and — since v21 — *failures* alike: a blocked click must
    /// leave the same trace as a fill. Callers treat store errors as
    /// best-effort (a logging failure never fails execution). The table is
    /// capped at the newest 2000 rows.
    pub fn insert_execution_log(&self, e: &ExecutionLogEntry) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO execution_log (id, ts, trade_type, strategy_id, symbol, venue_a, venue_b, leg_a_status, leg_b_status, success, gross_profit, fees, net_profit, latency_us, note, direction, trigger_spread_bps, baseline_mean_spread_bps, position_step, max_units, nominal_hurdle_bps, is_live) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            params![
                e.id, e.timestamp, e.trade_type, e.strategy_id, e.symbol, e.venue_a, e.venue_b,
                e.leg_a_status, e.leg_b_status, e.success as i32, e.gross_profit, e.fees,
                e.net_profit, e.latency_us as i64, e.note, e.direction, e.trigger_spread_bps,
                e.baseline_mean_spread_bps,
                e.position_step.map(|v| v as i64), e.max_units.map(|v| v as i64),
                e.nominal_hurdle_bps, e.is_live as i32,
            ],
        )?;
        conn.execute(
            "DELETE FROM execution_log WHERE rowid NOT IN (SELECT rowid FROM execution_log ORDER BY rowid DESC LIMIT 2000)",
            [],
        )?;
        Ok(())
    }

    /// Newest-first execution-log rows (survives restarts — the monitor no
    /// longer starts empty).
    pub fn load_execution_logs(&self, limit: i64) -> anyhow::Result<Vec<ExecutionLogEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, ts, trade_type, strategy_id, symbol, venue_a, venue_b, leg_a_status, leg_b_status, success, gross_profit, fees, net_profit, latency_us, note, direction, trigger_spread_bps, baseline_mean_spread_bps, position_step, max_units, nominal_hurdle_bps, is_live FROM execution_log ORDER BY rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.max(1)], |r| {
            Ok(ExecutionLogEntry {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                trade_type: r.get(2)?,
                strategy_id: r.get(3)?,
                symbol: r.get(4)?,
                venue_a: r.get(5)?,
                venue_b: r.get(6)?,
                leg_a_status: r.get(7)?,
                leg_b_status: r.get(8)?,
                success: r.get::<_, i64>(9)? != 0,
                gross_profit: r.get(10)?,
                fees: r.get(11)?,
                net_profit: r.get(12)?,
                latency_us: r
                    .get::<_, i64>(13)
                    .ok()
                    .and_then(|v| u64::try_from(v).ok())
                    .unwrap_or(0),
                note: r.get(14)?,
                direction: r.get(15)?,
                trigger_spread_bps: r.get(16)?,
                baseline_mean_spread_bps: r.get(17)?,
                position_step: r
                    .get::<_, Option<i64>>(18)?
                    .and_then(|v| u32::try_from(v).ok()),
                max_units: r
                    .get::<_, Option<i64>>(19)?
                    .and_then(|v| u32::try_from(v).ok()),
                nominal_hurdle_bps: r.get(20)?,
                is_live: r.get::<_, i64>(21)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn db_size_bytes(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok((page_count.max(0) as u64) * (page_size.max(0) as u64))
    }
}
