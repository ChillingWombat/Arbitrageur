//! Credential-profile metadata and strategy account routes.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_account(
        &self,
        id: &str,
        venue: &str,
        label: &str,
        enabled: bool,
        make_default: bool,
        account_tier: &str,
        max_leverage: Option<u32>,
    ) -> anyhow::Result<AccountProfile> {
        let id = normalize_id(id)?;
        let venue = normalize_venue(venue)?;
        let label = normalize_label(label)?;
        let account_tier = normalize_account_tier(account_tier)?;
        let max_leverage_i64 = match max_leverage {
            None => None,
            Some(v) if (1..=5).contains(&v) => Some(i64::from(v)),
            Some(v) => {
                anyhow::bail!("max_leverage {v} out of range; must be 1..=5 or unset (unlimited)")
            }
        };
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let existing_venue: Option<String> = tx
            .query_row(
                "SELECT venue FROM exchange_accounts WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing_venue {
            if existing != venue {
                anyhow::bail!("account venue is immutable once created");
            }
        }

        let prior_default: Option<String> = tx
            .query_row(ENABLED_DEFAULT_BY_VENUE_SQL, [&venue], |r| r.get(0))
            .optional()?;

        tx.execute(
            ACCOUNT_UPSERT_SQL,
            params![
                id,
                venue,
                label,
                enabled,
                account_tier,
                now,
                max_leverage_i64
            ],
        )?;

        let chosen: Option<String> = if make_default && enabled {
            Some(id.clone())
        } else {
            let preserved = match prior_default {
                Some(pid) => tx
                    .query_row(ACCOUNT_BY_ID_SQL, [&pid], account_from_row)
                    .optional()?
                    .filter(|a| a.enabled)
                    .map(|a| a.id),
                None => None,
            };
            match preserved {
                Some(pid) => Some(pid),
                None => tx
                    .query_row(FALLBACK_DEFAULT_SQL, [&venue], |r| r.get(0))
                    .optional()?,
            }
        };

        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        if let Some(chosen_id) = chosen {
            let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, chosen_id])?;
            if updated != 1 {
                anyhow::bail!("failed to assign default account for venue {venue}");
            }
        }

        let profile = tx.query_row(ACCOUNT_BY_ID_SQL, [&id], account_from_row)?;
        tx.commit()?;
        Ok(profile)
    }

    pub fn set_default_account(&self, venue: &str, account_id: &str) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        let account_id = normalize_id(account_id)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let row: Option<(String, bool)> = tx
            .query_row(
                "SELECT venue, enabled FROM exchange_accounts WHERE id = ?1",
                [&account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (stored_venue, stored_enabled) =
            row.ok_or_else(|| anyhow::anyhow!("account {account_id} does not exist"))?;
        if stored_venue != venue {
            anyhow::bail!("account {account_id} does not belong to venue {venue}");
        }
        if !stored_enabled {
            anyhow::bail!("disabled account cannot be the venue default");
        }

        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, account_id])?;
        if updated != 1 {
            anyhow::bail!("failed to assign default account for venue {venue}");
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_accounts(&self, venue: Option<&str>) -> anyhow::Result<Vec<AccountProfile>> {
        let conn = self.conn.lock();
        match venue {
            Some(raw) => {
                let venue = normalize_venue(raw)?;
                let mut stmt = conn.prepare(ACCOUNT_BY_VENUE_SQL)?;
                let rows = stmt.query_map([&venue], account_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = conn.prepare(ACCOUNT_ALL_SQL)?;
                let rows = stmt.query_map([], account_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }

    pub fn get_account(&self, account_id: &str) -> anyhow::Result<Option<AccountProfile>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(ACCOUNT_BY_ID_SQL, [&account_id], account_from_row)
            .optional()?)
    }

    pub fn delete_account(&self, account_id: &str) -> anyhow::Result<bool> {
        let account_id = normalize_id(account_id)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let venue: Option<String> = tx
            .query_row(
                "SELECT venue FROM exchange_accounts WHERE id = ?1",
                [&account_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(venue) = venue else {
            return Ok(false);
        };

        let deleted = match tx.execute("DELETE FROM exchange_accounts WHERE id = ?1", [&account_id])
        {
            Ok(n) => n,
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                anyhow::bail!(
                    "account {account_id} is referenced by strategy routes; remove or reassign those routes first"
                );
            }
            Err(err) => return Err(err.into()),
        };
        if deleted == 0 {
            return Ok(false);
        }

        let next_default: Option<String> = tx
            .query_row(FALLBACK_DEFAULT_SQL, [&venue], |r| r.get(0))
            .optional()?;
        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        if let Some(chosen_id) = next_default {
            let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, chosen_id])?;
            if updated != 1 {
                anyhow::bail!("failed to reassign default account for venue {venue}");
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn has_enabled_account(&self, venue: &str) -> anyhow::Result<bool> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
    SELECT 1 FROM exchange_accounts WHERE venue = ?1 AND enabled = 1
)",
            [&venue],
            |r| r.get(0),
        )?;
        Ok(exists)
    }

    pub fn default_account(&self, venue: &str) -> anyhow::Result<Option<AccountProfile>> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at
FROM exchange_accounts
WHERE venue = ?1 AND enabled = 1
ORDER BY is_default DESC, label COLLATE NOCASE, id
LIMIT 1",
                [&venue],
                account_from_row,
            )
            .optional()?)
    }

    pub fn upsert_strategy_route(
        &self,
        id: &str,
        strategy_id: &str,
        label: &str,
        account_a_id: &str,
        account_b_id: &str,
        enabled: bool,
    ) -> anyhow::Result<StrategyAccountRoute> {
        let id = normalize_id(id)?;
        let strategy_id = normalize_strategy_id(strategy_id)?;
        let label = normalize_label(label)?;
        let a_id = normalize_id(account_a_id)?;
        let b_id = normalize_id(account_b_id)?;
        if a_id == b_id {
            anyhow::bail!("route endpoints must reference two different accounts");
        }
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let account_a = tx
            .query_row(ACCOUNT_BY_ID_SQL, [&a_id], account_from_row)
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("route endpoint account {a_id} does not exist"))?;
        let account_b = tx
            .query_row(ACCOUNT_BY_ID_SQL, [&b_id], account_from_row)
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("route endpoint account {b_id} does not exist"))?;

        if account_a.venue == account_b.venue {
            anyhow::bail!("route endpoint accounts must be on different venues");
        }
        if enabled && !(account_a.enabled && account_b.enabled) {
            anyhow::bail!("enabled routes require both endpoint accounts to be enabled");
        }

        let (first, second) = if (account_a.venue.as_str(), account_a.id.as_str())
            <= (account_b.venue.as_str(), account_b.id.as_str())
        {
            (account_a, account_b)
        } else {
            (account_b, account_a)
        };

        let prior_created: Option<String> = tx
            .query_row(
                "SELECT created_at FROM strategy_account_routes WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;

        tx.execute(
            ROUTE_UPSERT_SQL,
            params![id, strategy_id, label, enabled, first.id, second.id, now],
        )?;

        let route = StrategyAccountRoute {
            id,
            strategy_id,
            label,
            enabled,
            account_a_id: first.id,
            account_a_venue: first.venue,
            account_a_label: first.label,
            account_b_id: second.id,
            account_b_venue: second.venue,
            account_b_label: second.label,
            created_at: prior_created.unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        tx.commit()?;
        Ok(route)
    }

    pub fn list_strategy_routes(
        &self,
        strategy_id: Option<&str>,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let conn = self.conn.lock();
        match strategy_id {
            Some(raw) => {
                let sid = normalize_strategy_id(raw)?;
                let mut stmt = conn.prepare(&format!(
                    "{ROUTE_JOIN_SQL}
WHERE r.strategy_id = ?1
ORDER BY r.label COLLATE NOCASE, r.id"
                ))?;
                let rows = stmt.query_map([&sid], route_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "{ROUTE_JOIN_SQL}
ORDER BY r.strategy_id COLLATE NOCASE, r.label COLLATE NOCASE, r.id"
                ))?;
                let rows = stmt.query_map([], route_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }

    pub fn eligible_strategy_routes(
        &self,
        strategy_id: &str,
        venue_a: &str,
        venue_b: &str,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let sid = normalize_strategy_id(strategy_id)?;
        let venue_a = normalize_venue(venue_a)?;
        let venue_b = normalize_venue(venue_b)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{ROUTE_JOIN_SQL}
WHERE r.strategy_id = ?1 AND r.enabled = 1
  AND a.enabled = 1 AND b.enabled = 1
  AND ((a.venue = ?2 AND b.venue = ?3) OR (a.venue = ?3 AND b.venue = ?2))
ORDER BY r.label COLLATE NOCASE, r.id"
        ))?;
        let rows = stmt.query_map(params![sid, venue_a, venue_b], route_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// All strategy routes that reference `account_id` on either leg. Used
    /// to block account deletion with an explanatory error before any secret
    /// material is touched.
    pub fn routes_referencing(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{ROUTE_JOIN_SQL}
WHERE r.account_a_id = ?1 OR r.account_b_id = ?1
ORDER BY r.strategy_id COLLATE NOCASE, r.label COLLATE NOCASE, r.id"
        ))?;
        let rows = stmt.query_map([&account_id], route_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_strategy_route(&self, route_id: &str) -> anyhow::Result<bool> {
        let route_id = normalize_id(route_id)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM strategy_account_routes WHERE id = ?1",
            [&route_id],
        )?;
        tx.commit()?;
        Ok(deleted > 0)
    }
}
