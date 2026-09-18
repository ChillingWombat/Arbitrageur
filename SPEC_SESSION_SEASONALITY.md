# System Specification: Session-Aware Seasonality Layer

Status: **Phase 1 IMPLEMENTED 2026-09-16 — profiles display-only; nothing goes live-aggressive before promotion review**

## Decisions Locked (proposed — confirm before build)

1. **Sessions (UTC)**: Asia `00–08`, Europe `08–13`, US–EU overlap `13–17`
   (peak), US late `17–22`, dead zone `22–24`, plus a separate
   weekend profile. Overlap-first: execution regimes key off these five,
   not naive 8h blocks (which mislabel the 13–17 peak).
2. **Volatility stack**: EWMA fast layer (5-min realized, λ≈0.96) per pair +
   nightly HAR-style daily anchor; no live GARCH (offline benchmark only).
3. **Math discipline**: shrunken 24h bins (±25% modulation max) over 5
   session aggregates; decisions read sessions, never raw hour bins.
   Provisional bins serve session values. Walk-forward promotion only.
4. **Fail-closed**: funding masks (±2 min around 00/08/16 UTC, +04/12/20
   for 4h venues) block entries; stale/dead sessions degrade to neutral
   ×1.0; Ondo equity legs gate on US cash hours; unknown = no adjustment.
5. **Scope**: Phase 1 = profiles + vol features + monitor/Radar integration
   + display. Phase 2 (later) = HAR anchor automation, autotune coupling,
   promotion jobs.

## Problem Statement

Crypto trades 24/7 but flow does not: volume/volatility concentrate in
US–EU overlap (13–17 UTC, ~47% of BTC volume post-ETF), trough 02–06 UTC
and 21–23 UTC, weekends run ~2× spreads on −20–40% volume, and funding
stamps (00/08/16 UTC, hourly vs 8h/4h venue mismatch) inject transient
basis jumps. Today the engine uses one global OU half-life, one hurdle,
and one sizing rule around the clock — overtrading thin hours into fees
and under-sizing the peak window. No hour/session, volume-series, ATR, or
regime store exists (verified by seam survey).

## Solution

### Session clock (`arbitrageur-math`)

`session_of(ts_ms) -> SessionId` with the five regimes + `is_weekend` +
`in_funding_mask(ts, venue_intervals)` + `us_cash_open(ts)` (Ondo gate,
DST-aware 13:30–20:00/14:30–21:00 UTC + holiday list seeded manually).
Pure, tested, no I/O.

### Data: volume sampler + profile tables (migration V8)

- New sampler hooks the existing volume refresh (`cached_venue_volumes`):
  persists `(venue, symbol, ts_ms, vol_usd)` hourly (or on TTL refresh —
  cheapest correct cadence) into `venue_volume_samples`.
- `session_profiles(symbol, session_id, n_episodes, med_abs_spread_bps,
  p90_spread_bps, rv_bps, win_rate, mean_hold_s, updated_ts)` — shrunk
  aggregates written by an hourly rollup over `spread_samples` +
  episode segmentation; 24h bins materialize only as ±25% modulators
  inside the same row family (`session_hour_bins`, provisional-flagged).
- Fast vol stays in-memory EWMA per pair (O(1)/tick); profiles are the
  durable part (SQLite survives restart; hub/cache do not).

### Strategy integration (monitor `evaluate_pair` + sizing)

- Entry threshold: `entry_thresh × session_entry_mult` (additive bps
  preferred; cap 2× base).
- Sizing: `deriveStepSizing` gains session multipliers —
  inverse-vol (`clamp(σ_med/σ_h, 0.5, 1.5)`) × depth
  (`clamp(D_h/D_med, 0.25, 1.0)`) × `session_allow` (overlap 1.0 /
  EU+US 0.8 / Asia 0.5 / dead 0.4 / weekend ×0.5 / funding-mask 0).
- Qualification: session-confidence gate alongside existing gates.
- Stand-downs: funding masks, spread >3× session median, Ondo-closed
  equity legs, fast-vol >3× norm → halve then halt. All render as
  explicit `block_reason`s.

### IPC + UI

- `get_session_profile(symbol)` → per-session rows + current session +
  active multipliers (snapshot pattern, registered in `main.rs`).
- Radar: session badge/multiplier column; SignalTrend: session overlay
  band; configurator: session-profile picker beside window-mode (display
  only in Phase 1 — no auto-apply of tuned values; autotune consumes the
  API when Phase 2 lands).

## Failure & Honesty Constraints

- Provisional/insufficient coverage → neutral ×1.0 + flagged, never a
  guessed multiplier (mirrors `using_manual_fallback`).
- Gaps stay gaps (no interpolation into bins); weekends need 2× floors.
- Stress weeks (e.g. mass-liquidation events) are excluded from
  calibration, labeled as stress data.
- Live mode: visible but non-executing, consistent with existing gates.

## Testing Decisions

- Math: fixture hour-bins → shrinkage math, session aggregation, EWMA
  updates, mask predicates, DST boundaries (frozen timestamps).
- Backend: seeded multi-day history → profile rows; multiplier neutrality
  on thin data; stand-down triggers; IPC round-trips; migration V7→V8.
- Frontend: badge/column rendering incl. provisional states; disabled
  states when no profile.

## Out of Scope (Phase 2+)

HAR-anchor nightly job automation, autotune coupling, walk-forward
promotion scheduler, per-hour τ sets (sessions only), GARCH, ML models,
quote-activity volume proxy refinement, holiday-calendar sync.

## Rollout

Session clock + tables + sampler → rollup + multipliers → IPC + UI →
gates green (`cargo test`, `clippy -D warnings`, `tsc`, `npm test`) →
weekend of live accumulation → first empirical profiles → promotion
review before any threshold goes live-aggressive.
