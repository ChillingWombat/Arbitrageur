//! KV store, strategy params, monitor/radar config, exemptions, venue tiers.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    pub fn upsert_strategy_params_json(
        &self,
        strategy_id: &str,
        params_json: &str,
    ) -> anyhow::Result<()> {
        let id = normalize_id(strategy_id)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO strategy_params (strategy_id, params_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(strategy_id) DO UPDATE SET params_json = ?2, updated_at = ?3",
            params![id, params_json, now],
        )?;
        Ok(())
    }

    pub fn strategy_params_json(&self) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT strategy_id, params_json FROM strategy_params ORDER BY strategy_id ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn kv_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT v FROM kv WHERE k = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        if key.is_empty() || key.len() > 128 {
            anyhow::bail!("invalid kv key");
        }
        if value.len() > 4096 {
            anyhow::bail!("invalid kv value");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = ?2",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM kv WHERE k = ?1", [key])?;
        Ok(())
    }

    /// All keys under a prefix (e.g. `autotune.mode.`), ascending. Lets the
    /// scheduler enumerate configured per-ticker entries without a schema
    /// column or a live venue scan.
    pub fn kv_keys_with_prefix(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT k FROM kv WHERE k LIKE ?1 || '%' ESCAPE '\\' ORDER BY k")?;
        let esc = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows = stmt
            .query_map([esc], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Set or clear a ticker's monitor/strategy exemption. Exempt tickers are
    /// dropped from the spread-monitor watchlist; a row with `exempt = false`
    /// is deleted so the table only ever holds active exemptions.
    pub fn set_ticker_exempt(&self, symbol: &str, exempt: bool) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        if exempt {
            conn.execute(
                "INSERT INTO ticker_exemptions (symbol, exempt, updated_at)
                 VALUES (?1, 1, ?2)
                 ON CONFLICT(symbol) DO UPDATE SET exempt = 1, updated_at = ?2",
                params![symbol, Utc::now().to_rfc3339()],
            )?;
        } else {
            conn.execute("DELETE FROM ticker_exemptions WHERE symbol = ?1", [&symbol])?;
        }
        Ok(())
    }

    /// Symbols currently exempted from the monitor.
    pub fn exempt_tickers(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT symbol FROM ticker_exemptions WHERE exempt = 1 ORDER BY symbol")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Remove every ticker exemption — the global "reveal all" control.
    pub fn clear_ticker_exemptions(&self) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute("DELETE FROM ticker_exemptions", [])?)
    }

    /// Account tiers of enabled accounts on a venue — the venue's shared-IP
    /// rate budget is driven by the lowest tier present (Lighter applies the
    /// lowest limit across accounts on the same IP).
    pub fn venue_enabled_tiers(&self, venue: &str) -> anyhow::Result<Vec<String>> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT account_tier FROM exchange_accounts
             WHERE venue = ?1 AND enabled = 1",
        )?;
        let rows = stmt.query_map([&venue], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /* ------------------------------------------------------------------ */
    /* Spread monitor: persisted config + pair-signal lifecycle            */
    /* ------------------------------------------------------------------ */

    /// Load the monitor config JSON blob, if one was saved.
    pub fn monitor_config_json(&self) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT config_json FROM monitor_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Persist the monitor config JSON blob (single row, upserted).
    pub fn save_monitor_config_json(&self, json: &str) -> anyhow::Result<()> {
        if json.is_empty() || json.len() > 64 * 1024 {
            anyhow::bail!("invalid monitor config payload");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_config (id, config_json, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2",
            params![json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn radar_bot_config_json(&self) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT config_json FROM radar_bot_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_radar_bot_config_json(&self, json: &str) -> anyhow::Result<()> {
        if json.is_empty() || json.len() > 64 * 1024 {
            anyhow::bail!("invalid radar bot config payload");
        }
        if serde_json::from_str::<serde_json::Value>(json).is_err() {
            anyhow::bail!("radar bot config payload is not valid JSON");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO radar_bot_config (id, config_json, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2",
            params![json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}
