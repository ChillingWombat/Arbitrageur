//! Scheduled-exit queue.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Insert a pending scheduled exit. Ids are caller-minted
    /// (`sched-exit-*`); inserting an existing id fails on the PK.
    pub fn insert_scheduled_exit(&self, exit: &ScheduledExit) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO scheduled_exits
                (id, venue, symbol, side, close_pct, mode, limit_price,
                 execute_at_ms, status, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                exit.id,
                exit.venue,
                exit.symbol,
                exit.side,
                exit.close_pct,
                exit.mode,
                exit.limit_price,
                exit.execute_at_ms,
                exit.status,
                exit.note,
                exit.created_at,
                exit.updated_at,
            ],
        )?;
        Ok(())
    }

    /// List scheduled exits, newest first. `status` filters (e.g.
    /// "pending" for the scheduler, None for the full audit view).
    pub fn list_scheduled_exits(&self, status: Option<&str>) -> anyhow::Result<Vec<ScheduledExit>> {
        let conn = self.conn.lock();
        let sql = "SELECT id, venue, symbol, side, close_pct, mode, limit_price,
                          execute_at_ms, status, note, created_at, updated_at, attempts
                   FROM scheduled_exits";
        let rows = match status {
            Some(s) => {
                let mut stmt = conn.prepare(&format!(
                    "{sql} WHERE status = ?1 ORDER BY execute_at_ms ASC"
                ))?;
                let r = stmt
                    .query_map([s], scheduled_exit_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(stmt);
                r
            }
            None => {
                let mut stmt = conn.prepare(&format!("{sql} ORDER BY created_at DESC"))?;
                let r = stmt
                    .query_map([], scheduled_exit_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(stmt);
                r
            }
        };
        Ok(rows)
    }

    /// Pending exits due at or before `now_ms`, due-time ascending.
    pub fn due_scheduled_exits(&self, now_ms: i64) -> anyhow::Result<Vec<ScheduledExit>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, venue, symbol, side, close_pct, mode, limit_price,
                    execute_at_ms, status, note, created_at, updated_at, attempts
             FROM scheduled_exits
             WHERE status = 'pending' AND execute_at_ms <= ?1
             ORDER BY execute_at_ms ASC",
        )?;
        let rows = stmt
            .query_map([now_ms], scheduled_exit_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Atomically claim a pending exit for dispatch (pending → firing).
    /// `true` only for the caller that won the transition — a restart or a
    /// duplicate tick can never dispatch the same row twice.
    pub fn claim_scheduled_exit(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE scheduled_exits SET status = 'firing', updated_at = ?2
             WHERE id = ?1 AND status = 'pending'",
            rusqlite::params![id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(n == 1)
    }

    /// Reschedule a claimed ('firing') exit back to pending at a later
    /// due time, bumping its attempt counter — transient-dispatch-failure
    /// backoff. Terminal rows are untouched; `false` when the row is not
    /// in a claimable state.
    pub fn reschedule_scheduled_exit(
        &self,
        id: &str,
        execute_at_ms: i64,
        note: Option<&str>,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE scheduled_exits SET status = 'pending', execute_at_ms = ?2,
                    attempts = attempts + 1, note = COALESCE(?3, note),
                    updated_at = ?4
             WHERE id = ?1 AND status IN ('pending', 'firing')",
            rusqlite::params![id, execute_at_ms, note, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(n == 1)
    }

    /// Transition a scheduled exit's status with a note. Transitions are
    /// one-way: a row already terminal (fired/failed/cancelled) is not
    /// touched, which keeps a restart from re-firing a dispatched exit.
    /// 'firing' rows (claimed mid-dispatch) may still resolve.
    pub fn set_scheduled_exit_status(
        &self,
        id: &str,
        status: &str,
        note: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE scheduled_exits SET status = ?2, note = COALESCE(?3, note),
                    updated_at = ?4
             WHERE id = ?1 AND status IN ('pending', 'firing')",
            rusqlite::params![id, status, note, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}
