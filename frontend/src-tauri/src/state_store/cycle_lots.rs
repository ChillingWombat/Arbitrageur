//! Durable virtual-cycle-ledger persistence (schema v30).
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.
//!
//! Cycle lots were in-memory only: a restart dropped every open lot, so
//! post-restart flattens found nothing to close (realized PnL, prediction
//! resolution, and the cumulative counter all silently skipped) and
//! `compute_venue_net_exposure`/`compute_strategy_metrics` forgot history.
//! Writers upsert the whole lot row on every mutation (open, fee refresh,
//! close); boot hydrates the full set — open lots resume supervision and
//! closed lots keep metrics continuity.

use super::*;
use arbitrageur_execution::virtual_ledger::{ArbitrageCycleLot, CycleStatus};

fn status_str(s: CycleStatus) -> &'static str {
    match s {
        CycleStatus::Open => "open",
        CycleStatus::Closed => "closed",
        CycleStatus::StoppedOut => "stopped_out",
        CycleStatus::TimedOut => "timed_out",
    }
}

fn status_from(raw: &str) -> Option<CycleStatus> {
    match raw {
        "open" => Some(CycleStatus::Open),
        "closed" => Some(CycleStatus::Closed),
        "stopped_out" => Some(CycleStatus::StoppedOut),
        "timed_out" => Some(CycleStatus::TimedOut),
        _ => None,
    }
}

impl StateStore {
    /// Upsert one cycle lot. Called on open, on the pre-close fee refresh,
    /// and on close — the row always mirrors the in-memory lot.
    pub fn upsert_cycle_lot(&self, lot: &ArbitrageCycleLot) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO cycle_lots \
             (id, strategy_id, symbol, venue_long, venue_short, entry_time, \
              entry_spread_bps, long_qty, long_entry_price, short_qty, \
              short_entry_price, fees_paid_usd, status, close_time, \
              close_spread_bps, realized_pnl_usd, realized_pnl_bps) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                lot.id,
                lot.strategy_id,
                lot.symbol,
                lot.venue_long,
                lot.venue_short,
                lot.entry_time.to_rfc3339(),
                lot.entry_spread_bps,
                lot.long_qty,
                lot.long_entry_price,
                lot.short_qty,
                lot.short_entry_price,
                lot.fees_paid_usd,
                status_str(lot.status),
                lot.close_time.map(|t| t.to_rfc3339()),
                lot.close_spread_bps,
                lot.realized_pnl_usd,
                lot.realized_pnl_bps,
            ],
        )?;
        Ok(())
    }

    /// Hydrate all persisted lots. Rows with an unparseable status or
    /// timestamp are skipped rather than failing startup — a corrupt row
    /// costs one lot, never the whole ledger.
    pub fn load_cycle_lots(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, ArbitrageCycleLot>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, strategy_id, symbol, venue_long, venue_short, entry_time, \
                    entry_spread_bps, long_qty, long_entry_price, short_qty, \
                    short_entry_price, fees_paid_usd, status, close_time, \
                    close_spread_bps, realized_pnl_usd, realized_pnl_bps \
             FROM cycle_lots",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, f64>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, f64>(8)?,
                r.get::<_, f64>(9)?,
                r.get::<_, f64>(10)?,
                r.get::<_, f64>(11)?,
                r.get::<_, String>(12)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, Option<f64>>(14)?,
                r.get::<_, Option<f64>>(15)?,
                r.get::<_, Option<f64>>(16)?,
            ))
        })?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (
                id,
                strategy_id,
                symbol,
                venue_long,
                venue_short,
                entry_time,
                entry_spread_bps,
                long_qty,
                long_entry_price,
                short_qty,
                short_entry_price,
                fees_paid_usd,
                status_raw,
                close_time,
                close_spread_bps,
                realized_pnl_usd,
                realized_pnl_bps,
            ) = row?;
            let (Some(status), Some(entry)) = (
                status_from(&status_raw),
                chrono::DateTime::parse_from_rfc3339(&entry_time)
                    .ok()
                    .map(|t| t.with_timezone(&chrono::Utc)),
            ) else {
                continue;
            };
            let close = close_time.and_then(|t| {
                chrono::DateTime::parse_from_rfc3339(&t)
                    .ok()
                    .map(|t| t.with_timezone(&chrono::Utc))
            });
            out.insert(
                id.clone(),
                ArbitrageCycleLot {
                    id,
                    strategy_id,
                    symbol,
                    venue_long,
                    venue_short,
                    entry_time: entry,
                    entry_spread_bps,
                    long_qty,
                    long_entry_price,
                    short_qty,
                    short_entry_price,
                    fees_paid_usd,
                    status,
                    close_time: close,
                    close_spread_bps,
                    realized_pnl_usd,
                    realized_pnl_bps,
                },
            );
        }
        Ok(out)
    }
}
