//! Session profiles, hour bins, HAR anchors, promotion state.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Read shrunk session aggregates for a symbol (alias-aware). Missing
    /// sessions simply have no row — callers read that as neutral x1.0.
    pub fn session_profiles(&self, symbol: &str) -> anyhow::Result<Vec<SessionProfileRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
                    rv_bps, win_rate, mean_hold_s, provisional, updated_ts
             FROM session_profiles WHERE symbol IN ({placeholders})
             ORDER BY session_id ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SessionProfileRow {
                symbol: r.get(0)?,
                session_id: r.get(1)?,
                n_episodes: r.get(2)?,
                med_abs_spread_bps: r.get(3)?,
                p90_spread_bps: r.get(4)?,
                rv_bps: r.get(5)?,
                win_rate: r.get(6)?,
                mean_hold_s: r.get(7)?,
                provisional: r.get::<_, i64>(8)? != 0,
                updated_ts: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Read hour-bin modulators for a symbol (alias-aware).
    pub fn session_hour_bins(&self, symbol: &str) -> anyhow::Result<Vec<SessionHourBinRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts
             FROM session_hour_bins WHERE symbol IN ({placeholders})
             ORDER BY hour ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SessionHourBinRow {
                symbol: r.get(0)?,
                hour: r.get(1)?,
                n_obs: r.get(2)?,
                med_spread_bps: r.get(3)?,
                modulator: r.get(4)?,
                provisional: r.get::<_, i64>(5)? != 0,
                updated_ts: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Hourly rollup: rebuild one symbol's session aggregates plus 24h
    /// hour-bin modulators from the trailing spread log. Pair spreads are
    /// aligned with the same 30s gap rule as the live monitor; gaps stay
    /// gaps (no interpolation). Returns profile rows written (0 when the
    /// history cannot support a single pair observation).
    pub fn rollup_session_profiles(
        &self,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<usize> {
        use arbitrageur_math::pair_monitor::{build_pair_series, reversion_stats, PairObservation};
        use arbitrageur_math::session::{
            hour_modulator, hour_of_day_utc, percentile_sorted, session_of, session_stats,
            winsorized_median, SessionId, SESSION_MIN_EPISODES,
        };

        let canonical = canonical_base(symbol);
        if canonical.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        let samples = self.spread_samples(symbol, from_ts_ms, to_ts_ms)?;
        let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for s in &samples {
            if s.mid.is_finite() && s.mid > 0.0 {
                mids_by_venue
                    .entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
        let venues: Vec<String> = mids_by_venue.keys().cloned().collect();
        if venues.len() < 2 {
            return Ok(0);
        }
        let mut all: Vec<PairObservation> = Vec::new();
        for a in &venues {
            for b in &venues {
                if a == b {
                    continue;
                }
                if let (Some(ma), Some(mb)) = (mids_by_venue.get(a), mids_by_venue.get(b)) {
                    all.extend(build_pair_series(ma, mb, 30_000));
                }
            }
        }
        if all.is_empty() {
            return Ok(0);
        }
        // Episode segmentation runs on absolute spreads against the global
        // median magnitude: session profiles calibrate how far the spread
        // stretches per regime and how fast magnitude reverts. Signed
        // directions would cancel across ordered pairs and fuse the whole
        // window into one open episode.
        let mut all_abs: Vec<PairObservation> = all
            .iter()
            .map(|o| PairObservation {
                ts_ms: o.ts_ms,
                spread_bps: o.spread_bps.abs(),
            })
            .collect();
        all_abs.sort_by_key(|o| o.ts_ms);
        let abs_vals: Vec<f64> = all_abs.iter().map(|o| o.spread_bps).collect();
        let global_med = match winsorized_median(&abs_vals) {
            Some(g) if g.is_finite() => g,
            _ => return Ok(0),
        };
        let spreads: Vec<f64> = abs_vals.clone();
        let n_all = spreads.len();
        let sigma_abs = if n_all >= 2 {
            let v = spreads
                .iter()
                .map(|x| (x - global_med).powi(2))
                .sum::<f64>()
                / (n_all - 1) as f64;
            if v.is_finite() && v >= 0.0 {
                v.sqrt()
            } else {
                0.0
            }
        } else {
            0.0
        };
        let have_moments = n_all >= 2 && sigma_abs.is_finite() && sigma_abs > 0.0;

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut written = 0usize;
        // Per-session aggregates first so hour bins can reference them.
        let mut session_shrunk: HashMap<String, f64> = HashMap::new();
        {
            let mut upsert = tx.prepare(
                "INSERT OR REPLACE INTO session_profiles
                 (symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
                  rv_bps, win_rate, mean_hold_s, provisional, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for sess in SessionId::all() {
                let sub: Vec<PairObservation> = all_abs
                    .iter()
                    .filter(|o| session_of(o.ts_ms) == sess)
                    .cloned()
                    .collect();
                let abs_vals: Vec<f64> = sub.iter().map(|o| o.spread_bps).collect();
                let stats = session_stats(&abs_vals, global_med);
                let sid = sess.as_str().to_string();
                if sub.is_empty() {
                    upsert.execute(params![
                        canonical,
                        sid,
                        0,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        1,
                        now_ms
                    ])?;
                    written += 1;
                    continue;
                }
                let (n_episodes, win_rate, mean_hold_s) = if have_moments {
                    let rs = reversion_stats(&sub, global_med, sigma_abs, 2.0, 2.0, 3.0, 1);
                    (rs.episodes as i64, rs.p_win, rs.median_converge_secs)
                } else {
                    (0i64, None, None)
                };
                let p90 = percentile_sorted(
                    &{
                        let mut v = abs_vals.clone();
                        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        v
                    },
                    0.90,
                );
                let rv = if sub.len() >= 2 {
                    let v = sub
                        .iter()
                        .map(|o| (o.spread_bps - global_med).powi(2))
                        .sum::<f64>()
                        / (sub.len() - 1) as f64;
                    if v.is_finite() && v >= 0.0 {
                        Some(v.sqrt())
                    } else {
                        None
                    }
                } else {
                    None
                };
                let provisional = (n_episodes as usize) < SESSION_MIN_EPISODES;
                if let Some(shrunk) = stats.shrunk_med {
                    if shrunk.is_finite() {
                        session_shrunk.insert(sid.clone(), shrunk);
                    }
                }
                upsert.execute(params![
                    canonical,
                    sid,
                    n_episodes,
                    stats.shrunk_med,
                    p90,
                    rv,
                    win_rate,
                    mean_hold_s,
                    if provisional { 1 } else { 0 },
                    now_ms
                ])?;
                written += 1;
            }
        }
        {
            let mut upsert = tx.prepare(
                "INSERT OR REPLACE INTO session_hour_bins
                 (symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for hour in 0..24i64 {
                let vals: Vec<f64> = all_abs
                    .iter()
                    .filter(|o| hour_of_day_utc(o.ts_ms) == Some(hour as u32))
                    .map(|o| o.spread_bps)
                    .collect();
                let med = winsorized_median(&vals);
                let session_of_hour = SessionId::all().into_iter().find(|s| {
                    let open = s.open_hour_utc() as i64;
                    let next = match s {
                        SessionId::Asia => 8,
                        SessionId::Europe => 13,
                        SessionId::Overlap => 17,
                        SessionId::UsLate => 22,
                        SessionId::Dead => 24,
                    };
                    hour >= open && hour < next
                });
                let modulator = match (
                    med,
                    session_of_hour
                        .as_ref()
                        .and_then(|s| session_shrunk.get(s.as_str())),
                ) {
                    (Some(m), Some(sm)) => hour_modulator(m, *sm),
                    _ => 1.0,
                };
                let provisional = vals.len() < SESSION_MIN_EPISODES;
                let modulator = if provisional { 1.0 } else { modulator };
                upsert.execute(params![
                    canonical,
                    hour,
                    vals.len() as i64,
                    med,
                    modulator,
                    if provisional { 1 } else { 0 },
                    now_ms
                ])?;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    pub fn phase2_har_anchor(&self, symbol: &str) -> anyhow::Result<Option<HarAnchorRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
                    forecast_rv_bps, trailing_mean_rv_bps, updated_ts
             FROM har_anchors WHERE symbol IN ({placeholders}) LIMIT 1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let row = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(HarAnchorRow {
                    symbol: r.get(0)?,
                    intercept: r.get(1)?,
                    b_daily: r.get(2)?,
                    b_weekly: r.get(3)?,
                    b_monthly: r.get(4)?,
                    r2: r.get(5)?,
                    n_days: r.get(6)?,
                    forecast_rv_bps: r.get(7)?,
                    trailing_mean_rv_bps: r.get(8)?,
                    updated_ts: r.get(9)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
    }

    pub fn phase2_upsert_har_anchor(&self, row: &HarAnchorRow) -> anyhow::Result<()> {
        let symbol = canonical_base(&row.symbol);
        if symbol.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        for (name, v) in [
            ("intercept", row.intercept),
            ("b_daily", row.b_daily),
            ("b_weekly", row.b_weekly),
            ("b_monthly", row.b_monthly),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite HAR anchor {name}");
            }
        }
        if !(0.0..=1.0).contains(&row.r2) || !row.r2.is_finite() {
            anyhow::bail!("HAR anchor r2 outside 0..=1");
        }
        if row.n_days < 0 || row.updated_ts < 0 {
            anyhow::bail!("negative HAR anchor day count or timestamp");
        }
        if !(row.forecast_rv_bps.is_finite()
            && row.forecast_rv_bps > 0.0
            && row.trailing_mean_rv_bps.is_finite()
            && row.trailing_mean_rv_bps > 0.0)
        {
            anyhow::bail!("non-positive HAR anchor forecast or trailing mean");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO har_anchors
                 (symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
                  forecast_rv_bps, trailing_mean_rv_bps, updated_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(symbol) DO UPDATE SET
                 intercept = ?2, b_daily = ?3, b_weekly = ?4, b_monthly = ?5,
                 r2 = ?6, n_days = ?7, forecast_rv_bps = ?8,
                 trailing_mean_rv_bps = ?9, updated_ts = ?10",
            params![
                symbol,
                row.intercept,
                row.b_daily,
                row.b_weekly,
                row.b_monthly,
                row.r2,
                row.n_days,
                row.forecast_rv_bps,
                row.trailing_mean_rv_bps,
                row.updated_ts
            ],
        )?;
        Ok(())
    }

    pub fn phase2_promotion_state(
        &self,
        symbol: &str,
    ) -> anyhow::Result<Option<PromotionStateRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts
             FROM promotion_state WHERE symbol IN ({placeholders}) LIMIT 1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let row = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(PromotionStateRow {
                    symbol: r.get(0)?,
                    status: r.get(1)?,
                    fit_delta_bps: r.get(2)?,
                    val_delta_bps: r.get(3)?,
                    reason: r.get(4)?,
                    updated_ts: r.get(5)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
    }

    pub fn phase2_upsert_promotion_state(&self, row: &PromotionStateRow) -> anyhow::Result<()> {
        let symbol = canonical_base(&row.symbol);
        if symbol.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        if !matches!(row.status.as_str(), "promoted" | "held" | "demoted") {
            anyhow::bail!("invalid promotion status '{}'", row.status);
        }
        for (name, v) in [
            ("fit_delta", row.fit_delta_bps),
            ("val_delta", row.val_delta_bps),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite promotion {name}");
            }
        }
        if row.reason.is_empty() || row.reason.len() > 512 || row.updated_ts < 0 {
            anyhow::bail!("invalid promotion reason or timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO promotion_state
                 (symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(symbol) DO UPDATE SET
                 status = ?2, fit_delta_bps = ?3, val_delta_bps = ?4,
                 reason = ?5, updated_ts = ?6",
            params![
                symbol,
                row.status,
                row.fit_delta_bps,
                row.val_delta_bps,
                row.reason,
                row.updated_ts
            ],
        )?;
        Ok(())
    }
}
