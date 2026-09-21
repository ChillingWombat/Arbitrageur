//! AutoTune apply-event audit trail.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    pub fn record_autotune_apply_event(
        &self,
        symbol: &str,
        applied_json: &str,
        detail_json: &str,
        ts_ms: i64,
    ) -> anyhow::Result<i64> {
        let symbol = normalize_symbol(symbol)?;
        if ts_ms < 0 {
            anyhow::bail!("negative autotune apply timestamp");
        }
        for (name, j) in [("applied", applied_json), ("detail", detail_json)] {
            if j.len() < 2 || j.len() > 8192 {
                anyhow::bail!("autotune apply {name} snapshot has invalid size");
            }
            if serde_json::from_str::<serde_json::Value>(j).is_err() {
                anyhow::bail!("autotune apply {name} snapshot is not valid JSON");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO autotune_apply_events (symbol, applied_json, detail_json, ts_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![symbol, applied_json, detail_json, ts_ms],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn recent_autotune_apply_events(
        &self,
        symbol: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<AutotuneApplyEventRow>> {
        let symbol = normalize_symbol(symbol)?;
        if limit <= 0 {
            anyhow::bail!("event limit must be positive");
        }
        let limit = limit.min(1000);
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, symbol, applied_json, detail_json, ts_ms
             FROM autotune_apply_events WHERE symbol IN ({placeholders})
             ORDER BY ts_ms DESC, id DESC LIMIT ?"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut binds: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        binds.push(&limit);
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
            Ok(AutotuneApplyEventRow {
                id: r.get(0)?,
                symbol: r.get(1)?,
                applied_json: r.get(2)?,
                detail_json: r.get(3)?,
                ts_ms: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
