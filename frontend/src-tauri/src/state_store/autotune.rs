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
        environment: &str,
    ) -> anyhow::Result<i64> {
        self.record_autotune_apply_event_mode(
            symbol,
            applied_json,
            detail_json,
            ts_ms,
            true,
            environment,
        )
    }

    /// Mode-aware variant: the audit row keys on the active policy's pair
    /// key so reads under `separate` stay quote-strict. `environment` is
    /// part of the row identity — a testnet apply event never surfaces in
    /// the live audit trail.
    pub fn record_autotune_apply_event_mode(
        &self,
        symbol: &str,
        applied_json: &str,
        detail_json: &str,
        ts_ms: i64,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<i64> {
        let symbol = arbitrageur_core::symbol::pair_key(symbol, merge_usd);
        let symbol = normalize_symbol(&symbol)?;
        let environment = normalize_environment(environment)?;
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
            "INSERT INTO autotune_apply_events
                 (symbol, applied_json, detail_json, ts_ms, environment)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![symbol, applied_json, detail_json, ts_ms, environment],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn recent_autotune_apply_events(
        &self,
        symbol: &str,
        limit: i64,
        environment: &str,
    ) -> anyhow::Result<Vec<AutotuneApplyEventRow>> {
        self.recent_autotune_apply_events_mode(symbol, limit, true, environment)
    }

    /// Mode-aware variant: under `separate` the audit trail is quote-strict
    /// — a BTC-USDT apply event never surfaces for a BTC-USDC query.
    /// Reads are environment-strict.
    pub fn recent_autotune_apply_events_mode(
        &self,
        symbol: &str,
        limit: i64,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<Vec<AutotuneApplyEventRow>> {
        let symbol = normalize_symbol(symbol)?;
        let environment = normalize_environment(environment)?;
        if limit <= 0 {
            anyhow::bail!("event limit must be positive");
        }
        let limit = limit.min(1000);
        let keys = crate::state_store::samples::sample_keys(&symbol, merge_usd);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, symbol, applied_json, detail_json, ts_ms, environment
             FROM autotune_apply_events
             WHERE symbol IN ({placeholders}) AND environment = ?
             ORDER BY ts_ms DESC, id DESC LIMIT ?"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut binds: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        binds.push(&environment);
        binds.push(&limit);
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
            Ok(AutotuneApplyEventRow {
                id: r.get(0)?,
                symbol: r.get(1)?,
                applied_json: r.get(2)?,
                detail_json: r.get(3)?,
                ts_ms: r.get(4)?,
                environment: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
