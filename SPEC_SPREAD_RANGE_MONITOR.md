# System Specification: Spread Range Monitor (Opportunity Radar)

Status: **APPROVED — implementation in progress**

## Decisions Locked (user sign-off)

1. **Cadence**: dynamic, budget-driven. Price data is pulled once per venue
   via that venue's **default account** — never duplicated per account.
   Per-venue request budget respects real documented limits (Lighter:
   60 req/min **per IP, shared across accounts — the lowest tier among
   accounts on the same IP applies**; premium account tiers raise the cap
   but add +0.5 bps fees, selected per account in credentials). Headroom is
   always reserved for execution traffic so monitoring never starves trades.
   Cadence self-adjusts to `symbols × venues` growth.
2. **Watchlist**: derived from live `fetch_markets()` per admitted venue,
   cached and refreshed hourly (symbols appear/disappear as accounts are
   enabled). Per-ticker **hide/exempt** flag in the ticker's settings;
   a system setting reveals (un-hides) all exempted tickers globally.
3. **Baseline**: mid-mid history, executable-edge current value.
4. **UI**: separate Radar tab.
5. **Auto window**: `2 × half-life`, 30-minute floor, noise filter (observed
   deviations during warm-up must exceed the friction band), all adjustable
   in the configurator. (1-day observation noted as a manual option — market
   hours affect funding/basis.)
6. **Ranking**: expected bps (`net capture × P_win`), primary sort.
7. **Funding horizon**: conservative — net cost → `max(t_hold, 8h)`;
   net gain → `min(t_hold, 1h)`.
8. **Capacity**: fixed tiers `$1k / $5k / $10k` (configurable); breakeven =
   expected-profit integral over consumed book levels using **per-level win
   probability** (`P_win` varies with entry price depth), marginal
   `P_win ≥ p_min` (default 90%, configurable); depth of **both** legs.
   One-way/two-way execution mode in configurator — default **one-way**
   (divergence: spread getting larger).

## Problem Statement

The current system evaluates spatial arbitrage **on demand for one selected ticker**:
`scan_spatial_opportunities` is only invoked when a symbol is chosen in the explorer,
and the per-ticker strategy bot trades only its configured symbol. There is no
systematic, always-on view of dislocation opportunities across the whole tradable
universe, no statistical ranking of *which* deviation is most likely to revert
profitably, and no way to compare the maturity/reliability of signals across pairs.

The trader needs a radar that continuously answers, for every directional
(symbol, buy-venue, sell-venue) pair:

- How far is the current spread from its historical expected value?
- How much capital can this pair actually absorb right now?
- What is the probability the deviation reverts to baseline — after friction?
- How long has this signal been alive, and how long until it should converge?
- Which opportunity is worth executing *right now*?

---

## Goals & Non-Goals

**Goals**

1. A persistent monitor over **all spread-eligible tickers** — every symbol
   quoted by ≥2 admitted venues — evaluated across all venue pairs and both
   directions.
2. Statistically-grounded row metrics: baseline expected value, deviation,
   capacity, win probability, signal age, expected convergence time.
3. Signal lifecycle with a configurable qualification window (manual or
   system-estimated) so rows only become actionable once enough data exists.
4. Ranking by expected profit in bps = net capture × win probability.
5. Filters: ticker search, buy-venue filter, sell-venue filter.
6. Per-row **Execute** button wired to the existing paper execution path.
7. A clean seam for a future autonomous strategy that consumes qualified
   signals (the "overall opportunities bot") without reworking the monitor.

**Non-Goals (v1)**

- Live/real-money execution — the whole system remains paper-only.
- Auto-execution — the bot is Phase 2; v1 ships the monitor + manual Execute.
- New persistence schema — v1 reuses `spread_samples`; a dedicated
  `monitor_signals` table is deferred to Phase 2.

---

## Data Already Available (integration inventory)

| Existing piece | Role in this feature |
|---|---|
| `spread_samples` table `(symbol, venue_id, ts_ms, mid, best_bid, best_ask)` | Historical per-venue price series — the raw material for pair-spread history. Already pruned at 4M rows. |
| `compute_deviation_series` (arbitrageur-math) | Volume-weighted consensus/leader baseline machinery — reused for weight conventions. |
| `OuModelParams::fit` (arbitrageur-math) | AR(1) OLS fit → θ, μ, σ, half-life, stationary std, R² — powers win-probability and convergence estimates. |
| `evaluate_spatial_spread` + `SmartOrderRouter` + `verify_pre_execution_slippage` | Current net-spread and depth/slippage feasibility math — reused for current spread and capacity. |
| `cached_venue_volumes` (60s cache) | 24h notional per venue — volume weighting + participation cap. |
| `venue_fee_bps` | Real per-venue fee table — friction input, never invented. |
| `fetch_funding_rate` (per adapter) | Live funding rate per symbol — carry input; monitor samples it each cycle and keeps a per-venue-symbol EMA. |
| `market_data_hub` | Book cache/validity (350ms freshness) — current-spread source and the ladder's depth-walk input. |
| `execute_spatial_arbitrage_impl` / `get_ticker_position_state` | Execute path + in-flight signal blocking for the row button. |
| `TickerStrategyConfigDto.sliding_window` | Per-symbol window convention to mirror for monitor qualification windows. |

---

## Definitions

For each ordered directional pair **(symbol, buy venue A, sell venue B)**:

- **Current spread `s_t`** — executable-edge directional spread at time t:
  `s_t = (bid_B(t) − ask_A(t)) / ask_A(t) × 10⁴` in bps.
  For the *baseline series* we use mid-mid (`(mid_B − mid_A)/mid_A`) because
  edge prices are noisier; the *current* value is shown both gross and net of
  fees. Both conventions are documented; mixing is avoided within each series.
- **Baseline expected value `μ_pair`** — the historical average directional
  spread for the pair over its sliding window (the system's "expected value"):
  mean of the pair's spread series. When an OU fit qualifies, `μ_OU`
  supersedes it (documented in the row's tooltip).
- **Deviation** — `dev = s_now − μ_pair` (bps) and `z = dev / σ_stationary`.
- **Friction `F(q)`** — round-trip friction in bps **at size q**: `fee_A + fee_B`
  (from `venue_fee_bps`, real values) + execution slippage on *both* legs at
  qty q (orderbook-walk VWAP slippage via existing safety-guard math) +
  estimated closing friction (unwinding the pair incurs the same walk on
  the exit side — double-counted taker round trip by default, configurable).
- **Funding carry `C_f`** — expected funding over the holding horizon for
  the `long A / short B` pair, with per-interval rates `f_A, f_B` from
  `fetch_funding_rate` (real data, EMA-smoothed over recent samples):
  `C_f(t_hold) = (f_A − f_B) × t_hold / funding_interval` in bps.
  **Conservative horizon rule**: when net funding is a *cost* (`f_A > f_B`),
  `t_hold = max(t̂_hold, 8h)`; when net funding is a *gain*,
  `t_hold = min(t̂_hold, 1h)` — we never overstate carry benefit nor
  understate drag. Funding feeds unavailable → `null`, row flagged
  `funding_unknown`; returns are explicitly marked unadjusted, never
  silently zeroed.

### Win probability (honest, fail-closed)

Primary estimator — **empirical reversion hit-rate** over the pair's own
history:

1. Segment history into *episodes*: contiguous runs where `|dev| ≥ |dev_now|`.
2. An episode is a **win** if the spread reverts to within `F(q*) + ε` of
   `μ_pair` before exceeding a stop band (e.g. `|dev| ≥ k·σ`, k configurable,
   default 3).
3. `P_win = wins / episodes`, shown only when `episodes ≥ min_episodes`
   (default 5) **and** the reversion magnitude of typical wins exceeds
   `F(q*) + C_f` — i.e. probability is explicitly conditioned on friction
   *and* funding carry being covered.

Fallback — **OU first-passage approximation**: when `episodes < min_episodes`
but the OU fit has `R² ≥ min_r²` (default 0.6), use the fitted θ/μ/σ to
approximate `P(hit μ-band before stop within horizon H)` — the probability
mass of the conditional distribution crossing the band, computable in closed
form from `conditional_mean`/`conditional_variance` at horizon H.

Otherwise the field is **`null` → displayed "—" (insufficient data)**. Never
an invented number.

### Potential volume for the fund — marginal-capacity ladder

Taking from the orderbook means **net capture decays with size**: the deeper
we walk each leg's book, the worse our VWAP — *and each deeper level has a
different entry deviation, hence a different win probability*. Expected
profit at size `q` is a **per-level weighted integral**, not a flat
edge × size:

```
E[profit(q)] = Σ_levels  P_win(dev_L) × capture_L(q_L)
```

where each book level `L` consumed contributes its own deviation
`dev_L = price_L vs μ_pair` and its own probability — deeper levels enter at
worse prices (lower dev → lower P_win). Both legs' books are walked
together; the binding leg limits each level.

For a **fixed tier ladder** `q₁…qₙ` (default `$1k / $5k / $10k`,
configurable), compute per tier `net_bps(qᵢ)` from this integral — the
monotone-decaying profit-vs-size curve, cut off where either leg's book is
exhausted.

Derived headline numbers:

- **`optimal_size_usd`** — argmax over the ladder of expected USD
  (`Σ P_win(dev_L)·capture_L`), capped by participation
  (`participation × min(vol_24h_A, vol_24h_B)`, default 1%, configurable)
  and the user's risk limits.
- **`breakeven_size_usd`** — the largest size where cumulative expected
  profit stays positive **and** marginal `P_win(dev_L) ≥ p_min`
  (default 90%, configurable) — not merely where raw benefit hits zero.
- **`net_bps_at_optimal`** — expected net bps at optimal size (below
  top-of-book bps — honest by construction).

Volatility enters through the book walk itself: thin/volatile books walk
deeper per size → faster ladder decay — no separate fudge factor. The
ladder is computed **only when a real baseline exists** — no `μ` means an
empty ladder, never one scored against an invented reference price.

### Expected time to converge

Primary: OU half-life `t½ = ln2/θ` when the fit qualifies.
Fallback: empirical median time-to-revert of winning episodes.
Otherwise `null → "—"`. This `t_hold` also feeds the funding-carry term
above — longer expected holds accrue more funding drag.

### Expected profit (ranking key)

```
net_capture_bps(q*) = dev − F(q*) − C_f(t_hold)   (signed; negative dev ⇒ losing direction)
expected_profit_bps = net_capture_bps(q*) × P_win
expected_profit_usd = expected_profit_bps × q* × 10⁻⁴
```

Null `P_win` or unknown `t_hold` → the dependent fields are `null` and the
row sorts below ranked rows (marked collecting/degraded), never scored on
an invented number.

---

## Signal Lifecycle

Each ordered pair row carries a state machine (as implemented in
`monitor.rs::evaluate_pair`):

```
collecting ──(history span ≥ window ∧ ≥10 aligned samples)──▶ watching
watching ──(dev_exec ≥ entry threshold — positive only; the pair is
            directional and a negative deviation belongs to the
            reverse-direction row)──▶ signaled
signaled ──(variation gate ∧ p_win known ∧ funding known ∧ ladder
            non-empty — a real baseline exists)──▶ qualified
signaled/qualified ──(dev < threshold)──▶ watching
any ──(either book stale/missing)──▶ stale (never executable)
```

- **Qualification window**: `auto` = `max(halfLifeMultiplier × OU
  half-life, minWindowSecs)` — default `2 × half-life`, 30-min floor; or
  `manual` = `manualWindowSecs` (default 24h for market-hours coverage).
  Samples persist via `spread_samples`, so restarts don't restart learning.
- **Noise filter** (`variation_covers_friction`): within the window, every
  half-life chunk (manual mode: each half-window) must show
  `max|dev| ≥ frictionCoverMult × friction_bps` — a pair whose motion never
  clears its costs is noise and stays `signaled`, never `qualified`.
- **Entry threshold**: `dev_exec = edge_exec − μ ≥ friction_bps` (strictly
  positive — signaling on `|dev|` would fire on the losing direction).
  Friction = fees + funding carry (+ exit estimate in two-way mode);
  book-walk slippage is priced per level inside the capacity ladder.
- **Directional honesty**: `net_capture_bps` and the profit fallbacks are
  signed — a negative-deviation row displays a negative expected value,
  never `abs()`'d into a phantom gain.
- **Exit estimate (two-way)**: exit fees + current top-of-book crossing on
  both books `(ask−bid)/mid`. A floor, not a guarantee — books at depth
  may be worse.
- **Execute gate**: `qualified` + fresh books + `p_win ≥ min_win_prob` +
  funding known + `optimal_size > 0` + no blocking signal on the symbol +
  paper mode + engine not paused. Any failure disables the button with the
  reason surfaced.
- **Signal age**: `firstSeenMs`/`lastSeenMs` persist to the
  `monitor_signals` table on transitions (±60s heartbeat granularity), so
  discovery time and duration survive restarts and long refresh gaps.
  `firstSeenMs` resets when a signal lapses to watching.

---

## Architecture

### Backend — `SpreadMonitorEngine` (new, `frontend/src-tauri/src/monitor.rs`)

A tokio background task owned by `AppEngineState`:

1. **Dynamic cadence under per-venue rate budgets** (`VenueLane`). Each
   admitted venue gets a rolling 60s request counter and a budget derived
   from the *lowest enabled account tier* on that venue — Lighter applies
   the lowest limit across accounts sharing an IP: standard 60/min,
   plus 1200/min (conservative map of the 120k weighted budget), premium
   2400, builder 4800; Hyperliquid 500/min usable (of the documented
   ~600/min for weight-2 `l2Book` calls); unknown venues fail closed to
   60/min. `txReservePct` (default 25%) is always held back for
   transaction traffic (it also covers the UI's own book polling, which
   shares the same source IP). Per-symbol refresh =
   `symbols × 60s / usable` clamped to
   `[minSymbolIntervalSecs, maxSymbolIntervalSecs]`, fetched round-robin —
   cadence degrades gracefully as the watchlist grows.
   **Request weighting**: a Lighter-family book read is two HTTP calls
   (`orderBookDetails` + `orderBookOrders`); it spends two budget slots so
   the rolling per-IP counter stays honest. Queued fetches run
   concurrently (JoinSet) — a slow venue never serializes a cycle; the
   adapter's `AccountRequestCoordinator` still enforces its min-interval
   per venue, so bursts drain through the existing throttle.
   Reads run once per venue lane (default account's tier governs); there
   is no per-account duplication. Failures back off exponentially
   (2^streak ≤ 120s) and the lane resumes automatically — a venue outage
   marks its pairs `stale`, it never freezes the loop.
   **Canonical symbols**: monitor samples persist under the base symbol
   (`BTC`); evaluation merges the `BASE`, `BASE-USDC`, and `BASE-USD`
   sample keys so history written by the legacy ticker-flow path (under
   UI symbols like `BTC-USDC`) feeds the same baseline. Venue-specific
   spellings only ever exist inside adapter request code.
2. Funding rates poll each lane every 60s through the bulk
   `fetch_funding_rates` endpoint (one request covers all markets) into a
   per-(venue, symbol) EMA; the hub gets each snapshot too. Lifecycle
   state (`first_seen`, `last_seen`, status, last edge/dev) persists to
   `monitor_signals` on transitions; `spread_samples` keeps the raw
   history.
3. **Watchlist** = symbols listed on ≥2 admitted venues per the latest
   successful `fetch_markets()` per venue, refreshed **hourly** and cached —
   the list follows account enable/disable automatically. A ticker's
   **exempt flag** (set in its strategy settings) removes it from the
   watchlist and all monitoring/strategy evaluation; a system setting
   reveals all exempted tickers globally. If markets can't be fetched the
   watchlist is empty and the monitor reports `collecting` — never a
   fabricated list.

### IPC surface (as built)

- `get_monitor_snapshot() -> MonitorSnapshotDto` — full snapshot: rows
  sorted by `expected_profit_bps` desc, watchlist, per-venue lane status
  (budget used/limit, refresh interval, tier, backoff), exempted list.
- `get_monitor_config / set_monitor_config` — `MonitorConfigDto` persisted
  as one JSON row in `monitor_config` (migration V5).
- `set_ticker_exempted(symbol, exempt)` / `get_exempted_tickers()` /
  `reveal_all_tickers()` — durable per-ticker exemption + global undo.
- `execute_monitor_opportunity(symbol, buy_venue, sell_venue, size_usd)` —
  re-validates the row is still qualified/executable in the latest
  snapshot, converts USD→qty at the live ask, then calls the existing
  `execute_spatial_arbitrage_impl` paper path. Blocking in-flight signals
  still block (delta-safety preserved).

### DTO (as built, camelCase over IPC)

```rust
pub struct MonitorRowDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub status: String,                    // collecting|watching|signaled|qualified|stale
    pub current_edge_bps: Option<f64>,     // executable edge, sell bid vs buy ask
    pub baseline_bps: Option<f64>,         // μ_pair — historical mid-mid mean
    pub deviation_bps: Option<f64>,        // current edge − baseline
    pub net_capture_bps: Option<f64>,      // expected net bps at optimal size
    pub p_win: Option<f64>,                // empirical conditional; null = thin data
    pub expected_profit_bps: Option<f64>,  // net_capture × p_win — ranking key
    pub expected_profit_usd: Option<f64>,  // expected USD at optimal size
    pub optimal_size_usd: Option<f64>,     // argmax expected USD (capped)
    pub breakeven_size_usd: Option<f64>,   // largest size: E>0 ∧ marginal p ≥ p_min
    pub capacity_ladder: Vec<CapacityTierDto>, // {sizeUsd,filledUsd,netBps,expectedNetBps,marginalPWin}
    pub funding_carry_bps: Option<f64>,    // signed; null => funding_unknown
    pub funding_unknown: bool,
    pub first_seen_ms: Option<i64>,        // signal discovery (durable)
    pub last_seen_ms: Option<i64>,
    pub signal_age_secs: Option<f64>,
    pub expected_converge_secs: Option<f64>, // OU half-life → empirical median → null
    pub qualification_progress: f64,       // 0..1 while collecting
    pub observed_episodes: usize,
    pub executable: bool,
    pub block_reason: Option<String>,
}
```

### Frontend — "Radar" nav tab

`OpportunityRadar.tsx` mounts under `activeTab === "radar"`: self-polls
`get_monitor_snapshot` every 5s while mounted; filter bar (ticker search,
buy/sell venue selects, min-E[bps], min-P%); status chips with
qualification-progress % on `collecting`; per-row expandable capacity
ladder (mini bar chart over tiers + optimal/breakeven callouts); per-row
size input + Execute button (disabled with `blockReason` tooltip).
A config drawer edits every `MonitorConfigDto` knob. Missing values render
as "—", funding-unknown as "?".

**Capacity column** — the profit-vs-size curve, shown intuitively:

- Cell headline: `net_bps@optimal` over `optimal_size` (e.g. `9.1bp / $25k`),
  with `breakeven_size` as a subtle sub-line (`max $84k`).
- Hover/expand reveals the full ladder as a compact tier table or mini
  bar-chart (`$10k→12.4 · $25k→9.1 · $50k→4.7 · $100k→−1.2`) — the decaying
  bars make "profit shrinks as you size up" legible at a glance.
- **Funding** shown as a signed chip on the expected-return cell
  (`carry −2.3bp`) — visually distinguished when positive (carry works for
  us) vs negative (drag); `funding_unknown` rows show a `?` marker so the
  adjustment's absence is explicit.

Honesty rules carried over: `null` renders as "—", never 0; `collecting`
rows show data progress instead of metrics; `stale` rows are greyed and
unrankable.

---

## Phase 2 seam — autonomous opportunities bot

- `SpreadMonitorEngine::qualified_opportunities()` exposes the same rows
  the UI consumes (status == qualified && executable). The future bot polls
  that stream, applies its own risk gates (max concurrent positions,
  per-venue notional caps, circuit breaker, delta guard) and calls the same
  `execute_spatial_arbitrage_impl`.
- `monitor_signals` (lifecycle persistence: status + first/last-seen +
  last edge/dev per pair) landed with v1 so discovery/duration survive
  restarts; a signal-event audit log still lands with the bot.

---

## Failure & Honesty Constraints

- No fabricated probabilities, capacities, or convergence times — every
  estimator reports `null` below its data floor.
- `collecting` is a first-class state, distinct from "no opportunity".
- Venue unreachable / WAF-blocked → pair goes `stale`, never shows stale
  numbers as live.
- Fees always from `venue_fee_bps`; volumes from `cached_venue_volumes`;
  books only via `market_data_hub` validity.
- Live mode: monitor is research-visible but all rows are non-executable
  (execution stays disabled, consistent with the rest of the app).

---

## Testing Decisions

- **Unit (arbitrageur-math)**: pair-spread series builder, episode
  segmentation + hit-rate estimator, OU probability fallback, capacity math
  on synthetic books.
- **Backend (commands/monitor)**: qualification transitions, null-when-
  insufficient-data, watchlist derivation, blocked-execution path, stale
  handling, restart rebuild from `spread_samples`.
- **Frontend**: row rendering incl. `—` for nulls, filters, sorting,
  collecting-state display, disabled-execute on blocked symbols.

---

## Open Questions for the User

All resolved — see **Decisions Locked** at the top of this document.
