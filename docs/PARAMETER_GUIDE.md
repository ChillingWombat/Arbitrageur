# Manual Strategy Parameter Guide

This guide explains how to set parameters without treating software defaults as profitable recommendations. Arbitrageur does not ship Conservative/Balanced/Aggressive trading presets yet because the repository does not contain a sufficiently broad, reviewed outcome corpus from which to derive them honestly.

Use Testnet mode, collect venue-specific history, review AutoTune's held-out results, and change one risk dimension at a time.

## General rules

1. **Start from executable friction, not gross spread.** Entry, exit, fees, slippage, funding, and quantity conversion all matter.
2. **Unknown means blocked.** Missing depth, stale books, missing funding, provisional session data, or insufficient history must not be replaced with zero.
3. **Size to the weaker leg.** Capacity is bounded by the shallower book and the smaller available margin pool.
4. **Use leverage last.** First establish stable fills, reconciliation, and drawdown at 1x–2x on venue testnets.
5. **Do not infer live readiness from a backtest.** Venue order handling and restart reconciliation are separate release gates.
6. **Treat annualized carry as a comparison, not a forecast.** Funding can change or invert before costs are recovered.

## Simple Spread

### Target Profit (BPS)

The target is the minimum expected net capture after modeled round-trip friction:

```text
required gross BPS
  = entry fees
  + expected entry slippage
  + exit fees
  + expected exit crossing/slippage
  + target profit BPS
```

Raise the target when realized testnet slippage or failed/partial fills exceed the model. Lower it only when a held-out sample still has positive expected net capture after costs.

### Sliding Window

- **Manual:** Use a horizon long enough to contain multiple independent divergence/reversion episodes. A 24-hour software default covers a full crypto day but is not universally optimal.
- **Auto:** Uses qualified pair history and OU half-life gates. When evidence is insufficient, the system must display and use the manual fallback.
- Avoid tuning the window to one recent profitable episode.

### Window Percentile

**Saved + display only.** The percentile parameterizes the baseline snapshot the Ticker Configurator renders — the live entry gate does not consult a spread percentile; it requires expected net ≥ Target Profit after route friction. The monitor's own qualification window (Radar Configurator) governs live scanning.

### Step size and ticker equity cap

**Partially live (2026-09-23).** The derived `max_accumulated_units` (max steps) persists on the ticker config and now caps concurrently-open matched pairs for that symbol — counted from the durable signal ledger (`balanced` rows), exact after restarts. Entry quantity still comes from the book-depth capacity ladder; `stepSizeMult`/`maxStepUsd` derive the stored step notional but are not yet a sizing input.

- Reduce `maxStepUsd` before increasing the number of opportunities monitored.
- Keep the ticker equity cap small enough that one unresolved pair cannot dominate the portfolio.
- If either venue cannot fill one step inside the slippage/depth guards, the step is too large regardless of account equity.

### Execution mode

**Live per-ticker override (2026-09-23).** The Ticker Configurator's Single/Two-Way toggle persists on the ticker config and overrides the global Radar Configurator → Execution Mode (`one_way`/`two_way`) for that symbol's exit costing; tickers without a saved config follow the global policy. The strategy-params mirror of the same field remains unconsumed.

- **Single-Way:** One directional divergence at a time. This is the safer default while validating a pair.
- **Double-Way:** Requires independent evidence that both directional rows mean-revert after all costs. Do not enable merely because the historical mid spread crossed zero.

### Leverage

Effective leverage is the minimum of strategy target, account cap, venue capability, and global Risk Control cap. A lower setting increases required margin but improves liquidation distance. A configured target is not proof that the venue accepted the leverage update.

## Funding carry

### Trade size

Size is per leg. For perp/perp carry, each venue must cover its own initial margin. For spot/perp basis on one venue, required capital is approximately:

```text
spot notional + perpetual initial margin + fee/slippage reserve
```

The same free-margin balance must not be counted twice.

### Minimum APR

APR is a normalized comparison of the current funding gap. It is not a guaranteed realized return. The entry hurdle should cover:

- Both legs' entry and exit fees
- Measured entry price gap
- Expected exit friction
- Funding decay/inversion risk
- Capital lock-up and transfer constraints

Use expected USD and trailing outcome distributions alongside APR.

### Maximum slippage

This bounds each leg's depth-walk slippage at dispatch. A tighter value rejects more opportunities; a wider value permits more adverse execution. Increase it only after testnet fill evidence shows that the additional fills improve net expectancy.

### Maximum entry spread

This bounds the absolute execution-price gap between legs. It protects against entering a funding trade whose basis move can overwhelm expected carry. It is independent of the funding-rate hurdle and must be enforced at dispatch.

### Spread-divergence ceiling (risk setting)

`risk.cb_max_spread_bps` (default 250) is the plausibility ceiling on an entry's expected net spread after friction. A route whose net edge exceeds it is refused outright — an edge that large is a symbol collision or corrupt quote, not alpha (production observed SPX500 quoting ~147M bps across Hyperliquid/Vanta). The gate sits in the shared spatial entry path, so manual, monitor, candidates, and radar-bot entries all honor it. Raise it only with evidence of real dislocations beyond 2.5%.

### Basis controls

- Basis rows are long spot / short perpetual only.
- The spot and perpetual symbols remain distinct.
- Reverse basis requires verified borrow/short-spot support and is not implemented.
- Keep the basis floor disabled unless its sign and units are understood: positive means the perpetual is above spot.

### Holding horizon

Break-even time assumes the current gap persists. The decay-aware expected hold is preferable but remains a model. Exit when the strategy's validated condition fires; do not hold solely to recover sunk fees.

## Triangle

A triangle is executable only when all three exact markets exist on the same venue and the full quantity can be priced through all legs. Use the minimum net-profit hurdle after three fees and three depth walks. A venue that silently maps a cross-quoted symbol to a USD market must be rejected.

## Spot and prediction

Integrated spot-arbitrage and prediction execution are disabled. Their configurator fields are research/schema placeholders and must not be interpreted as active risk controls.

## Dormant parameters (saved but not enforced)

These fields persist, validate, and round-trip through the UI, but no live runtime path consumes them yet. The UI marks each one "NOT YET ENFORCED" where it is rendered; treat every one as inert until wired:

| Field | Surface | Status |
|---|---|---|
| `funding_auto_rebalance` | Funding → Risk & Rebalance | Dormant — no rebalance supervisor exists |
| `funding_rotation_threshold_bps` | Funding → Risk & Rebalance | Dormant — no rotation supervisor exists (`rotation_improves` is library-only) |
| `funding_auto_exit_spread_pct` | Funding → Execution & Slippage | Dormant — exits run on the min-APR supervisor + operator flatten; this spread trigger has no consumer |
| `funding_liq_trim_pct` | Funding → Risk & Rebalance | Dormant — liquidation-trim watcher is futures-only |
| `spot_liq_trim_pct` | Spot → Risk & Safety | Dormant — same watcher is futures-only |
| `prediction_liq_trim_pct` | persisted, not rendered | Dormant — no prediction supervisor |
| `triangle_max_slippage_bps` | Triangle → Cycle Guardrails | Dormant — the cycle path consumes no slippage limit |
| `spot_trade_size_usd` / `spot_min_net_spread_bps` / `spot_max_slippage_bps` | Spot → Spatial | Dormant — spot scan and execute are both stubbed |
| `execution_mode` (per-ticker) | Ticker Configurator → Signal Quality | **Live** — overrides global unwind mode per symbol; strategy-params mirror remains dormant |
| `max_accumulated_units` (derived) | Ticker Configurator → Execution & Sizing | **Live** — caps open matched pairs per symbol via ledger count |
| `step_size_mult` / `max_step_usd` / `max_ticker_equity_pct` | Ticker Configurator → Execution & Sizing | Partial — derive the persisted cap; not yet a sizing input (capacity ladder sizes entries) |
| `funding_holding_days` | persisted, not rendered | Dormant — no consumer (default 14 flows nowhere) |
| `window_percentile` / `sliding_window*` | Ticker Configurator → Signal Quality | Display + autotune input only — not a live entry filter |
| `hsm_url` / `hmac_token` | Engine Settings | Reserved — remote-HSM integration stub by design |
| `fast_warmup_enabled` | persisted | Reserved — no consumer by design |
| `robinhood_rpc_url` | persisted | Reserved — no consumer by design |

## AutoTune

AutoTune uses a chronological fit/validation split and applies only rows marked `READY`. `HELD` means the current manual value remains authoritative.

Before applying:

- Confirm the symbol and venue pair.
- Check observation count and date span.
- Read both fit and validation metrics.
- Reject recommendations based on one regime or thin depth.
- Apply selected rows only; do not change every parameter merely because one row is READY.
- Preserve the audit ID for later outcome review.

AutoTune does not yet learn from a complete join of quoted opportunities to realized fills/slippage. Treat recommendations as research assistance, not an autonomous mandate.

## Current software defaults

These values initialize missing settings; they are not universal recommendations:

| Field | Default | Consumed? |
|---|---:|---|
| Target profit | 1 BPS | Live — entry net-spread floor |
| Window percentile | 50 | Display + autotune only |
| Sliding window | Manual, 24 hours | Display + autotune only |
| Step-size multiplier | 10 | Partial — derives persisted cap |
| Maximum step | $2,500 | Partial — derives persisted cap |
| Maximum ticker equity | 20% | Live — derives the open-pairs cap |
| Target leverage | 5x | Live — effective-leverage resolution |
| Futures max slippage | 15 BPS | Live — dispatch VWAP gate |
| Funding trade size | $25,000 per leg | Live — scan sizing arg |
| Funding minimum APR | 8% | Live — scan hurdle **and** exit floor: the 60s supervisor flattens an open carry/basis when EMA-smoothed net funding decays below it |
| Funding max slippage | 15 BPS | Live — dispatch VWAP gate |
| Funding max entry spread | 50 BPS | Live — dispatch gate |
| Funding auto-exit spread | off | Dormant |
| Funding rotation threshold | off | Dormant |
| Funding auto-rebalance | off | Dormant |
| Funding holding days | 14 | Dormant (no consumer) |
| Spot trade size | $1,000 | Dormant (strategy stubbed) |
| Spot max slippage | 5 BPS | Dormant (strategy stubbed) |
| Spot minimum net spread | 15 BPS | Dormant (strategy stubbed) |
| Triangle trade size | — | Live — cycle sizing |
| Triangle min profit | — | Live — cycle hurdle |
| Triangle max leg slippage | 5 BPS | Dormant |

Review and reduce these values for the actual account before any testnet run. Live strategy entries are additionally opt-in gated (`live_entry_enabled`) with a per-leg notional cap.

## Storage retention

`spread_samples_max_days` and `spread_samples_max_gb` (Engine Settings → storage/retention inputs, persisted as `retention.max_days`/`retention.max_gb`) bound the always-on market-data recorder via the hourly `enforce_retention_pass`. Both default to **unlimited**. Measured growth on a 9-venue watchlist (2026-09-13→23): ~83 MB/day — `spread_samples` dominates (4.9M rows, 831 MB total DB after 10 days). The same day-cutoff also bounds `venue_volume_samples`, `monitor_signal_events`, `margin_snapshots`, and `funding_rate_samples`. Analytics consumers never read beyond 30 days (`BASELINE_RETENTION_MS`), so `max_days = 30` loses no model input. `max_gb` caps the DB's *used* bytes (allocated minus freelist — the file's own size cannot shrink on deletes alone); after evictions the pass runs a bounded `PRAGMA incremental_vacuum` so the file converges toward the cap over successive hourly runs. `session_profiles`/`session_hour_bins` are derived rollups and are never pruned.

## Why fixed risk presets are still gated

A named preset creates an implied recommendation. To publish one responsibly, the project needs:

- A versioned source dataset
- Venue and asset-class coverage
- Held-out outcome metrics after fees/slippage
- Drawdown and unresolved-leg statistics
- Stability across sessions and regimes
- Documented sample thresholds and invalidation rules

Until that evidence exists, the honest cold-start behavior is a clearly labeled software default plus manual guidance, not fabricated risk tiers.
