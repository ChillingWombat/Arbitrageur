//! Spread/volume/funding-rate samples plus retention prunes.
//!
//! `impl StateStore` continuation — see `mod.rs` for the struct,
//! schema, and migrations. Methods stay `pub` on the store.

use super::*;

impl StateStore {
    /// Record one poll's per-venue top-of-book observations in a single
    /// transaction. Rows with non-positive/non-finite mids are dropped rather
    /// than persisted as plausible-looking numbers. `environment` is the
    /// producing environment's tag — a testnet print must never land in
    /// the live series (and vice versa).
    pub fn record_spread_samples(
        &self,
        symbol: &str,
        ts_ms: i64,
        entries: &[VenueSampleEntry],
        environment: &str,
    ) -> anyhow::Result<()> {
        self.record_spread_samples_mode(symbol, true, ts_ms, entries, environment)
    }

    /// Mode-aware variant of [`Self::record_spread_samples`]: under the
    /// `separate` USD-quote policy (`merge_usd = false`) a quoted key like
    /// "BTC-USDT" is its own instrument — the same-instrument anchor must
    /// not consult sibling quote spellings.
    pub fn record_spread_samples_mode(
        &self,
        symbol: &str,
        merge_usd: bool,
        ts_ms: i64,
        entries: &[VenueSampleEntry],
        environment: &str,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let environment = normalize_environment(environment)?;
        if entries.is_empty() || ts_ms < 0 {
            return Ok(());
        }
        let mut conn = self.telemetry().lock();
        // Same-instrument gate: once a key has an established scale, rows
        // irreconcilable with it are a symbol collision at the source —
        // refuse to persist them at all. A fresh key has no incumbent yet,
        // so its first cells still bind the instrument freely. The anchor
        // is environment-scoped: a testnet scale must not arbitrate live
        // rows (and pre-v38 'unknown' rows arbitrate nothing).
        let anchor = incumbent_anchor(&conn, &sample_keys(&symbol, merge_usd), &environment)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO spread_samples
                    (symbol, venue_id, ts_ms, mid, best_bid, best_ask, bid_depth, ask_depth, environment)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for e in entries {
                let venue_id = normalize_venue(&e.venue_id)?;
                if !e.mid.is_finite() || e.mid <= 0.0 {
                    continue;
                }
                // Crossed quotes produce a meaningless mid — drop rather than
                // persist a plausible-looking number (upstream validators
                // already reject crossed books; this is the last line).
                if e.best_ask.is_finite() && e.best_bid.is_finite() && e.best_ask < e.best_bid {
                    continue;
                }
                if let Some(a) = anchor {
                    if !within_consensus(e.mid, a) {
                        tracing::warn!(
                            "spread_samples: dropping {symbol} row for {venue_id} — mid {} vs incumbent {a} (symbol collision)",
                            e.mid
                        );
                        continue;
                    }
                }
                stmt.execute(params![
                    symbol,
                    venue_id,
                    ts_ms,
                    e.mid,
                    e.best_bid,
                    e.best_ask,
                    e.bid_depth.filter(|d| d.is_finite() && *d > 0.0),
                    e.ask_depth.filter(|d| d.is_finite() && *d > 0.0),
                    environment,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Minimum interval between persisted frames per (symbol, venue). WS
    /// feeds can push hundreds of book deltas a second; telemetry is
    /// lossy-at-worst, so excess frames drop here rather than queue on the
    /// telemetry writer. Poll-cadence callers keep `record_spread_samples_mode`.
    pub const SAMPLE_MIN_INTERVAL_MS: i64 = 500;

    /// Frame-rate variant of [`Self::record_spread_samples_mode`]: entries
    /// whose (symbol, venue, environment) tuple was persisted within
    /// [`SAMPLE_MIN_INTERVAL_MS`](Self::SAMPLE_MIN_INTERVAL_MS) are dropped
    /// before the write is even attempted.
    pub fn record_spread_samples_throttled(
        &self,
        symbol: &str,
        merge_usd: bool,
        ts_ms: i64,
        entries: &[VenueSampleEntry],
        environment: &str,
    ) -> anyhow::Result<()> {
        let kept: Vec<VenueSampleEntry> = {
            let mut wm = self.sample_watermarks.lock();
            entries
                .iter()
                .filter(|e| {
                    let key = (
                        symbol.to_string(),
                        e.venue_id.clone(),
                        environment.to_string(),
                    );
                    if wm.get(&key).is_some_and(|last| {
                        ts_ms.saturating_sub(*last) < Self::SAMPLE_MIN_INTERVAL_MS
                    }) {
                        return false;
                    }
                    wm.insert(key, ts_ms);
                    true
                })
                .cloned()
                .collect()
        };
        self.record_spread_samples_mode(symbol, merge_usd, ts_ms, &kept, environment)
    }

    /// Samples for a symbol in `[from_ts_ms, to_ts_ms]`, ordered by time then venue.
    /// Matches every storage alias for the asset (canonical base plus
    /// quoted UI forms) so background and UI recording paths read as one.
    ///
    /// Every read passes through [`filter_symbol_consistency`]: a canonical
    /// key must represent ONE instrument, so samples that are irreconcilable
    /// with the cross-venue consensus (ticker collisions like an index perp
    /// vs a same-named memecoin, or a corrupted feed) are dropped here —
    /// once, for every chart, baseline, and pair evaluation downstream.
    /// Reads are environment-strict: 'unknown' provenance rows (pre-v38)
    /// and the other environment's rows never mix into this series.
    pub fn spread_samples(
        &self,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        environment: &str,
    ) -> anyhow::Result<Vec<SpreadSample>> {
        self.spread_samples_mode(symbol, true, from_ts_ms, to_ts_ms, environment)
    }

    /// Mode-aware variant of [`Self::spread_samples`]: `merge_usd = false`
    /// (the `separate` USD-quote policy) reads only the exact key spelling —
    /// "BTC-USDT" never sees "BTC-USDC" or bare "BTC" rows.
    pub fn spread_samples_mode(
        &self,
        symbol: &str,
        merge_usd: bool,
        from_ts_ms: i64,
        to_ts_ms: i64,
        environment: &str,
    ) -> anyhow::Result<Vec<SpreadSample>> {
        let symbol = normalize_symbol(symbol)?;
        let environment = normalize_environment(environment)?;
        let keys = sample_keys(&symbol, merge_usd);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, venue_id, ts_ms, mid, best_bid, best_ask, bid_depth, ask_depth
             FROM spread_samples
             WHERE symbol IN ({placeholders}) AND ts_ms >= ? AND ts_ms <= ?
               AND environment = ?
             ORDER BY ts_ms ASC, venue_id ASC"
        );
        let conn = self.telemetry().lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        params.push(&from_ts_ms);
        params.push(&to_ts_ms);
        params.push(&environment);
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SpreadSample {
                symbol: r.get(0)?,
                venue_id: r.get(1)?,
                ts_ms: r.get(2)?,
                mid: r.get(3)?,
                best_bid: r.get(4)?,
                best_ask: r.get(5)?,
                bid_depth: r.get(6).ok().flatten(),
                ask_depth: r.get(7).ok().flatten(),
            })
        })?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        // One anchor source for every read: the scale the canonical key was
        // first bound to *in this environment*. A window of pure intruder
        // data reads self-consistent at the wrong scale, so the incumbent
        // must arbitrate even when nothing inside the window looks split.
        let anchor = incumbent_anchor(&conn, &keys, &environment)?;
        Ok(filter_symbol_consistency(rows, anchor))
    }

    /// Earliest recorded sample timestamp for a symbol (history depth indicator).
    /// Alias-aware, matching [`Self::spread_samples`].
    pub fn earliest_spread_sample_ts(
        &self,
        symbol: &str,
        environment: &str,
    ) -> anyhow::Result<Option<i64>> {
        self.earliest_spread_sample_ts_mode(symbol, true, environment)
    }

    /// Mode-aware variant matching [`Self::spread_samples_mode`].
    pub fn earliest_spread_sample_ts_mode(
        &self,
        symbol: &str,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<Option<i64>> {
        let symbol = normalize_symbol(symbol)?;
        let environment = normalize_environment(environment)?;
        let keys = sample_keys(&symbol, merge_usd);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT MIN(ts_ms) FROM spread_samples
             WHERE symbol IN ({placeholders}) AND environment = ?"
        );
        let conn = self.telemetry().lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        params.push(&environment);
        Ok(stmt.query_row(rusqlite::params_from_iter(params), |r| r.get(0))?)
    }

    /// Global retained-history stats: row count plus oldest/newest sample
    /// timestamps across every symbol and venue. One table scan; the caller
    /// decides how often that is affordable.
    pub fn spread_samples_stats(&self) -> anyhow::Result<(i64, Option<i64>, Option<i64>)> {
        let conn = self.telemetry().lock();
        Ok(conn.query_row(
            "SELECT COUNT(*), MIN(ts_ms), MAX(ts_ms) FROM spread_samples",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    /// Drop samples older than `retain_after_ts_ms`, then enforce a global row
    /// cap by evicting the oldest rows first.
    pub fn prune_spread_samples(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.telemetry().lock();
        conn.execute(
            "DELETE FROM spread_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM spread_samples WHERE id IN (
                 SELECT id FROM spread_samples ORDER BY id DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }

    /* ------------------------------------------------------------------ */
    /* Session seasonality (Phase 1): volume sampler + profiles + rollup    */
    /* ------------------------------------------------------------------ */

    /// Persist one volume-refresh poll per venue, stamped with the
    /// producing environment. Symbols are stored under the canonical base
    /// so reads stay alias-normalized through `spread_symbol_aliases`.
    /// Best-effort shape: bad rows are skipped, never persisted as
    /// plausible numbers.
    pub fn record_venue_volume_samples(
        &self,
        venue: &str,
        ts_ms: i64,
        volumes: &HashMap<String, f64>,
        environment: &str,
    ) -> anyhow::Result<()> {
        self.record_venue_volume_samples_mode(venue, ts_ms, volumes, true, environment)
    }

    /// Mode-aware variant: under `separate` volume rows keep their pair key
    /// so sibling quotes never share a depth series.
    pub fn record_venue_volume_samples_mode(
        &self,
        venue: &str,
        ts_ms: i64,
        volumes: &HashMap<String, f64>,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        let environment = normalize_environment(environment)?;
        if volumes.is_empty() || ts_ms < 0 {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO venue_volume_samples
                     (venue_id, symbol, ts_ms, vol_usd, environment)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (asset, vol) in volumes {
                if !vol.is_finite() || *vol <= 0.0 {
                    continue;
                }
                let symbol = arbitrageur_core::symbol::pair_key(asset, merge_usd);
                if symbol.is_empty() {
                    continue;
                }
                stmt.execute(params![venue, symbol, ts_ms, vol, environment])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Raw volume series for one symbol/venue window in one environment.
    /// Alias-aware like `spread_samples`.
    pub fn venue_volume_series(
        &self,
        symbol: &str,
        venue: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        environment: &str,
    ) -> anyhow::Result<Vec<(i64, f64)>> {
        self.venue_volume_series_mode(symbol, venue, from_ts_ms, to_ts_ms, true, environment)
    }

    /// Mode-aware variant: under `separate` the pair key is the series —
    /// sibling quotes never satisfy the read.
    pub fn venue_volume_series_mode(
        &self,
        symbol: &str,
        venue: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<Vec<(i64, f64)>> {
        let symbol = normalize_symbol(symbol)?;
        let venue = normalize_venue(venue)?;
        let environment = normalize_environment(environment)?;
        let keys = sample_keys(&symbol, merge_usd);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT ts_ms, vol_usd FROM venue_volume_samples
             WHERE venue_id = ? AND symbol IN ({placeholders})
               AND ts_ms >= ? AND ts_ms <= ? AND environment = ?
             ORDER BY ts_ms ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(keys.len() + 4);
        params.push(&venue);
        for k in &keys {
            params.push(k);
        }
        params.push(&from_ts_ms);
        params.push(&to_ts_ms);
        params.push(&environment);
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bound the raw volume log with the same retention cutoffs as spread
    /// samples. Profile and hour-bin tables are never touched here — they
    /// are derived state that survives every raw-sample prune.
    pub fn prune_venue_volume_samples(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM venue_volume_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM venue_volume_samples WHERE rowid IN (
                 SELECT rowid FROM venue_volume_samples
                 ORDER BY rowid DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }

    /// Distinct canonical symbols present in the spread log for one
    /// environment (rollup driver — testnet symbols never seed live
    /// profiles and vice versa).
    pub fn distinct_sample_symbols(&self, environment: &str) -> anyhow::Result<Vec<String>> {
        self.distinct_sample_symbols_mode(true, environment)
    }

    /// Mode-aware variant: under `separate` the stored keys ARE the series
    /// identity — collapsing "BTC-USDT"/"BTC-USDC" to "BTC" would make the
    /// rollup driver read a key with no samples and merge sibling series.
    pub fn distinct_sample_symbols_mode(
        &self,
        merge_usd: bool,
        environment: &str,
    ) -> anyhow::Result<Vec<String>> {
        let environment = normalize_environment(environment)?;
        let conn = self.telemetry().lock();
        let mut stmt =
            conn.prepare("SELECT DISTINCT symbol FROM spread_samples WHERE environment = ?1")?;
        let rows = stmt.query_map([environment], |r| r.get::<_, String>(0))?;
        let mut out: Vec<String> = Vec::new();
        for raw in rows.collect::<rusqlite::Result<Vec<_>>>()? {
            let key = if merge_usd {
                canonical_base(&raw)
            } else {
                raw.trim().to_uppercase()
            };
            if !key.is_empty() && !out.contains(&key) {
                out.push(key);
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn delete_spread_samples_before(&self, retain_after_ts_ms: i64) -> anyhow::Result<usize> {
        let conn = self.telemetry().lock();
        Ok(conn.execute(
            "DELETE FROM spread_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?)
    }

    /// Record funding-rate prints observed by a scan under `environment`.
    /// `samples` carries `(venue_id, symbol, rate_8h_normalized,
    /// interval_hours)`; timestamps are bucketed to the minute so repeat
    /// scans within a minute REPLACE rather than duplicate (PK covers
    /// venue/symbol/minute/environment — a same-minute write from the
    /// other environment is a distinct row, never an overwrite). Fail-soft
    /// on bad rows: they are skipped, never aborting the batch.
    pub fn record_funding_rate_samples(
        &self,
        ts_ms: i64,
        samples: &[(&str, &str, f64, i64)],
        environment: &str,
    ) -> anyhow::Result<()> {
        let environment = normalize_environment(environment)?;
        if ts_ms < 0 || samples.is_empty() {
            return Ok(());
        }
        let bucketed = ts_ms / 60_000 * 60_000;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO funding_rate_samples
                 (venue_id, symbol, ts_ms, rate_8h, interval_hours, environment)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (venue_raw, symbol_raw, rate_8h, interval_hours) in samples {
            let Ok(venue) = normalize_venue(venue_raw) else {
                continue;
            };
            let Ok(symbol) = normalize_symbol(symbol_raw) else {
                continue;
            };
            if !rate_8h.is_finite() || *interval_hours < 1 {
                continue;
            }
            stmt.execute(rusqlite::params![
                venue,
                symbol,
                bucketed,
                rate_8h,
                interval_hours,
                environment
            ])?;
        }
        Ok(())
    }

    /// Ordered `(ts_ms, rate_8h, interval_hours)` history for one
    /// venue+symbol window in one environment. Training substrate for
    /// funding-persistence / half-life modeling; no modeling happens here.
    pub fn funding_rate_series(
        &self,
        venue: &str,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        environment: &str,
    ) -> anyhow::Result<Vec<(i64, f64, i64)>> {
        let venue = normalize_venue(venue)?;
        let symbol = normalize_symbol(symbol)?;
        let environment = normalize_environment(environment)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT ts_ms, rate_8h, interval_hours FROM funding_rate_samples
             WHERE venue_id = ?1 AND symbol = ?2
               AND ts_ms >= ?3 AND ts_ms <= ?4 AND environment = ?5
             ORDER BY ts_ms ASC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![venue, symbol, from_ts_ms, to_ts_ms, environment],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bound the funding-rate log with the same day cutoff the retention
    /// pass computes for spread/volume samples.
    pub fn prune_funding_rate_samples(&self, retain_after_ts_ms: i64) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute(
            "DELETE FROM funding_rate_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?)
    }

    pub fn delete_oldest_spread_samples(&self, limit: u64) -> anyhow::Result<usize> {
        let conn = self.telemetry().lock();
        // Rows are append-only with ts_ms = insert time, so rowid order is
        // timestamp order — ORDER BY id walks the PK directly instead of
        // sorting the whole table per batch under a GB retention cap.
        Ok(conn.execute(
            "DELETE FROM spread_samples WHERE id IN (
                 SELECT id FROM spread_samples ORDER BY id ASC LIMIT ?1
             )",
            [limit as i64],
        )?)
    }

    pub fn spread_sample_count(&self) -> anyhow::Result<u64> {
        let conn = self.telemetry().lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM spread_samples", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }
}

/* ------------------------------------------------------------------------ */
/* Cross-venue sample consistency                                            */
/* ------------------------------------------------------------------------ */

/// Storage keys a sample read/write consults. Merged modes use
/// [`spread_symbol_aliases`] (canonical base + quoted UI forms + index
/// siblings); `separate` mode is quote-strict — only the key's own
/// spellings, so "BTC-USDT" and "BTC-USDC" stay independent series.
pub(crate) fn sample_keys(symbol: &str, merge_usd: bool) -> Vec<String> {
    if merge_usd {
        return spread_symbol_aliases(symbol);
    }
    let mut keys = vec![symbol.to_string()];
    let up = symbol.to_uppercase();
    if !keys.contains(&up) {
        keys.push(up);
    }
    keys
}

/// A canonical symbol must price within this ratio across venues to be the
/// same instrument. Real cross-venue dislocations are basis points; a 3x
/// divergence means the venues are quoting different instruments under one
/// key (e.g. an index perp and a same-named memecoin) or a feed is corrupt.
const CONSISTENCY_MAX_RATIO: f64 = 3.0;
/// Consensus arbitration granularity — coarse enough that every REST lane
/// contributes, fine enough to not smear genuine repricing.
const CONSISTENCY_BUCKET_MS: i64 = 600_000;

fn median_f64(vals: &mut [f64]) -> Option<f64> {
    if vals.is_empty() {
        return None;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = vals.len();
    Some(if n % 2 == 1 {
        vals[n / 2]
    } else {
        (vals[n / 2 - 1] + vals[n / 2]) / 2.0
    })
}

fn within_consensus(mid: f64, consensus: f64) -> bool {
    let r = (mid / consensus).max(consensus / mid);
    r <= CONSISTENCY_MAX_RATIO
}

fn valid_sample(r: &SpreadSample) -> bool {
    r.mid.is_finite() && r.mid > 0.0 && r.ts_ms >= 0
}

/// Per-`(bucket, venue)` medians — one vote per venue per bucket. A hot
/// corrupt feed cannot out-sample the instrument's real scale, and a lane
/// that flipped instruments mid-window (observed: a venue whose `SPX500`
/// binding moved to a same-ticker memecoin, leaving interleaved ~$0.5 and
/// ~$7,700 rows) votes per bucket instead of being smeared into one window
/// median, so its on-scale cells still corroborate the instrument while
/// the collided ones lose.
fn cell_medians<'a>(rows: impl Iterator<Item = (i64, &'a str, f64)>) -> Vec<(f64, usize)> {
    let mut cells: HashMap<(i64, &'a str), Vec<f64>> = HashMap::new();
    for (ts, venue, mid) in rows {
        if mid.is_finite() && mid > 0.0 && ts >= 0 {
            cells
                .entry((ts / CONSISTENCY_BUCKET_MS, venue))
                .or_default()
                .push(mid);
        }
    }
    cells
        .values()
        .filter_map(|ms| median_f64(&mut ms.clone()).map(|m| (m, ms.len())))
        .collect()
}

/// Majority-cluster anchor over cell medians: the scale corroborated by
/// the most cells wins; ties break on covered row count, then the smaller
/// median — deterministic; quote frequency inside a bucket never votes
/// twice.
fn consensus_anchor(cells: &[(f64, usize)]) -> Option<f64> {
    let mut anchor: Option<f64> = None;
    let mut best = (0usize, 0usize);
    for (med, _) in cells {
        let mut supporters = 0usize;
        let mut support_rows = 0usize;
        for (other, n) in cells {
            if within_consensus(*other, *med) {
                supporters += 1;
                support_rows += n;
            }
        }
        let score = (supporters, support_rows);
        let better = match anchor {
            None => true,
            Some(a) => score > best || (score == best && *med < a),
        };
        if better {
            anchor = Some(*med);
            best = score;
        }
    }
    anchor
}

/// Instrument identity is first-bound: a canonical key belongs to the
/// instrument it was created for. Every read anchors on the symbol's
/// earliest `(bucket, venue)` cells — enough to cover the first few
/// buckets across several venues, so one bad first row cannot misbind the
/// key and a later intruder can never accumulate enough samples to steal
/// it. A key born ambiguous still resolves deterministically (earliest
/// majority wins); retention prunes naturally rebind a key only once its
/// original history is gone.
const INCUMBENT_CELL_CAP: usize = 6;
const INCUMBENT_ROW_CAP: i64 = 16_384;

fn incumbent_anchor(
    conn: &Connection,
    keys: &[String],
    environment: &str,
) -> rusqlite::Result<Option<f64>> {
    let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    // Environment-strict: a testnet incumbent must not arbitrate live rows
    // (identical key spaces, unrelated scales).
    let sql = format!(
        "SELECT venue_id, ts_ms, mid FROM spread_samples
         WHERE symbol IN ({placeholders}) AND environment = ?
         ORDER BY ts_ms ASC, venue_id ASC LIMIT {INCUMBENT_ROW_CAP}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<&dyn rusqlite::ToSql> =
        keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
    params.push(&environment);
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, f64>(2)?,
        ))
    })?;
    let mut cells: HashMap<(i64, String), Vec<f64>> = HashMap::new();
    for row in rows {
        let (venue, ts, mid) = row?;
        if mid.is_finite() && mid > 0.0 && ts >= 0 {
            cells
                .entry((ts / CONSISTENCY_BUCKET_MS, venue))
                .or_default()
                .push(mid);
            if cells.len() >= INCUMBENT_CELL_CAP {
                break;
            }
        }
    }
    let meds: Vec<(f64, usize)> = cells
        .values()
        .filter_map(|ms| median_f64(&mut ms.clone()).map(|m| (m, ms.len())))
        .collect();
    Ok(consensus_anchor(&meds))
}

/// Apply the arbitrated anchor: every surviving row must sit within 3x of
/// it, so a bucket whose only quoters are off-scale collapses to an honest
/// gap rather than emitting a fabricated mid. `anchor = None` falls back
/// to the window's own majority (no stored history — the window's earliest
/// cells are the incumbent). The validity gate runs unconditionally: a
/// lone sample must still be a real positive mid at a real timestamp.
fn filter_symbol_consistency(rows: Vec<SpreadSample>, anchor: Option<f64>) -> Vec<SpreadSample> {
    let anchor = anchor.or_else(|| {
        (rows.len() >= 2)
            .then(|| {
                consensus_anchor(&cell_medians(
                    rows.iter().map(|r| (r.ts_ms, r.venue_id.as_str(), r.mid)),
                ))
            })
            .flatten()
    });
    rows.into_iter()
        .filter(|r| match anchor {
            Some(a) => valid_sample(r) && within_consensus(r.mid, a),
            None => valid_sample(r),
        })
        .collect()
}
