//! Session-aware seasonality layer — Phase 1.
//!
//! Pure estimators with no I/O: a UTC session clock, funding-stamp masks,
//! DST-aware US cash hours (Ondo equity gate), empirical-Bayes shrinkage
//! over winsorized medians, and an in-memory EWMA fast-vol tracker.
//!
//! Fail-closed throughout: unknown or provisional inputs degrade to neutral
//! x1.0 (flagged by the caller), never a fabricated multiplier. Gaps stay
//! gaps — no interpolation anywhere in this module.

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

/// Empirical-Bayes prior strength: a session estimate needs this many
/// episodes to carry half weight against the global median.
pub const SHRINK_N0: f64 = 150.0;

/// Below this many episodes a session aggregate is provisional: callers
/// must use neutral x1.0 multipliers and flag the row.
pub const SESSION_MIN_EPISODES: usize = 30;

/// Weekend spread floors are doubled (thin-book compensation).
pub const WEEKEND_FLOOR_MULT: f64 = 2.0;

/// Hour-bin modulators are capped at +/-25% around the session value.
pub const HOUR_MODULATOR_CAP: f64 = 0.25;

/// EWMA smoothing for the 5-minute realized fast-vol layer.
pub const FAST_VOL_LAMBDA: f64 = 0.96;
/// Fast-vol floor/cap (bps of absolute deviation): below the floor the
/// market is treated as flat, above the cap as a data spike, not signal.
pub const FAST_VOL_FLOOR_BPS: f64 = 0.5;
pub const FAST_VOL_CAP_BPS: f64 = 500.0;

/// Fast-vol ratio that halves size on first touch and halts on persistence.
pub const FAST_VOL_HALT_MULT: f64 = 3.0;
/// Current spread beyond this multiple of the session median stands down.
pub const SPREAD_STANDDOWN_MULT: f64 = 3.0;
/// Minimum threshold in bps for spread stand-down to prevent false halts
/// during quiet sessions with tightly compressed medians (e.g. med = 0.8 bps).
pub const SPREAD_STANDDOWN_FLOOR_BPS: f64 = 25.0;

/// Half-width of a funding-stamp stand-down, seconds.
pub const FUNDING_MASK_HALF_WIDTH_SECS: i64 = 120;

/* ------------------------------------------------------------------------ */
/* Session clock                                                             */
/* ------------------------------------------------------------------------ */

/// Intraday UTC regimes. The 13-17 UTC US-EU overlap is the peak window;
/// naive 8h blocks mislabel it, so it is a first-class session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionId {
    /// 00:00-08:00 UTC (thin).
    Asia,
    /// 08:00-13:00 UTC.
    Europe,
    /// 13:00-17:00 UTC (peak).
    Overlap,
    /// 17:00-22:00 UTC.
    UsLate,
    /// 22:00-24:00 UTC (thin).
    Dead,
}

impl SessionId {
    pub fn all() -> [SessionId; 5] {
        [
            SessionId::Asia,
            SessionId::Europe,
            SessionId::Overlap,
            SessionId::UsLate,
            SessionId::Dead,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SessionId::Asia => "asia",
            SessionId::Europe => "europe",
            SessionId::Overlap => "overlap",
            SessionId::UsLate => "us_late",
            SessionId::Dead => "dead",
        }
    }

    pub fn parse(s: &str) -> Option<SessionId> {
        match s.trim().to_lowercase().as_str() {
            "asia" => Some(SessionId::Asia),
            "europe" => Some(SessionId::Europe),
            "overlap" => Some(SessionId::Overlap),
            "us_late" | "us-late" | "uslate" => Some(SessionId::UsLate),
            "dead" => Some(SessionId::Dead),
            _ => None,
        }
    }

    /// UTC hour-of-day (0-23) that opens this session.
    pub fn open_hour_utc(self) -> u32 {
        match self {
            SessionId::Asia => 0,
            SessionId::Europe => 8,
            SessionId::Overlap => 13,
            SessionId::UsLate => 17,
            SessionId::Dead => 22,
        }
    }
}

fn datetime_of(ts_ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ts_ms)
}

/// Session for a millisecond epoch timestamp. Unparseable timestamps fail
/// closed to `Dead` (the most conservative sizing regime), never to peak.
pub fn session_of(ts_ms: i64) -> SessionId {
    let hour = datetime_of(ts_ms).map(|d| d.hour()).unwrap_or(23);
    match hour {
        0..=7 => SessionId::Asia,
        8..=12 => SessionId::Europe,
        13..=16 => SessionId::Overlap,
        17..=21 => SessionId::UsLate,
        _ => SessionId::Dead,
    }
}

/// Hour-of-day (UTC) for a timestamp; `None` when unparseable.
pub fn hour_of_day_utc(ts_ms: i64) -> Option<u32> {
    datetime_of(ts_ms).map(|d| d.hour())
}

/// True for Saturday/Sunday UTC. Weekends run a separate profile with
/// doubled spread floors and halved size allowance.
pub fn is_weekend(ts_ms: i64) -> bool {
    matches!(
        datetime_of(ts_ms).map(|d| d.weekday()),
        Some(Weekday::Sat) | Some(Weekday::Sun)
    )
}

/* ------------------------------------------------------------------------ */
/* Funding masks                                                             */
/* ------------------------------------------------------------------------ */

/// True inside +/-2 minutes of a funding stamp for the venue's cadence.
///
/// Stamps are always 00/08/16 UTC (8h venues); venues on a 4h cadence add
/// 04/12/20 UTC; hourly venues mask every hour. `venue_intervals_h` holds
/// the venue's funding intervals in hours (e.g. `&[8]`, `&[4]`, `&[1]`);
/// an empty slice falls back to the 8h stamps (fail closed).
pub fn in_funding_mask(ts_ms: i64, venue_intervals_h: &[u32]) -> bool {
    let dt = match datetime_of(ts_ms) {
        Some(d) => d,
        None => return true,
    };
    let secs_of_day = dt.num_seconds_from_midnight() as i64;
    let hourly = venue_intervals_h.iter().any(|&i| i <= 1);
    if hourly {
        let in_hour = secs_of_day % 3600;
        return in_hour <= FUNDING_MASK_HALF_WIDTH_SECS
            || in_hour >= 3600 - FUNDING_MASK_HALF_WIDTH_SECS;
    }
    let mut stamps = [false; 24];
    for h in [0u32, 8, 16] {
        stamps[h as usize] = true;
    }
    if venue_intervals_h.iter().any(|&i| i <= 4) {
        for h in [4u32, 12, 20] {
            stamps[h as usize] = true;
        }
    }
    let mut stamps = [false; 24];
    for h in [0u32, 8, 16] {
        stamps[h as usize] = true;
    }
    if venue_intervals_h.iter().any(|&i| i <= 4) {
        for h in [4u32, 12, 20] {
            stamps[h as usize] = true;
        }
    }
    // Circular distance to the nearest stamp: the +/-2min window straddles
    // the hour boundary (07:58 belongs to the 08:00 stamp).
    for (h, active) in stamps.iter().enumerate() {
        if !active {
            continue;
        }
        let stamp_secs = h as i64 * 3600;
        let diff = (secs_of_day - stamp_secs)
            .abs()
            .min(86400 - (secs_of_day - stamp_secs).abs());
        if diff <= FUNDING_MASK_HALF_WIDTH_SECS {
            return true;
        }
    }
    false
}

/* ------------------------------------------------------------------------ */
/* US cash hours (Ondo equity-leg gate)                                      */
/* ------------------------------------------------------------------------ */

fn nth_weekday_of_month(year: i32, month: u32, weekday: Weekday, n: u32) -> Option<u32> {
    let mut day = 1u32;
    let mut count = 0u32;
    while day <= 31 {
        let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        if date.weekday() == weekday {
            count += 1;
            if count == n {
                return Some(day);
            }
        }
        day += 1;
    }
    None
}

fn last_weekday_of_month(year: i32, month: u32, weekday: Weekday) -> Option<u32> {
    let mut day = 28u32;
    while chrono::NaiveDate::from_ymd_opt(year, month, day + 1).is_some() {
        day += 1;
    }
    while day >= 1 {
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
            if date.weekday() == weekday {
                return Some(day);
            }
        }
        if day == 1 {
            break;
        }
        day -= 1;
    }
    None
}

/// Gregorian computus (Anonymous algorithm): Good Friday month/day.
fn good_friday(year: i32) -> Option<(u32, u32)> {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let easter_month = ((h + l - 7 * m + 114) / 31) as u32;
    let easter_day = (((h + l - 7 * m + 114) % 31) + 1) as u32;
    let easter = chrono::NaiveDate::from_ymd_opt(year, easter_month, easter_day)?;
    let friday = easter.pred_opt()?.pred_opt()?;
    Some((friday.month(), friday.day()))
}

/// Seeded US equity holiday list (month, day) for a year, including
/// observed-day shifts (Saturday -> prior Friday, Sunday -> next Monday).
fn us_equity_holidays(year: i32) -> Vec<(u32, u32)> {
    let mut fixed: Vec<(u32, u32)> = vec![(1, 1), (6, 19), (7, 4), (12, 25)];
    if let Some(d) = nth_weekday_of_month(year, 1, Weekday::Mon, 3) {
        fixed.push((1, d));
    }
    if let Some(d) = nth_weekday_of_month(year, 2, Weekday::Mon, 3) {
        fixed.push((2, d));
    }
    if let Some((m, d)) = good_friday(year) {
        fixed.push((m, d));
    }
    if let Some(d) = last_weekday_of_month(year, 5, Weekday::Mon) {
        fixed.push((5, d));
    }
    if let Some(d) = nth_weekday_of_month(year, 9, Weekday::Mon, 1) {
        fixed.push((9, d));
    }
    if let Some(d) = nth_weekday_of_month(year, 11, Weekday::Thu, 4) {
        fixed.push((11, d));
    }
    let mut out = Vec::with_capacity(fixed.len() * 2);
    for (m, d) in fixed {
        out.push((m, d));
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, m, d) {
            match date.weekday() {
                Weekday::Sat => {
                    if let Some(obs) = date.pred_opt() {
                        out.push((obs.month(), obs.day()));
                    }
                }
                Weekday::Sun => {
                    if let Some(obs) = date.succ_opt() {
                        // Observed Monday may fall in the adjacent month.
                        let _ = obs.year();
                        out.push((obs.month(), obs.day()));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Second Sunday of March (DST start) and first Sunday of November
/// (DST end) for a year; `None` on calendar failure (fail closed to EST).
fn us_dst_bounds(year: i32) -> (Option<u32>, Option<u32>) {
    (
        nth_weekday_of_month(year, 3, Weekday::Sun, 2),
        nth_weekday_of_month(year, 11, Weekday::Sun, 1),
    )
}

/// True while US cash equities are open: 09:30-16:00 ET on non-holiday
/// weekdays. DST-aware: 13:30-20:00 UTC under EDT, 14:30-21:00 UTC under
/// EST. The seeded holiday list covers the regular NYSE calendar; unknown
/// calendar sync stays out of scope (Phase 2+).
pub fn us_cash_open(ts_ms: i64) -> bool {
    let dt = match datetime_of(ts_ms) {
        Some(d) => d,
        None => return false,
    };
    if matches!(dt.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let (y, m, d) = (dt.year(), dt.month(), dt.day());
    if us_equity_holidays(y).contains(&(m, d)) {
        return false;
    }
    let in_dst = match us_dst_bounds(y) {
        (Some(start_day), Some(end_day)) => {
            let after_start = (m, d) >= (3, start_day);
            let before_end = (m, d) < (11, end_day);
            (m > 3 || (m == 3 && after_start)) && (m < 11 || (m == 11 && before_end))
        }
        _ => false,
    };
    let secs = dt.num_seconds_from_midnight() as i64;
    if in_dst {
        (13 * 3600 + 1800..20 * 3600).contains(&secs)
    } else {
        (14 * 3600 + 1800..21 * 3600).contains(&secs)
    }
}

/* ------------------------------------------------------------------------ */
/* Shrinkage over winsorized medians                                         */
/* ------------------------------------------------------------------------ */

fn sorted_finite(values: &[f64]) -> Vec<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Quantile over an ascending-sorted slice; `None` when empty.
pub fn percentile_sorted(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() || !q.is_finite() || !(0.0..=1.0).contains(&q) {
        return None;
    }
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let rank = q * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    let (a, b) = (sorted[lo], sorted[hi.min(sorted.len() - 1)]);
    Some(a + (b - a) * frac)
}

/// Median of 5%/95% winsorized values: outliers are clipped to the
/// percentile edges, never dropped, so thin samples keep their weight.
pub fn winsorized_median(values: &[f64]) -> Option<f64> {
    let sorted = sorted_finite(values);
    if sorted.is_empty() {
        return None;
    }
    let lo = percentile_sorted(&sorted, 0.05).unwrap_or(sorted[0]);
    let hi = percentile_sorted(&sorted, 0.95).unwrap_or(sorted[sorted.len() - 1]);
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let clipped: Vec<f64> = sorted.iter().map(|v| v.clamp(lo, hi)).collect();
    let mid = clipped.len() / 2;
    if clipped.len() % 2 == 1 {
        Some(clipped[mid])
    } else {
        Some((clipped[mid - 1] + clipped[mid]) / 2.0)
    }
}

/// Empirical-Bayes weight of the sample against the global prior:
/// `n / (n + n0)`, so `n0 = 150` episodes carry half weight.
pub fn eb_weight(n: f64, n0: f64) -> f64 {
    if !n.is_finite() || !n0.is_finite() || n <= 0.0 || n0 < 0.0 {
        return 0.0;
    }
    n / (n + n0)
}

/// Shrink a session sample toward the global median. Non-finite inputs
/// degrade to the (finite) global, or `None` when both are unusable.
pub fn eb_shrink(sample: f64, global: f64, n: f64, n0: f64) -> Option<f64> {
    if !global.is_finite() {
        return None;
    }
    if !sample.is_finite() {
        return Some(global);
    }
    Some(eb_weight(n, n0) * sample + (1.0 - eb_weight(n, n0)) * global)
}

/// Per-session aggregate over absolute-spread observations.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStats {
    pub n: usize,
    pub med: Option<f64>,
    pub p90: Option<f64>,
    /// Sample median shrunk toward the global median (`n0 = 150`).
    pub shrunk_med: Option<f64>,
    /// True below [`SESSION_MIN_EPISODES`]: callers use neutral x1.0.
    pub provisional: bool,
}

/// Aggregate one session's absolute-spread values against the global
/// median. Empty input yields a provisional, all-`None` aggregate —
/// never a fabricated number.
pub fn session_stats(values: &[f64], global_med: f64) -> SessionStats {
    let sorted = sorted_finite(values);
    let n = sorted.len();
    let provisional = n < SESSION_MIN_EPISODES;
    if n == 0 {
        return SessionStats {
            n: 0,
            med: None,
            p90: None,
            shrunk_med: if global_med.is_finite() {
                Some(global_med)
            } else {
                None
            },
            provisional: true,
        };
    }
    let med = winsorized_median(&sorted);
    let p90 = percentile_sorted(&sorted, 0.90);
    let shrunk_med = match (med, global_med.is_finite()) {
        (Some(m), true) => eb_shrink(m, global_med, n as f64, SHRINK_N0),
        _ => {
            if global_med.is_finite() {
                Some(global_med)
            } else {
                None
            }
        }
    };
    SessionStats {
        n,
        med,
        p90,
        shrunk_med,
        provisional,
    }
}

/// Weekend-compensated spread floor: doubled on weekends, passthrough
/// otherwise. Non-finite or negative bases degrade to 0.0, never NaN.
pub fn spread_floor_bps(base_floor_bps: f64, weekend: bool) -> f64 {
    if !base_floor_bps.is_finite() || base_floor_bps <= 0.0 {
        return 0.0;
    }
    if weekend {
        base_floor_bps * WEEKEND_FLOOR_MULT
    } else {
        base_floor_bps
    }
}

/// Hour-bin modulator around the session value, capped at +/-25%.
/// Unknown inputs degrade to neutral 1.0.
pub fn hour_modulator(hour_med: f64, session_med: f64) -> f64 {
    if !hour_med.is_finite() || !session_med.is_finite() || session_med <= 0.0 {
        return 1.0;
    }
    (hour_med / session_med).clamp(1.0 - HOUR_MODULATOR_CAP, 1.0 + HOUR_MODULATOR_CAP)
}

/* ------------------------------------------------------------------------ */
/* In-memory EWMA fast vol                                                   */
/* ------------------------------------------------------------------------ */

/// O(1)-per-tick EWMA over realized absolute deviation (bps), lambda 0.96.
/// Durable profiles stay in SQLite; this tracker is intentionally
/// in-memory (hub/cache do not survive restart, profiles do).
#[derive(Debug, Clone)]
pub struct FastVolEwma {
    lambda: f64,
    floor_bps: f64,
    cap_bps: f64,
    value: Option<f64>,
}

impl FastVolEwma {
    pub fn new() -> Self {
        Self {
            lambda: FAST_VOL_LAMBDA,
            floor_bps: FAST_VOL_FLOOR_BPS,
            cap_bps: FAST_VOL_CAP_BPS,
            value: None,
        }
    }

    pub fn with_params(lambda: f64, floor_bps: f64, cap_bps: f64) -> Self {
        let mut floor = if floor_bps.is_finite() && floor_bps >= 0.0 {
            floor_bps
        } else {
            FAST_VOL_FLOOR_BPS
        };
        let mut cap = if cap_bps.is_finite() && cap_bps > 0.0 {
            cap_bps
        } else {
            FAST_VOL_CAP_BPS
        };
        if floor > cap {
            core::mem::swap(&mut floor, &mut cap);
        }
        Self {
            lambda: if lambda.is_finite() && lambda > 0.0 && lambda < 1.0 {
                lambda
            } else {
                FAST_VOL_LAMBDA
            },
            floor_bps: floor,
            cap_bps: cap,
            value: None,
        }
    }

    /// Fold one realized absolute deviation (bps) into the average.
    /// Non-finite or negative observations are ignored (gaps stay gaps).
    pub fn update(&mut self, realized_bps: f64) -> Option<f64> {
        if !realized_bps.is_finite() || realized_bps < 0.0 {
            return self.value;
        }
        let x = realized_bps.clamp(self.floor_bps, self.cap_bps);
        self.value = Some(match self.value {
            Some(prev) => self.lambda * prev + (1.0 - self.lambda) * x,
            None => x,
        });
        self.value
    }

    pub fn value(&self) -> Option<f64> {
        self.value
    }

    /// Ratio of fast vol to the session norm. `None` when either side is
    /// unusable — the caller treats it as neutral, never as elevated.
    pub fn ratio_to(&self, norm_bps: f64) -> Option<f64> {
        match (self.value, norm_bps) {
            (Some(v), n) if v.is_finite() && n.is_finite() && n > 0.0 => Some(v / n),
            _ => None,
        }
    }
}

impl Default for FastVolEwma {
    fn default() -> Self {
        Self::new()
    }
}

/* ------------------------------------------------------------------------ */
/* Session multipliers                                                       */
/* ------------------------------------------------------------------------ */

/// Size-allowance factor for a session: overlap 1.0, EU + US-late 0.8,
/// Asia 0.5, dead 0.4, weekends halve again, funding masks block (0.0).
pub fn session_allow(session: SessionId, weekend: bool, funding_mask: bool) -> f64 {
    if funding_mask {
        return 0.0;
    }
    let base = match session {
        SessionId::Overlap => 1.0,
        SessionId::Europe | SessionId::UsLate => 0.8,
        SessionId::Asia => 0.5,
        SessionId::Dead => 0.4,
    };
    if weekend {
        base * 0.5
    } else {
        base
    }
}

/// Additive entry-threshold adjustment, capped at 2x the base and floored
/// at 0.5x. A zero/unknown session delta leaves the base untouched.
pub fn apply_entry_adjust_bps(base_bps: f64, session_delta_bps: f64) -> f64 {
    if !base_bps.is_finite() || base_bps <= 0.0 {
        return base_bps;
    }
    if !session_delta_bps.is_finite() {
        return base_bps;
    }
    (base_bps + session_delta_bps).clamp(base_bps * 0.5, base_bps * 2.0)
}

/// Inverse-vol sizing factor `clamp(sigma_med / sigma_h, 0.5, 1.5)`;
/// unusable inputs degrade to neutral 1.0.
pub fn inverse_vol_mult(sigma_med: f64, sigma_h: f64) -> f64 {
    if !sigma_med.is_finite() || !sigma_h.is_finite() || sigma_med <= 0.0 || sigma_h <= 0.0 {
        return 1.0;
    }
    (sigma_med / sigma_h).clamp(0.5, 1.5)
}

/// Depth sizing factor `clamp(depth_h / depth_med, 0.25, 1.0)` — depth
/// only ever shrinks size, never inflates it. Unusable inputs are 1.0.
pub fn depth_mult(depth_h: f64, depth_med: f64) -> f64 {
    if !depth_h.is_finite() || !depth_med.is_finite() || depth_med <= 0.0 || depth_h < 0.0 {
        return 1.0;
    }
    (depth_h / depth_med).clamp(0.25, 1.0)
}

/// True when either leg venue is an Ondo equity leg (US cash-hours gated).
pub fn is_ondo_venue(venue: &str) -> bool {
    matches!(
        venue.trim().to_lowercase().as_str(),
        "ondoperps" | "ondo" | "ondo perps"
    )
}

/// True when the venue trades cash shares rather than perpetuals — the
/// only venues whose legs owe dividend/borrow/expense carry in addition
/// to (or instead of) perp funding. Alpaca and IBKR are share/equity venues; Ondo legs
/// are tokenized perps whose funding already wraps the carry.
pub fn is_equity_rails(venue: &str) -> bool {
    let raw = venue.trim().to_lowercase();
    let v = raw
        .strip_prefix("live_")
        .or_else(|| raw.strip_prefix("tn_"))
        .unwrap_or(&raw);
    matches!(v, "alpaca" | "ibkr" | "ig")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("fixture timestamp must be valid")
            .timestamp_millis()
    }

    #[test]
    fn sessions_follow_utc_boundaries() {
        // Thursday 2026-01-15 (winter, weekday).
        assert_eq!(session_of(ts_ms(2026, 1, 15, 0, 0, 0)), SessionId::Asia);
        assert_eq!(session_of(ts_ms(2026, 1, 15, 7, 59, 59)), SessionId::Asia);
        assert_eq!(session_of(ts_ms(2026, 1, 15, 8, 0, 0)), SessionId::Europe);
        assert_eq!(
            session_of(ts_ms(2026, 1, 15, 12, 59, 59)),
            SessionId::Europe
        );
        assert_eq!(session_of(ts_ms(2026, 1, 15, 13, 0, 0)), SessionId::Overlap);
        assert_eq!(
            session_of(ts_ms(2026, 1, 15, 16, 59, 59)),
            SessionId::Overlap
        );
        assert_eq!(session_of(ts_ms(2026, 1, 15, 17, 0, 0)), SessionId::UsLate);
        assert_eq!(
            session_of(ts_ms(2026, 1, 15, 21, 59, 59)),
            SessionId::UsLate
        );
        assert_eq!(session_of(ts_ms(2026, 1, 15, 22, 0, 0)), SessionId::Dead);
        assert_eq!(session_of(ts_ms(2026, 1, 15, 23, 59, 59)), SessionId::Dead);
    }

    #[test]
    fn session_ids_round_trip() {
        for s in SessionId::all() {
            assert_eq!(SessionId::parse(s.as_str()), Some(s));
        }
        assert_eq!(SessionId::parse("bogus"), None);
        // Weekend still reports its intraday session; weekend is orthogonal.
        assert_eq!(session_of(ts_ms(2026, 1, 17, 14, 0, 0)), SessionId::Overlap);
    }

    #[test]
    fn weekend_predicate_frozen() {
        // 2026-01-17 is a Saturday, 2026-01-19 a Monday.
        assert!(is_weekend(ts_ms(2026, 1, 17, 12, 0, 0)));
        assert!(is_weekend(ts_ms(2026, 1, 18, 12, 0, 0)));
        assert!(!is_weekend(ts_ms(2026, 1, 15, 12, 0, 0)));
        assert!(!is_weekend(ts_ms(2026, 1, 19, 12, 0, 0)));
    }

    #[test]
    fn funding_mask_8h_stamps() {
        let eight = [8u32];
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 8, 0, 0), &eight));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 7, 58, 1), &eight));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 8, 1, 59), &eight));
        assert!(!in_funding_mask(ts_ms(2026, 1, 15, 8, 2, 1), &eight));
        assert!(!in_funding_mask(ts_ms(2026, 1, 15, 7, 57, 59), &eight));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 0, 0, 0), &eight));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 16, 0, 0), &eight));
        // 04:00 is not an 8h stamp.
        assert!(!in_funding_mask(ts_ms(2026, 1, 15, 4, 0, 0), &eight));
        // Empty intervals fail closed to the 8h stamps.
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 8, 0, 0), &[]));
    }

    #[test]
    fn funding_mask_4h_and_hourly() {
        let four = [4u32];
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 4, 0, 0), &four));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 12, 1, 0), &four));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 20, 0, 0), &four));
        assert!(!in_funding_mask(ts_ms(2026, 1, 15, 5, 0, 0), &four));
        let hourly = [1u32];
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 5, 0, 0), &hourly));
        assert!(in_funding_mask(ts_ms(2026, 1, 15, 23, 59, 30), &hourly));
        assert!(!in_funding_mask(ts_ms(2026, 1, 15, 5, 30, 0), &hourly));
    }

    #[test]
    fn us_cash_open_dst_boundaries() {
        // Winter (EST, UTC-5): 14:30-21:00 UTC on Thu 2026-01-15.
        assert!(!us_cash_open(ts_ms(2026, 1, 15, 14, 29, 59)));
        assert!(us_cash_open(ts_ms(2026, 1, 15, 14, 30, 0)));
        assert!(us_cash_open(ts_ms(2026, 1, 15, 20, 59, 59)));
        assert!(!us_cash_open(ts_ms(2026, 1, 15, 21, 0, 0)));
        // Summer (EDT, UTC-4): 13:30-20:00 UTC on Wed 2026-07-15.
        assert!(!us_cash_open(ts_ms(2026, 7, 15, 13, 29, 59)));
        assert!(us_cash_open(ts_ms(2026, 7, 15, 13, 30, 0)));
        assert!(us_cash_open(ts_ms(2026, 7, 15, 19, 59, 59)));
        assert!(!us_cash_open(ts_ms(2026, 7, 15, 20, 0, 0)));
        // DST 2026 starts Mar 8: Mar 6 (Fri, EST) vs Mar 9 (Mon, EDT).
        assert!(!us_cash_open(ts_ms(2026, 3, 6, 13, 45, 0)));
        assert!(us_cash_open(ts_ms(2026, 3, 6, 14, 45, 0)));
        assert!(us_cash_open(ts_ms(2026, 3, 9, 13, 45, 0)));
        assert!(!us_cash_open(ts_ms(2026, 3, 9, 20, 0, 0)));
        // DST ends Nov 1 2026: Nov 2 is back on EST.
        assert!(!us_cash_open(ts_ms(2026, 11, 2, 13, 45, 0)));
        assert!(us_cash_open(ts_ms(2026, 11, 2, 14, 45, 0)));
    }

    #[test]
    fn us_cash_open_holidays() {
        // Christmas 2026 (Friday) closed all session.
        assert!(!us_cash_open(ts_ms(2026, 12, 25, 15, 0, 0)));
        // Independence Day 2026 falls Saturday -> observed Friday Jul 3.
        assert!(!us_cash_open(ts_ms(2026, 7, 3, 15, 0, 0)));
        assert!(us_cash_open(ts_ms(2026, 7, 6, 15, 0, 0)));
        // Thanksgiving Thu 2026-11-26 closed; Friday open.
        assert!(!us_cash_open(ts_ms(2026, 11, 26, 15, 0, 0)));
        assert!(us_cash_open(ts_ms(2026, 11, 27, 15, 0, 0)));
        // Good Friday 2026-04-03 closed.
        assert!(!us_cash_open(ts_ms(2026, 4, 3, 15, 0, 0)));
        // New Year's Day 2026 (Thursday) closed.
        assert!(!us_cash_open(ts_ms(2026, 1, 1, 15, 0, 0)));
    }

    #[test]
    fn shrinkage_centers_and_provisional_flags() {
        // No data: global passthrough, provisional.
        let s = session_stats(&[], 10.0);
        assert_eq!(s.n, 0);
        assert!(s.provisional);
        assert_eq!(s.shrunk_med, Some(10.0));
        // Thin data stays near the prior and provisional.
        let thin = vec![20.0; 5];
        let s = session_stats(&thin, 10.0);
        assert!(s.provisional);
        let w = eb_weight(5.0, SHRINK_N0);
        assert!((w - 5.0 / 155.0).abs() < 1e-12);
        assert!((s.shrunk_med.unwrap() - (w * 20.0 + (1.0 - w) * 10.0)).abs() < 1e-9);
        // n0 episodes carry exactly half weight.
        assert!((eb_weight(SHRINK_N0, SHRINK_N0) - 0.5).abs() < 1e-12);
        assert!((eb_shrink(20.0, 10.0, SHRINK_N0, SHRINK_N0).unwrap() - 15.0).abs() < 1e-12);
        // Deep data converges and clears provisional.
        let deep = vec![20.0; 300];
        let s = session_stats(&deep, 10.0);
        assert!(!s.provisional);
        assert!(
            (s.shrunk_med.unwrap() - (300.0 / 450.0 * 20.0 + 150.0 / 450.0 * 10.0)).abs() < 1e-9
        );
        // Zero weight keeps the prior exactly.
        assert_eq!(eb_shrink(20.0, 10.0, 0.0, SHRINK_N0), Some(10.0));
        assert_eq!(eb_weight(-3.0, SHRINK_N0), 0.0);
    }

    #[test]
    fn winsorized_median_clips_outliers() {
        let vals = vec![10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 1000.0];
        let m = winsorized_median(&vals).unwrap();
        assert!(m < 100.0, "outlier must be clipped, got {m}");
        assert!(winsorized_median(&[]).is_none());
        assert!(winsorized_median(&[f64::NAN]).is_none());
        assert_eq!(winsorized_median(&[7.0]), Some(7.0));
    }

    #[test]
    fn weekend_floors_and_hour_modulator() {
        assert_eq!(spread_floor_bps(2.0, false), 2.0);
        assert_eq!(spread_floor_bps(2.0, true), 4.0);
        assert_eq!(spread_floor_bps(f64::NAN, true), 0.0);
        assert_eq!(hour_modulator(10.0, 10.0), 1.0);
        assert_eq!(hour_modulator(20.0, 10.0), 1.25);
        assert_eq!(hour_modulator(1.0, 10.0), 0.75);
        assert_eq!(hour_modulator(f64::NAN, 10.0), 1.0);
        assert_eq!(hour_modulator(10.0, 0.0), 1.0);
    }

    #[test]
    fn ewma_updates_floors_caps_and_gaps() {
        let mut e = FastVolEwma::new();
        assert_eq!(e.value(), None);
        assert_eq!(e.ratio_to(10.0), None);
        // First observation seeds.
        assert_eq!(e.update(10.0), Some(10.0));
        // Second blends with lambda 0.96.
        let v = e.update(20.0).unwrap();
        assert!((v - (0.96 * 10.0 + 0.04 * 20.0)).abs() < 1e-9);
        // Gaps are ignored, never interpolated.
        let before = e.value();
        assert_eq!(e.update(f64::NAN), before);
        assert_eq!(e.update(-5.0), before);
        // Cap clamps spikes before blending.
        let mut capped = FastVolEwma::with_params(0.0, 0.0, 100.0);
        assert_eq!(capped.update(10_000.0), Some(100.0));
        // Bad params degrade to defaults, bad norms to None.
        let mut d = FastVolEwma::with_params(f64::NAN, -1.0, 0.0);
        assert_eq!(d.update(10.0), Some(10.0));
        assert_eq!(e.ratio_to(0.0), None);
        assert_eq!(e.ratio_to(f64::NAN), None);
    }

    #[test]
    fn ewma_inverted_floor_cap_does_not_panic() {
        let mut e = FastVolEwma::with_params(0.96, 500.0, 100.0);
        assert_eq!(e.update(10.0), Some(100.0));
        let v = e.update(10_000.0).unwrap();
        assert!((v - (0.96 * 100.0 + 0.04 * 500.0)).abs() < 1e-9);
    }

    #[test]
    fn session_allow_table() {
        assert_eq!(session_allow(SessionId::Overlap, false, false), 1.0);
        assert_eq!(session_allow(SessionId::Europe, false, false), 0.8);
        assert_eq!(session_allow(SessionId::UsLate, false, false), 0.8);
        assert_eq!(session_allow(SessionId::Asia, false, false), 0.5);
        assert_eq!(session_allow(SessionId::Dead, false, false), 0.4);
        assert_eq!(session_allow(SessionId::Overlap, true, false), 0.5);
        assert_eq!(session_allow(SessionId::Dead, true, false), 0.2);
        assert_eq!(session_allow(SessionId::Overlap, false, true), 0.0);
        assert_eq!(session_allow(SessionId::Asia, true, true), 0.0);
    }

    #[test]
    fn entry_adjust_additive_capped_at_2x() {
        assert_eq!(apply_entry_adjust_bps(10.0, 0.0), 10.0);
        assert_eq!(apply_entry_adjust_bps(10.0, 3.0), 13.0);
        assert_eq!(apply_entry_adjust_bps(10.0, 50.0), 20.0);
        assert_eq!(apply_entry_adjust_bps(10.0, -50.0), 5.0);
        assert_eq!(apply_entry_adjust_bps(10.0, f64::NAN), 10.0);
    }

    #[test]
    fn sizing_factors_clamp_and_neutral() {
        assert_eq!(inverse_vol_mult(10.0, 10.0), 1.0);
        assert_eq!(inverse_vol_mult(10.0, 5.0), 1.5);
        assert_eq!(inverse_vol_mult(10.0, 100.0), 0.5);
        assert_eq!(inverse_vol_mult(0.0, 10.0), 1.0);
        assert_eq!(inverse_vol_mult(10.0, 0.0), 1.0);
        assert_eq!(inverse_vol_mult(f64::NAN, 10.0), 1.0);
        assert_eq!(depth_mult(5.0, 10.0), 0.5);
        assert_eq!(depth_mult(50.0, 10.0), 1.0);
        assert_eq!(depth_mult(0.5, 10.0), 0.25);
        assert_eq!(depth_mult(5.0, 0.0), 1.0);
        assert!(is_ondo_venue("ondoperps"));
        assert!(is_ondo_venue(" Ondo "));
        assert!(!is_ondo_venue("hyperliquid"));
        assert!(is_equity_rails("alpaca"));
        assert!(is_equity_rails("ibkr"));
        assert!(is_equity_rails(" IBKR "));
        assert!(!is_equity_rails("hyperliquid"));
    }
}
