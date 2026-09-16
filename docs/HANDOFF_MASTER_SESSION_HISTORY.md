# Master Handoff & Session History: Arbitrager Cross-Market Quantitative Terminal

> **Historical record.** This document captures the development history as it happened. Some feature claims below describe aspirational or since-disabled functionality; for the current, accurate safety and capability status, see `README.md` → *Current Safety Status*.

**Document Version**: 2.0.0  
**Generated At**: 2026-09-12  
**Repository**: `/mnt/Data/Projects/Arbitrager`  
**Application**: Arbitrager (High-Frequency Multi-Venue Arbitrage & Quantitative Terminal)  
**Primary Stack**: Rust (Cargo Workspace), Tauri v2, Pixi, React 18, TypeScript, Tailwind CSS

---

## 1. Executive Summary & Purpose

This document provides a comprehensive, unified record of all user requirements, questions, investigative sessions, architectural iterations, and bug resolutions across both **local development environments** and **remote cloud deployments** (`panda-popdex` VPS). It is designed to allow any incoming agent or engineer to immediately understand the entire history, operational lessons, quant models, failure modes, and current state of the project without losing context.

---

## 2. Chronological Synthesis of All User Tasks & Messages

### Phase 1: Cloud Server Exploration & Bot Reverse Engineering (`panda-popdex`)
The project originated with hands-on testing, deployment, and decompilation of open-source arbitrage tools on an Ubuntu cloud server (`ubuntu@panda-popdex`, `/opt/pandazhai/popdex`):
- **User Task & Tool Testing**:
  - Downloaded and analyzed `PandaZhai-Arbitrage-Tool` (`panda-arb` and `panda-node`).
  - Explored `RobinHoodLighter---Lighter-Arbitrage-Tool` and PopDEX authorization (`popdex-agent-authorize`).
  - Tested multi-bot deployment across server ports (`9100`, `9101`, etc.).
- **Crucial Operational Discoveries Identified by User**:
  1. **Why BTC / Single-Step Was Not Winning**: The user observed that high target spreads (high BPS hurdles) rarely filled, and when they did, adverse selection occurred on trending moves. Tight, frequent steps with low target BPS captured high turnover.
  2. **The "Compensated" State & Pause**: When one execution leg succeeds on Exchange A but fails or experiences severe slippage on Exchange B, the bot enters a "Compensated" state. The bot compensates the unhedged inventory and automatically pauses to prevent cascading liquidation.
  3. **Recovery Procedure**: When one leg fails, naive bots attempt to market-dump the surviving leg immediately, suffering double taker fees and slippage. The recovery procedure staggers unwinds or places passive limit orders to balance inventory back to flat delta.
  4. **Double-Way vs. Single-Way Convergence**: In pairs like WLD or ETH, spreads oscillate symmetrically above and below baseline. "Double-Way" enables entering both when Exchange A > B and when B > A, doubling capital efficiency.
  5. **Friction & Spread Deductions**: Real profits only exist after deducting exchange taker fees on entry, taker fees on exit, and the bid-ask closing spread friction.
  6. **Asymmetric Profit Accrual**: The user observed in live trading that net gains were concentrated almost entirely on the *illiquid* venue of the pair, where stale market maker quotes linger.
  7. **Language Performance Limit**: Python proved too slow for sub-millisecond atomic orderbook evaluation and race execution, motivating the full native Rust rewrite.

---

### Phase 2: Native Rust Engine Architecture & Quant Models
The user requested a ground-up Rust engine managed with Pixi:
- **Core Requirement**:
  *"Create a Cross-market Arbitrager tool for Price Difference and Funding rate difference across multiple markets at the same time. Blockchain coins first, then predictive markets, then traditional markets. Web3: Hyperliquid, Lighter, Vanta.exchange, Robinhood, Arcus, PopDEX. Use Rust for speed. Modular design with Pixi."*
- **Mathematical Formulations Built**:
  1. **Spatial Spread Dislocation**: Evaluates orderbook bid/ask depth VWAP minus taker fees across all permutations $(i, j)$ where $i \ne j$.
  2. **Funding Rate Carry (Delta-Neutral APR)**: Normalizes disparate funding periods (Hyperliquid 1h vs. Lighter 8h) to continuous hourly rates and compounds to annual percentage rate:
     $$r_{\text{hourly}} = \frac{F}{\text{Interval Hours}}, \quad \text{APR} = r_{\text{hourly}} \times 8{,}760$$
  3. **Bertram Optimal Entry/Exit Thresholds**: Solves the optimal round-trip switching thresholds for mean-reverting spreads with transaction costs:
     $$x^*(c) = \mu \pm \sigma \sqrt{\frac{2 c}{\theta}}$$
  4. **Ornstein-Uhlenbeck (OU) Parameter Calibration**: Fits drift speed $\theta$, long-term mean $\mu$, and volatility $\sigma$ to empirical spread observations via maximum likelihood estimation (MLE).
  5. **Kalman Filter Dynamic Hedge Ratio**: Dynamically tracks time-varying beta ($\beta_t$) and intercept ($\alpha_t$) for statistical arbitrage pair trading ($P_y = \alpha + \beta P_x + \epsilon$).
  6. **Arrow-Debreu Synthetic Prediction Parity**: Risk-free synthetic arbitrage between Polymarket CTF and Kalshi CFTC binary event contracts:
     $$\text{Cost} = \text{Price}(\text{YES}) + \text{Price}(\text{NO}) + \text{Fees} < \$1.00 \implies \text{Arb Return} = \frac{\$1.00 - \text{Cost}}{\text{Cost}}$$

---

### Phase 3: Desktop UI & Terminal Architecture (Tauri v2 + TypeScript)
- **Framework Choice**: Tauri v2 with native Rust IPC bindings + React 18 + Tailwind CSS.
- **Theme & Design Language**:
  - User requested a minimalist light and blue theme inspired by `https://ddeng.me/`.
  - Removed flashy neon widgets, complex gradients, and decorative emojis in favor of high-density quantitative typography and clean tabular cockpits.
- **Layout & Navigation Evolution**:
  1. **Top Strategy Switcher**: Moved strategy navigation to the top header across 6 quantitative tabs:
     - `Spread Range` (Spatial Dislocation Matrix)
     - `Funding Rate` (Delta-Neutral Carry Radar)
     - `Spot Trade` (Cash Cross-Exchange Arbitrage)
     - `Triangle` (3-Leg Cycle CLOB Parity)
     - `Prediction Market` (Arrow-Debreu Synthetic Parity)
     - `Statistical` (Kalman Tracker, Bertram Solver, OU Process)
  2. **Left-Sidebar Ticker Explorer**:
     - Built a union list of 52 perpetual contracts present on at least 2 venues.
     - Implemented 3-way sorting controls: Alphabetical (A-Z), Transaction Count (most bot fills), and Total Net Profit.
  3. **2D Interactive Matrix Grid**:
     - Converted flat lists into a 2D matrix (Y-axis = Long Venue/Ticker, X-axis = Short Venue/Ticker).
     - Calculates rolling spread delta ($\Delta$ BPS) against 5 selectable sliding windows (10s, 30s, 1m, 5m, 15m).
     - Cell heatmap tinting and 1-click atomic execution pop-outs.
  4. **Pop-Out Ticker Strategy Configurator**:
     - Modal dialog allowing per-ticker parameter overrides (Target Profit BPS, Nominal Hurdle, Percentile Filter, Unit Accumulation, Max Steps, Safe Leverage).

---

### Phase 4: Vault Security, Layout Hardening & Zero-Synthetic Data (Latest)
- **User Requests Addressed**:
  1. *"Remove this tag"* / Clean up duplicate ticker badges and symbols.
  2. *"The information question mark symbol is not working properly"* -> Rebuilt `InfoTooltip` with hover cards, click outside dismiss, and downward positioning so content is never clipped by modal headers.
  3. *"When the vault is locked, this api exchange page is hidden by 'grey out' completely, with a tiny white window in the center asking to enter the passcode. The vault will be automatically locked after exiting the setting page for a minute, and also locked when the program is exited."*
     - Built `VaultUnlockModal` full-screen grey-out overlay with centered PIN dialog.
     - Added 60s inactivity auto-lock and exit lock hooks.
  4. *"Check and refine the layout, gaps between section, edges and fonts"* -> Modernized typography, rounded borders, and consistent spacing.
  5. *"Remove the lock indicator"* -> Removed redundant status badges from the exchange cards.
  6. *"Relocate the trading mode indicator to bottom left of the setting page"* -> Moved Paper/Live toggle to the bottom-left sidebar/footer.
  7. *"Move the execution section before the exchange section"* -> Reordered Settings so Execution & Sizing Guardrails appear first.
  8. *"Why are there still synthetic data?"* (User uploaded screenshot of Spot Trade tab showing `+-1816518.8 BPS` and badge `4` on Prediction tab):
     - **Root Cause 1 (Absurd BPS Bug)**: Spot fee calculation was multiplying `buy_price * trade_size * fee_bps / 10000`, calculating $181,650 fee on a $1,000 trade! Corrected to `(trade_size * buy_fee / 10000) + (trade_size * sell_fee / 10000)`.
     - **Root Cause 2 (Public Scraping)**: Scanner was hitting public Binance and Kraken endpoints even when zero API keys were configured.
     - **Root Cause 3 (Fake Prediction Fallbacks)**: Removed hardcoded fallback pairs (`FED-RATE-CUT-DECISION`, etc.) that fabricated badge `4`.
     - **Root Cause 4 (StatArb Fake Series)**: Removed `[mu - sigma, ...]` fallback array in `StatArbLab.tsx`.
     - **Enforced Fail-Closed**: All strategies return `0` opportunities with clean empty states when $< 2$ configured venues exist.

---

## 3. Project Directory Structure

```
/mnt/Data/Projects/Arbitrager/
├── Cargo.toml                    # Root Cargo workspace configuration
├── pixi.toml                     # Pixi environment & task definitions
├── .gitignore                    # Strictly ignores keychain.toml, *.key, target/
├── SPEC_CROSS_MARKET_ARBITRAGE_ENGINE_V2.md # Full quantitative engine specification
├── docs/                         # Technical documentation & quantitative research
│   ├── 01_QUANT_MATH_RESEARCH.md
│   ├── 02_REFERENCE_PROJECTS_ANALYSIS.md
│   ├── 03_SYSTEM_ARCHITECTURE.md
│   ├── 04_EXCHANGE_INTEGRATIONS_GUIDE.md
│   ├── 05_BUGS_SOLUTIONS_AND_RUNBOOK.md
│   ├── 06_MOB_STRATEGY_VAULTS_AND_RWA_RESEARCH.md
│   └── HANDOFF_MASTER_SESSION_HISTORY.md # [THIS FILE]
├── crates/                       # Modular Rust Backend Crates
│   ├── arbitrager-core/          # Domain models, Decimal math, ExchangeAdapter traits
│   ├── arbitrager-math/          # Spatial spread, OU process, Bertram solver, WFO
│   ├── arbitrager-execution/     # Concurrent execution coordinator, virtual ledger
│   └── arbitrager-strategy/      # Arbitrage strategies (Spatial, Carry, Prediction)
└── frontend/                     # Tauri Desktop & React UI
    ├── package.json              # Vite, React, Lucide-React, Tailwind CSS
    ├── src-tauri/                # Tauri v2 Desktop Host
    │   ├── Cargo.toml            # Tauri dependencies (Argon2id, AES-256-GCM)
    │   └── src/
    │       ├── main.rs           # Entry point, WebKitGTK env flags, command registration
    │       ├── commands.rs       # All IPC command handlers and 25 unit tests
    │       ├── state.rs          # Engine state, live adapters, credential filters
    │       └── vault.rs          # Encrypted credential vault implementation
    └── src/                      # React Frontend Source
        ├── App.tsx               # Root component, global state, tab switching
        ├── components/           # UI Components
        │   ├── EngineSettings.tsx# Settings & Vault configuration panel
        │   ├── Header.tsx        # Top navigation & system controls
        │   ├── LiveMatrix.tsx    # 2D Arbitrage Dislocation Matrix
        │   ├── Navigation.tsx    # Strategy switcher tabs
        │   ├── PortfolioMonitor.tsx # Venue balances & risk telemetry
        │   ├── PredictionHub.tsx # Prediction market synthetic parity
        │   ├── SpotArbitrageBot.tsx # Spot cross-exchange bot
        │   ├── StatArbLab.tsx    # Statistical arbitrage & OU fitting lab
        │   ├── TickerConfigModal.tsx # Per-ticker strategy parameter modal
        │   ├── TickerExplorer.tsx# 52-ticker left sidebar with 3-way sort
        │   ├── TriangularArbitrage.tsx # 3-leg cycle CLOB arbitrage
        │   └── VaultUnlockModal.tsx # Full grey-out PIN unlock dialog
        ├── services/
        │   └── api.ts            # API bridge (TauriApiService & mockEngine)
        └── types/
            ├── index.ts          # Strategy DTOs, Orderbooks, Quotes
            └── tickers.ts        # 52 Perp tickers, venue credential filters
```

---

## 4. Key Quantitative Models & Formulations Reference

### 4.1 Real Friction Spread Deduction
For any cross-venue pair $(A, B)$ where we Buy on $A$ and Sell on $B$:
$$\text{Gross Spread} = \text{Bid}_B - \text{Ask}_A$$
$$\text{Gross BPS} = \frac{\text{Bid}_B - \text{Ask}_A}{\text{Ask}_A} \times 10{,}000$$
$$\text{Round-Trip Friction} = \text{Fee}_A + \text{Fee}_B + \text{Closing Spread Friction}$$
$$\text{Net BPS} = \text{Gross BPS} - \text{Round-Trip Friction}$$

### 4.2 Sliding Window Spread Delta ($\Delta \text{BPS}$)
Baseline mean $\bar{S}_w$ is computed over rolling window $w \in \{10\text{s}, 30\text{s}, 1\text{m}, 5\text{m}, 15\text{m}\}$:
$$\Delta \text{BPS} = \text{Net BPS}_t - \bar{S}_{w, t}$$
- $\Delta \text{BPS} > 0$: Current dislocation is unusually wide relative to the recent baseline (prime entry condition).
- $\Delta \text{BPS} \le 0$: Dislocation has compressed back to or below average (exit / unwind condition).

### 4.3 Safe Leverage Sizing
Because margin pools across exchanges are isolated:
$$\text{Max Leverage} = \min \left( \text{UserTarget}, \left\lfloor \frac{1}{\text{Max Drawdown Buffer}} \right\rfloor, 5\text{x} \right)$$
- Standard recommendation: **2x - 3x**.
- Absolute hard ceiling: **5x**.

---

## 5. Build, Test, and Execution Commands

All commands can be run from the repository root:

```bash
# 1. Run full Rust workspace checks & tests
cargo check --workspace
cargo test --workspace

# 2. Run backend Tauri command tests (25 passing tests)
cd frontend/src-tauri
cargo test --bin arbitrager-desktop
cd ../..

# 3. Run frontend unit tests (node:test via --experimental-strip-types; 32 passing tests)
cd frontend
npm test
cd ..

# 4. Build frontend production assets
cd frontend
npm run build
cd ..

# 5. Compile release desktop binary
cargo build --release --bin arbitrager-desktop

# 6. Launch release binary on local X11/Wayland display
DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 \
/mnt/Data/Projects/Arbitrager/target/release/arbitrager-desktop
```

---

## 6. Open Items & Backlog for Future Sessions

0. **Adaptive Dislocation Analytics & Auto-Calibrated Parameters** *(spec published — [GitHub Issue #3](https://github.com/Dav1D1010/Arbitrager/issues/3), label `ready-for-agent`)*:
   - Persist venue mid/dislocation samples in SQLite WAL; compute deviation series + analytics in `arbitrager-math` (not the frontend); chart windows default to the bot's trailing window and extend to days via rollups.
   - Seasonality (hour-of-day, day-of-week), per-pair mean/half-life, regime-shift detection; parameter recommendations (hurdle, window, units, leverage) validated out-of-sample via existing WFO module; opt-in auto-tune modes; curated default presets for distribution.
   - Data-first mandate: accumulate our own corpus and analyze patterns before offering auto-tune to external users.
1. **WebSocket Orderbook Streaming Integration** *(spec published — [GitHub Issue #4](https://github.com/Dav1D1010/Arbitrager/issues/4), label `ready-for-agent`)*:
   - Hyperliquid already features direct REST/WS endpoints.
   - Wire persistent WebSockets for Lighter zkDEX, Vanta (Orderly), and Robinhood EVM to achieve continuous sub-10ms quote updates. Robinhood has no CLOB API — spec routes it to honestly-attributed on-chain pool quotes; Vanta/Robinhood now fail closed until feeds land (post-sweep they contribute zero orderbook data).
2. **Triangular Backend Migration**:
   - `TriangularArbitrage.tsx` currently delegates to `api.scanTriangleOpportunities`.
   - Implement native `scan_triangular_opportunities` and `execute_triangular_arbitrage` in `frontend/src-tauri/src/commands.rs`.
3. **Live Transaction Signing**:
   - Hyperliquid EIP-712 agent wallet signing is scaffolded in `arbitrager-core`.
   - Connect live signing when Vault is unlocked and trading mode is switched to `Live`.
4. **Cloud Multi-Bot Deployment Scripts**:
   - Package systemd service templates (`arbitrager@9100.service`, `arbitrager@9101.service`) for multi-instance deployment on remote Ubuntu VPS instances.

---

## 7. Session Handoff — 2026-09-13 (UI Truthfulness, Lifecycle Controls, Liquidity Sort, Signal Trend)

### 7.1 Theme
Continuation of the fail-closed, zero-synthetic-data posture: purged remaining fabricated UI fallbacks, unified strategy-page ribbon/lifecycle behavior, split telemetry from execution history, automated the log stream, added a real-data liquidity sort, and shipped the Signal Trend dislocation chart — culminating in a published spec for the adaptive analytics/calibration subsystem.

### 7.2 Synthetic-Data Purge (final sweep)
- **StatArbLab**: fabricated fallbacks removed — `18.6200` hedge ratio, `$0.00` residual, `0.00 Z` now render `—`; Z-score no longer claims profit-green on a fake zero.
- **PortfolioMonitor**: fake `+0.0%`/`100.0% safe` liquidation-buffer fallback and 100%-full green bar replaced with `—` and an empty bar; same for margin utilization. Fake `max_drawdown_limit_usd` fallback removed.
- **Kept deliberately (not synthetic)**: FundingRadar `$25,000` = user-configured `fundingTradeSize`; PredictionHub `$1.00` payout = structural Arrow-Debreu settlement constant.
- **TickerLogMonitor heartbeat**: hardcoded `estFees = 7.6 BPS` replaced by per-route fees derived from `EXCHANGES_ALPHABETICAL` venue metadata; unresolved routes report gross spread + "route fees unresolved".

### 7.3 Bot Lifecycle Controls — Unified Gating
- `BotLifecycleControls.tsx` shared component: Start/Pause/Reduce Only/Stop + phase badge.
- **`disabled` gates Start only** (entry action); Pause, Reduce Only, and Stop stay available per phase/positions so risk-reducing exits are never locked out when connectivity drops mid-run.
- Disabled buttons keep visible edges: `border-surface-border` + `bg-surface-raised/40` (was `border-transparent`).
- All five strategy pages accept `hasApiConnectivity` (default `false`, fail-closed), wired from `App.tsx` as `activeVenueCodes.size >= 2` — same source as Spread Range.

### 7.4 Log Monitor Automation
- "Manual Scanning" removed → feed-status dot: **Live Feed** (pulsing) / **Awaiting Market Data**; PAUSED badge retained.
- Telemetry logs generate on **every** orderbook update regardless of bot state; hurdle breaches log `TRIGGER` when running vs `EVAL` ("entries disabled, monitoring only") when idle.
- Per-symbol persistence to `localStorage` (`ticker-log-monitor:<symbol>`, capped 500); Clear wipes the persisted stream.
- Depth filter dropdown: Last 50/100/200/500, Last 1h, Last 24h, All.

### 7.5 Telemetry/History Split & Ribbon Alignment
- `TickerTelemetryGadget.tsx` (new): KPI grid + realized PnL + win/fill rate + trigger spread + baseline mean + dislocation Δ + tuning assistant; aggregates unfiltered by table filters.
- `TickerTransactionHistory.tsx`: history-only (search/status/direction filters + executions table).
- LiveMatrix dock tabs: Ranked Opportunities · **Signal Trend** · Execution History · Telemetry · Log Monitor.
- Spread Range ribbon restructured to the standard strategy layout (`px-4 py-3`, `STRATEGY PARAMETERS` title, uniform chips, Configure → lifecycle → error ordering); no fabricated param defaults without a connected ticker.

### 7.6 PortfolioMonitor
- Removed subtitle + per-card captions + manual Refresh button (`riskData`/`status` already arrive on App's 3s poll; strategy metrics now poll on the same cadence inside the component).
- New **Monitor:** dropdown (default **All**) scoping Realized PnL, Cycle Win Rate, Sharpe/APR, and the attribution grid to a selected strategy; account-level tiles (equity, liquidation buffer, circuit breaker) stay global.

### 7.7 Liquidity Sort (real data, new backend path)
- `ExchangeAdapter::fetch_volumes_24h()` → `HashMap<asset, Decimal>`; default returns empty (non-reporting venues contribute nothing).
- Hyperliquid: one `metaAndAssetCtxs` call → `dayNtlVlm` per asset. Lighter: `orderBookDetails` → `daily_quote_token_volume` (fallback `daily_base_token_volume × last_trade_price`); unreported assets skipped, not zeroed.
- New `get_ticker_volumes` command → `Vec<VenueVolumesDto>` (per-venue raw maps, attribution preserved); registered in `main.rs` + `NATIVE_COMMANDS`.
- Frontend: `getVenueAssetVolumeUsd` normalizes venue asset names (case-insensitive; `kPEPE`/`1000X` → base); `getTickerAvgVolumeUsd` means across reporting venues; `TickerSortOption` adds `"liquidity"`; TickerExplorer sorts desc with unreported tickers last (never fabricated as 0), row sub-line shows `vol $X.XB 24h` under liquidity sort.
- `App.tsx` polls volumes every 30s, refetches when the connected venue set changes.

### 7.8 Signal Trend Gadget — durable Rust pipeline (implemented)
The gadget is now backend-driven end to end; no frontend math.

**Persistence** (`state_store.rs`, migration v3)
- `spread_samples` table: `(symbol, venue_id, ts_ms, mid, best_bid, best_ask)` with `(symbol, ts_ms)` index; WAL mode.
- `record_spread_samples` (batch tx, drops non-finite/non-positive mids, no-op on empty batches), `spread_samples(symbol, from, to)` range query, `earliest_spread_sample_ts`, `prune_spread_samples(retention, row_cap)`.
- `fetch_orderbooks_impl` records every poll's per-venue top-of-book best-effort (store failures never break market data); hourly prune keeps **30 days**, capped at **4M rows** (`AppEngineState::sample_prune_at` tracks the last prune hour).

**Math** (`arbitrager-math/src/dislocation.rs`)
- `compute_deviation_series(samples, weights, mode, bucket_ms, max_points)` → bucketed per-venue bps deviations from baseline + max–min dislocation band; null-preserved gaps; endpoint-preserving stride downsample to `max_points`.
- `BaselineMode::Consensus` = volume-weighted mean of mids (equal-weight when unreported); `BaselineMode::Leader` = highest-weight venue pinned at 0.
- 6 unit tests cover: weighting, leader anchoring, gap preservation, endpoint-preserving downsample, invalid-mid drops.

**Command** (`get_signal_series`)
- Validates symbol; clamps window 10s–7d; bucket = `max(window*1000/500, 1000ms)`; queries samples on `spawn_blocking`; weights venues by the cached 24h-volume maps (asset-name normalized: `BASE`/`kBASE`/`1000BASE`, case-insensitive).
- Returns `SignalSeriesDto`: `ts_ms[]`, `venues[]` (ids), `venue_names`, `dev_bps[i][j]` nullable matrix, `band_max/min_bps[]`, `sample_count`, `earliest_ts_ms`.
- Volume fetch now shared behind `cached_venue_volumes` (5-min TTL, `AppEngineState::volume_cache`); `get_ticker_volumes` consumes the same cache.

**Gadget** (`SignalTrendGadget.tsx`, rewritten)
- Polls `api.getSignalSeries` — 3s cadence for ≤15m windows, 15s beyond.
- **Default window = the bot's configured trailing window** (first chip shows the formatted duration only — the `BOT` prefix was removed in 9.16; tooltip still explains it mirrors the strategy's window); presets extend to **5m/15m/1h/4h/1d/3d/7d**.
- Per-venue lines break across null buckets (gaps stay gaps); band renders only where ≥2 venues observed; span-aware x labels (clock → date+time for multi-day); hover crosshair; legend chips rich→cheap with BUY/SELL extremes; `Spread X / hurdle Y BPS` badge.
- Honest states: `No recorded samples for {symbol} in this window` / `Collecting signal samples…` / `Signal series unavailable` on invoke failure; footer reports `N venue samples in window · Xh/Xd history recorded` + active baseline mode.

### 7.9 Published Spec
- **GitHub Issue #3** — *Adaptive Dislocation Analytics & Auto-Calibrated Strategy Parameters* (`ready-for-agent`): durable dislocation sampling, Rust-side series + analytics, seasonality, WFO-guarded parameter recommendations, opt-in auto-tune, curated presets. See §6 item 0.

### 7.10 Working Tree State
- All changes **uncommitted**; `git status` shows modifications across `crates/` (exchange.rs, hyperliquid.rs, lighter.rs), `src-tauri` (commands.rs, main.rs, state.rs, vault.rs, state_store.rs new), and most frontend components. New files: `SignalTrendGadget.tsx`, `TickerTelemetryGadget.tsx`, `BotLifecycleControls.tsx`, `ExchangeAccounts.tsx`.
- ~~Known caveat: `Ticker.volume_24h` in several adapters still returns placeholder constants~~ — **resolved post-handoff**: the dead field was removed from `Ticker` entirely (it was write-only, never consumed); real per-venue volume data continues to flow through `fetch_volumes_24h`. Prediction-market empty-book ticker fallbacks (`0.41/0.42`, `0.56/0.57`) were also zeroed for consistency.
- **Post-handoff sweep** also corrected `fetch_account` across live adapters: Vanta/Robinhood/Arcus/PopDex/Polymarket/Kalshi returned fully fabricated snapshots → now fail closed (`bail!`, matching `place_order`); Hyperliquid and Lighter positions now parse real API fields (`unrealizedPnl`/`unrealized_pnl`, `liquidationPx`/`liquidation_price`, `leverage.value`/`allocated_margin`, `marginUsed`, `positionValue`/`position_value` → mark). Lighter bugs fixed: `entry_price` → `avg_entry_price` (was always 0) and side now derives from `sign` (shorts were classified long). Lighter `maintenance_margin` uses `allocated_margin` as a conservative proxy (venue does not expose maintenance margin). Orderbook synthesis removed: Vanta no longer fabricates a ±1bp BBO from mark price (which also carried a hardcoded `60000` mark fallback), and Robinhood no longer proxies Binance/Hyperliquid books under its own `venue_id` — both now fail closed instead of corrupting `spread_samples` with misattributed quotes.

### 7.11 Verification (all green at handoff)
```bash
cargo test --workspace                  # all crates green (arbitrager-math 23, incl. 7 dislocation)
cd frontend/src-tauri && cargo test     # 96 pass / 0 fail (3 store + 5 signal-series/recorder tests)
cd frontend && npx tsc --noEmit         # clean
node --experimental-strip-types --test  # 32 pass / 0 fail (node:test)
npm run build                           # production build succeeds
```

**Edge cases audited and fixed in this pass:**
- Leader-mode baseline was nondeterministic under tied/empty weights (HashMap iteration order) — now weight-desc/index-asc; covered by `leader_tiebreak_is_deterministic`.
- "Spread / hurdle" badge could quote a stale bucket when recent polls are single-venue — now gated to the window head (≤2 buckets old).
- Single-bucket venue observations drew invisible `M`-only paths — now rendered as dots.
- Single-venue windows show `1 venue — pairwise spread needs ≥2 venues` instead of a misleading flat zero.
- `get_ticker_volumes` and `get_signal_series` share `cached_venue_volumes` (5-min TTL) so the chart's baseline weighting doesn't add exchange traffic.

---

## 8. Session Handoff — 2026-09-14 (Robinhood Live Data, Spread Range Monitor / Opportunity Radar)

### 8.1 Theme
Two workstreams: (a) completing the venue layer — Robinhood wired as a real Lighter-protocol adapter instead of a stub, latency probes verified/hardened, ticker universe driven by the backend's own admission report; (b) the **Spread Range Monitor** (spec `SPEC_SPREAD_RANGE_MONITOR.md`) — a system-wide opportunity radar across all spread-eligible tickers × all admitted venues with statistical qualification, capacity math, funding carry, and per-row paper execution. Everything remains research/paper-trading only; live signing/execution is still absent by design.

### 8.2 Robinhood — real adapter (was a stub)
- `RobinhoodAdapter` deleted; `LighterAdapter` is now venue-parameterized via `new_with_venue(base_url, venue_id)` — Robinhood Chain serves the Lighter protocol at `api.rh.lighter.xyz`, so `live_robinhood` is a genuine adapter with venue id `robinhood`.
- `get_venues` admits it on flag-or-account → real `ping()` latency probe (~930ms AU→origin, honest red tier) and real `fetch_markets`/`fetch_orderbook`.
- `get_paper_exchange`/`paper_exchanges()`/`canonical_venue_id` include robinhood → its seeded $100k sim shows in the balance panel and monitor/execution paths resolve it.
- **Bug found while wiring**: `total_realized_profit` had a hardcoded `dec!(200000)` baseline — `SimulatedExchange` now tracks `initial_equity` (updated by `reset_account`, which also clears positions so a paper-balance reset can't fabricate PnL); the baseline is `state.paper_baseline_equity()` = sum of actual sim seeds.

### 8.3 Latency & UI truthfulness fixes
- `ping()` hardened: requires `Content-Type: json` — a WAF 200 HTML challenge no longer counts as a fast fake success.
- Both displayed latencies verified live: Robinhood ~0.9s/req (EMA 669ms shown), Lighter ~96ms warm keep-alive (curl pays fresh TCP+TLS → ~160ms).
- "No available tickers" bug: `availableTickers` read stale frontend engine flags and never mapped robinhood. Now derives from `systemStatus.active_venues` (backend's real admission, 3s poll); `getAdmittedVenueCodes` added in `tickers.ts`, `VENUE_ID_TO_CODE` already covered `robinhood→RH`.
- Settings-page scroll jank: removed full-screen `backdrop-blur` (GPU re-composite per frame), `EngineSettings` wrapped in `React.memo` with venue-aware equality (was re-rendering the ~2400-line tree every 3s status poll), App callbacks stabilized with `useCallback`.

### 8.4 Spread Range Monitor — backend
- **Math** (`arbitrager-math/src/pair_monitor.rs`, new): time-aligned directional pair series; episode segmentation (open on threshold, win on revert-to-band, stop at k·σ, else open); empirical conditional `P_win(dev)` — for a fresh entry at deviation x the lookup uses the *smallest observed threshold ≥ x* (excursions whose peak reached at least x); capacity ladder walking **both** books with per-level deviation→probability weighting, `optimal_size_usd` (argmax expected USD under participation cap) and `breakeven_size_usd` (largest tier with E>0 **and** marginal p ≥ `minWinProbability`, default 0.90); conservative funding carry (`f_A − f_B` × horizon, cost→`max(t̂,8h)`, gain→`min(t̂,1h)`). All outputs fail closed below data floors — never fabricated zeroes.
- **Bulk funding** on `ExchangeAdapter::fetch_funding_rates()` (default empty): Lighter `/api/v1/funding-rates` filtered to the venue's own entries; Hyperliquid `metaAndAssetCtxs` (one call, all coins). Feeds a per-(venue,symbol) EMA — funding is noisy, snapshots lie.
- **Schema V4**: `account_tier` on `exchange_accounts` (standard/plus/premium/builder; Plus adds the documented +0.5bps), `ticker_exemptions` table. **Schema V5**: `monitor_config` JSON row + `monitor_signals` durable lifecycle table.
- **`SpreadMonitorEngine`** (`frontend/src-tauri/src/monitor.rs`, tokio task spawned in `main.rs`):
  - One lane per admitted venue — **default account only**, no per-account data duplication.
  - Per-lane rolling 60s budget from the **lowest enabled account tier sharing the IP** (Lighter standard 60/min; plus/premium/builder mapped conservatively inside their weighted envelopes; Hyperliquid 500/min; unknown → 60). `txReservePct` (25%) always held back for transaction + UI traffic.
  - Per-symbol refresh = `symbols × 60s / usable` clamped `[min, max]` (5s..300s), round-robin cursor, exponential backoff ≤120s on failures.
  - **Request weighting**: Lighter-family book reads cost **2 slots** (orderBookDetails + orderBookOrders).
  - Jobs run concurrently via `JoinSet` — slow venues never serialize a cycle; adapter `AccountRequestCoordinator` min-intervals still serialize per venue.
  - Watchlist = symbols on ≥2 admitted venues per hourly `fetch_markets()`, minus durable exemptions. Empty on failure — never fabricated.
  - Samples persist to `spread_samples` under the **canonical base** (`BTC`); evaluation merges `BASE`/`BASE-USDC`/`BASE-USD` keys so legacy ticker-flow history feeds the same baseline. `canonical_base` handles USDT quotes and refuses to collapse real tickers ending in USD (`TUSD`, `SUSD`, `BUSD` stay intact).
  - Lifecycle `collecting → watching → signaled → qualified` (stale any time books lapse); qualification window = `max(halfLifeMultiplier × OU half-life, minWindowSecs)` (default 2×hl, 30-min floor) or manual 24h; per-chunk `variation ≥ frictionCoverMult × friction` noise gate.
  - **Directional honesty**: signals require `dev ≥ +friction` (positive only — negative dev is the reverse pair's trade); `net_capture_bps`/`expected_profit_bps` are signed, never `abs()`'d into phantom gains; ladder requires a real `μ` (empty otherwise); two-way exit est = exit fees + top-of-book crossing both books (documented floor).
  - Lifecycle persists on transitions (±60s last-seen heartbeat) → `firstSeenMs`/duration survive restarts and long refresh gaps; rehydrated at engine boot.
  - `set_config` clamps every field to sane ranges (modes whitelisted, windows/tiers bounded, empty tier list → defaults).
  - `qualified_opportunities()` = the documented seam for the future autonomous bot — same qualified surface the UI executes from.

### 8.5 Spread Range Monitor — IPC + UI
- Commands: `get_monitor_snapshot`, `get/set_monitor_config`, `set_ticker_exempted`, `get_exempted_tickers`, `reveal_all_tickers`, `execute_monitor_opportunity` (re-validates qualified+executable on the latest snapshot, converts USD→qty at live ask, dispatches the existing guarded paper path — blocking signals still block).
- `OpportunityRadar.tsx` (new nav tab): full table — ticker, route, edge/base/dev bps, net bps, P(win), E[bp], E[$], optimal/breakeven size, funding carry, signal age, T→μ, status chip, per-row Execute with `block_reason` tooltips. Filters: ticker search, buy-venue, sell-venue, min E[bp], min P%. Expandable capacity-ladder bars per row. Lane chips show `used/budget /min`, tier, refresh interval, backoff.
- Config drawer exposes every user-requested knob: enable, qualification mode, manual window, half-life ×, min window, friction-cover ×, capacity tiers, min win prob, execution mode (one_way default / two_way), tx reserve, watchlist refresh, history window.
- `TickerConfigModal`: per-ticker exemption toggle. `EngineSettings`: global reveal-all. `App.tsx` applies exemptions to explorer/strategy ticker derivation.

### 8.6 Verification (all green at handoff)
```bash
cargo test -p arbitrager-desktop    # 110 pass / 0 fail (7 monitor tests incl. end-to-end evaluate)
cargo test -p arbitrager-math       # 32 pass
cargo test -p arbitrager-exchanges  # 31 pass
cd frontend && npx tsc --noEmit     # clean
npm test                            # 37 pass (node:test — NOT vitest)
git diff --check                    # clean
```

### 8.7 Working tree state & open items
- **Everything uncommitted** — the session's full diff spans `crates/`, `frontend/src-tauri/`, and `frontend/src/`. `docs/HANDOFF_*.md` + `SPEC_SPREAD_RANGE_MONITOR.md` updated.
- **Not done / next**: (1) the autonomous opportunities bot consuming `qualified_opportunities()` — spec'd seam only; (2) depth-dependent *exit* walk for two-way mode (current exit estimate is a top-of-book floor); (3) live `sendTx` remains unimplemented everywhere — monitor execution is paper-only and the execute gate hard-blocks live mode; (4) monitor's lane budget doesn't count requests the *UI itself* issues to the same Lighter IP — `txReservePct` is the mitigation, revisit if WAF pressure reappears; (5) no PR/merge state — local working tree only.
- Rebuild the desktop binary (`cargo build --release --bin arbitrager-desktop` or the debug `cargo build -p arbitrager-desktop`) before relaunching — last-pass fixes landed after the running binary was built.

---

## 9. Session Handoff — 2026-09-14 (Consolidated Positions, System-Wide Style Canon, Universal Strategy Configurators)

### 9.1 Theme
Three workstreams: (a) the **consolidated Positions panel** and Portfolio Monitor navigation toggle; (b) a **system-wide style canon** — one spacing/radius/selected-state scale across every page and modal; (c) the **universal per-strategy configurator** refactor — `StrategyParamsDto` keyed by strategy id replaces per-ticker authoring, with global concerns (ticker visibility) moved to System Settings. Fail-closed/paper-only posture unchanged throughout.

### 9.2 Consolidated Positions Panel
- `PositionsPanel.tsx` aggregates open positions by pair/event with signed per-venue nets, mark-to-market PnL, transaction aggregation, sort (name/PnL/size), and selected-position highlighting.
- **Strategy scoping**: Single Spread positions carry `strategy_id = "futures"`, Spread Radar `"radar"` — signals/execution/Stop-flattening are scoped independently.
- Scope toggle: **All** / **{current strategy}**; the scope button uses a short label map (`Prediction Market` → `Prediction`) while tooltips/empty-states keep full names.
- `get_open_positions` IPC + `OpenPositionDto` — backend aggregation in `state_store.rs` with honest nulls.

### 9.3 Style Canon (system-wide sweep)
- Canonical scale now enforced: page `p-4 space-y-4`; gadget cards `rounded-lg border shadow-2xs p-3.5`; card inner stacks `space-y-3`; nested panels `p-3 space-y-2`; gadget header dividers `pb-2.5`; one table-header recipe (`bg-surface-raised text-xs text-slate-500 tracking-wider`); `ring-1` focus; `shadow-2xs` chips.
- **Selected-state convention unified**: `bg-accent text-white shadow-2xs` active / `text-slate-500 hover:text-slate-900` inactive across all segmented controls and toggles (was three competing treatments: raised white chip, accent-dim tint, plain). Semantic FILLED/FAILED colors kept.
- Modals deliberately keep `rounded-xl p-6` + heavier shadows; micro-gaps and badge paddings kept.
- **Obscura caveat learned**: the privacy browser spoofs `getComputedStyle` for layout metrics (padding/gap/radius report 0px while rendering correctly) — verify layout claims with `offsetHeight` or the served stylesheet, not computed styles.
- Signal Trend extracted into a standalone gadget above the quotes grid; TickerTelemetry's doubled vertical padding (`p-3.5` card + `py-4` grid) fixed.
- Typography normalized to local Roboto / Roboto Mono variable fonts.

### 9.4 Portfolio Monitor — nav toggle + style normalization
- `App.tsx` keeps `prevTabRef`: entering Portfolio Monitor records the prior strategy tab; clicking the button again returns to it (`handleTogglePortfolio`). Verified live (Funding Rate → Monitor → back to Funding Rate).
- Full style pass to main-page conventions: compact `p-3.5` KPI cards, `rounded-md` nested panels, `text-xs` flat typography, standard thead recipe, `py-0.5 rounded-sm` badges.

### 9.5 Opportunity Radar — columns, filters, header parity
- Table columns reordered/renamed: Symbol | Route | **Exp Profit** (3rd) | **Net Profit** (4th) | Baseline BPS | Deviation BPS | Win % | Exp Profit $ | Opt Size $ | Max Size $ | Signal Age | **Time to Converge** | Status | Action. Edge BPS and Funding BPS removed from the outer table; `colSpan`s updated.
- `{n} tickers · {n} pairs` counter removed from the filter strip.
- **Venue filter → dropdown multi-selector**: trigger shows `Venue ⌄ ALL` or joined selection (`LGT+RH`); ALL row clears; rows toggle with `✓`; closes on outside click; matches when either leg hits any selected venue.
- **`+Exp Profit` toggle removed** — the filter is built-in. New persisted config field `min_expected_profit_bps` on `MonitorConfigDto` (serde default `0.0`, clamped ±10,000, editable in the radar configurator); rows filter `expectedProfitBps > floor` unconditionally; `0` = positive-only, negative reveals losing rows.
- Header now carries the standard control set: labeled **Configure Strategy** + shared `BotLifecycleControls` (radar-scoped `useBotLifecycle({strategyId: "radar"})`); gear sits left of Stop.
- **Venue rate-budget chips moved off the page** → System Settings → Exchanges → per-venue `VenueBalancePanel` gains a `lane` prop rendering `used/budget /min`, tier, refresh interval, backoff state — fed by a 5s `get_monitor_snapshot` poll inside EngineSettings, with a truthful "no monitor lane" fallback.

### 9.6 EngineSettings
- **Max Slippage Tolerance default 15 → 1 BPS** (initial state, load fallback, empty-input fallback; backend floor is 0.1). Previously saved values still load — default change only.
- Trading Mode card restructured 1×2 → **2×1 stacked** (mode row, then Paper Starting Balance below a `border-t`).
- **Hidden Tickers upgraded to a full manager**: hidden-ticker chips with per-ticker restore (×), a hide-new input backed by the `PERP_TICKERS` datalist, Reveal All — replacing the count + Reveal-All-only row. This is now the *only* surface for ticker visibility (was previously also inside the per-ticker modal).

### 9.7 Universal Strategy Configurators — the headline change
**Problem**: the shared `{ticker} Strategy Configurator` modal mixed three axes — strategy switching from any page, per-ticker persistence (configuring BTC left ETH untouched), and a global hide-ticker control inside a strategy surface. Funding/Spatial params lived only in React state — unpersisted.

**New canonical store** — `StrategyParamsDto` (`state.rs`), flat record keyed by `strategy_id` ∈ `{futures, funding, spot}`:
- futures: `target_profit_bps`, `window_percentile`, `sliding_window`, `max_accumulated_units`, `execution_mode`, **`unit_size_usd`**, `user_target_leverage`
- funding: `funding_trade_size_usd`, `funding_min_apr_pct`, `funding_holding_days`
- spot: `spot_trade_size_usd`, `spot_max_slippage_bps`, `spot_min_net_spread_bps`
- `AppEngineState.strategy_params: HashMap<String, StrategyParamsDto>` — **in-memory**, same durability as `strategy_configs` (session store; persistence is a deliberate follow-up).
- Commands `get/save/get_all_strategy_params` (+ `_impl`s) with full-field validation and a `KNOWN_STRATEGY_IDS` whitelist; `get_all` always returns all three with defaults. Registered in `main.rs` + `NATIVE_COMMANDS`.
- The futures execute path's leverage check now reads universal params first, per-symbol record as legacy fallback, then default 5.

**Universal sizing semantic**: `unit_size_usd` is the authored field; `deriveUnitSize(symbol, venues, basePrice, usd)` in `contractSpecs.ts` converts to base units — ceil to max step size, clamped to the venue-compatible min lot. LiveMatrix `orderQty`, matrix-quant, and modal previews all consume the derived value. Per-symbol `TickerStrategyConfigDto` shrinks to runtime state (`current_accumulated_units` + cached unit size).

**`TickerConfigModal`**: props `{isOpen, onClose, tickerSymbol (context only), initialStrategy (fixed), isEnabled?, onToggleEnabled?, onSaveConfig(params)}`; title is `{StrategyName} Configurator`; left panel = `STRATEGY_SECTIONS` nav within the active strategy (Hurdle & Triggers / Sizing & Grid / Leverage & Margin for futures; Carry Settings for funding; Spatial Parameters for spot); Unit Size input is USD notional with `≈ N BASE on SYMBOL` preview; accumulated-units reset explicitly labeled per-ticker; `On {base}` toggle + `{base} Preview` card render **only for futures** (they're meaningless elsewhere). Save = `save_strategy_params` (primary) + best-effort per-symbol grid-state write. Fail-closed: a rejected/unreachable save leaves the modal open with no fake "Saved!".

**Pages**: Configure Strategy buttons no longer gated on ticker selection (universal params need none); `handleOpenTickerConfig` defaults context symbol to `BTC-USDC`. Funding/Spatial params are derived from `strategyParams` map in App (replacing React-local `fundingTradeSize`/`fundingParams`/`spotParams`). TickerExplorer's max-units check reads universal `maxAccumulatedUnits`. **Triangle's two modal entry points removed** — `"triangle"` isn't a known strategy id and its inline Cycle Guardrails card is the real configurator.

### 9.8 Published Spec
- **GitHub Issue #6** — *Spec: Universal Per-Strategy Configurators — strategy-scoped params shared across all tickers* (`ready-for-agent`). Captures the full design contract, user stories, test seam (the strategy-params command boundary), and out-of-scope items (durable persistence, per-ticker overrides, other-strategy configurators).

### 9.9 Verification (all green at handoff)
```bash
cd frontend/src-tauri && cargo check      # clean
cargo test -p arbitrager-desktop          # strategy tests pass (incl. config CRUD)
cd frontend && npx tsc --noEmit           # clean
npm test                                  # 48 pass (node:test)
```
Live-verified via Obscura: all three configurators render strategy-scoped (Single Spread: 3 sections + BTC Preview + On-BTC toggle; Funding: Carry Settings only; Spatial: Spatial Parameters only); save fails closed in the browser session (no Tauri runtime → modal stays open, no fake success); Hidden Tickers manager renders in System Settings.

### 9.10 Working tree state & open items
- **Everything uncommitted** — diff spans `frontend/src-tauri` (state.rs, commands.rs, main.rs, monitor.rs, state_store.rs) and `frontend/src` (~20 components + api.ts, types, contractSpecs).
- **Not done / next**: (1) **strategy params are session-only** — `strategy_params` and `strategy_configs` are both in-memory; durable persistence needs a state-store schema migration (spec'd as follow-up in issue #6); (2) `unit_size_usd`→base derivation uses the static reference `basePrice` — consider live-price normalization later; (3) per-ticker parameter overrides are rejected by design — would be a new seam if ever wanted; (4) Prediction/Statistical have no params record; Spread Radar keeps its own monitor configurator; (5) the modal's `isEnabled`/`onToggleEnabled` for futures toggles the *context* ticker's enable flag — a per-ticker gate, kept deliberately; (6) no PR/merge state — local working tree only.
- Rebuild the desktop binary before relaunching to pick up the new commands (`cargo build -p arbitrager-desktop`).

### 9.11 Bugfix — "connected accounts but no tickers" (venue status/admission drift)

**Symptom**: user had enabled accounts on lighter, robinhood, vanta, binance, decibel — yet `No Available Tickers`.

**Root cause**: `get_system_status_impl` emitted each venue's `supported` flag as a **hardcoded per-venue bool** while `get_venues()` is the real admission source. `get_venues` admits `robinhood` (Lighter-protocol adapter at `api.rh.lighter.xyz`) when enabled, but the status table hardcoded `supported=false` → robinhood reported `UNSUPPORTED` → `getAdmittedVenueCodes` (READ_ONLY-only) dropped it → `activeVenueCodes = {LGT}` → every ticker needs ≥2 admitted venues → empty universe. Enabled accounts on vanta/binance/decibel legitimately remain UNSUPPORTED — enabled ≠ stream-capable.

**Fix** (`commands.rs`): `venue_status(vid, enabled)` now looks up `admitted_map` (built from `get_venues()`) — `READ_ONLY` iff the venue is actually admitted. Structurally impossible to drift again. In live mode `get_venues` returns empty → all enabled venues report UNSUPPORTED → fail-closed (no read-only lanes in live mode).

**UI truth pass**:
- `TickerExplorer` empty state now names the real cause via `tickerUnavailabilityReason(status, activeVenueCodes)` (`tickers.ts`): engine unreachable / live mode / enabled-but-no-adapter / only-one-venue-streaming / all-hidden.
- `EngineSettings` exchange list: dot color + right label now reflect backend status — emerald `READ_ONLY` (streaming), amber `NO ADAPTER` (UNSUPPORTED), grey `OFF` — instead of green-for-anything-enabled. Footer adds a `Streaming: N` count (READ_ONLY venues) next to Active/Configured.
- `getEnabledConnectedVenueCodes` doc comment corrected (HL/LGT/RH adapters; prefer `getAdmittedVenueCodes`).

**Tests**: `test_system_status_reflects_account_level_enablement` extended — robinhood account → READ_ONLY, binance account → UNSUPPORTED. New frontend tests cover `getAdmittedVenueCodes` (READ_ONLY-only admission) and every branch of `tickerUnavailabilityReason`. `cargo test` 113 pass; `npm test` 51 pass; tsc clean.

**Note**: `VantaAdapter`/`ArcusAdapter`/`PopDexAdapter` exist but are deliberately NOT admitted by `get_venues` (vanta's public Orderly book works unauthenticated; arcus bails without an API key; popdex host is a parked domain). Admitting vanta would widen the ticker universe — product decision, left unadmitted.

### 9.12 Corner-radius canon sweep (controls)

Radius scale enforced: cards/tiles `rounded-lg` (8px) · buttons/inputs/selects `rounded-md` (6px) · segmented chips + badges `rounded-sm` (2px) · modals `rounded-xl` (12px) · toggles `rounded-full`. Fixed ~35 deviant controls:

- bare `rounded` (4px) → `rounded-md`: `BotLifecycleControls` START/PAUSE/STOP group, OpportunityRadar configurator inputs (12), modal close-X, in-row action button, SpotArb select chips, TriangularArb asset chips
- `rounded-lg` on controls → `rounded-md`: EngineSettings (5 inputs + 4 buttons incl. footer Save/Cancel + vault-set buttons), ExchangeAccounts inputs, OpportunityRadar modal footer, SpotArb Deploy, TriangularArb inputs + Execute, StrategyPerformance buttons, VaultUnlockModal inputs + buttons
- status chip `rounded` → `rounded-sm` (OpportunityRadar), exec-mode toggle chips `rounded` → `rounded-sm` (TickerConfigModal)

Deliberately kept `rounded-lg`: card panels, list tiles (exchange rows, position cards, config sidebar nav), dashed add-account dropzone. Live audit: every button computes 6px, chips 2px, nav tabs 0px. `getComputedStyle().borderRadius` shorthand serializes "0px" in the Obscura harness — use `borderTopLeftRadius` per-corner when verifying.

**Concentric-corner follow-up**: segmented chips nested in `p-0.5`+`rounded-md` containers were `rounded-sm` (2px) — visibly sharper than the 6px outer corner. Rule: `inner radius ≈ outer radius − padding`. Fixed chips → `rounded` (4px) in PositionsPanel scope toggle, SignalTrend baseline/window chips, TickerLogMonitor level filter, TickerTransactionHistory status filter. `p-1`+`rounded-md` containers correctly pair with `rounded-sm` (6−4=2): TickerConfigModal timeframe grid + exec mode, TriangularArb asset chips.

**Padding-balance sweep**: user flagged the BotLifecycleControls group container (`px-2 py-1` — left gap 8px vs top/bottom 4px) → `px-1.5 py-1`. Then a full audit normalized every framed interactive element to horizontal padding ≤ ~2× vertical. Convention: compact chips/buttons `px-2 py-1` (8/4); standard buttons `px-3 py-1.5` (12/6); large `px-4 py-1.5/2`. Fixes: `py-0.5` chips → `py-1` (SignalTrendGadget mode/window, TickerLogMonitor level, StatArbLab input-mode, OpportunityRadar Exec, EngineSettings un-hide chips, ExchangeAccounts label input); `px-3 py-1` → `px-3 py-1.5` (EngineSettings/ExchangeAccounts secondary buttons, LiveMatrix exec cell, SpotArb Select, TickerConfigModal exec-mode pair, TriangularArb Fill Cycle); Header PAUSE ALL/STOP ALL `px-2.5 py-1` → `px-2 py-1`. Intentionally untouched: full-height strip tabs (py-0, fills parent), non-interactive badges/spans, `px-1 py-0.5` compact table inputs, `w-full py-2/3` CTAs. Live audit after: zero buttons above 2:1.

**Label-case canon**: all word labels are sentence case (first char uppercase only). Removed `uppercase` utility from BotLifecycleControls buttons + phase badge, ExchangeAccounts "Default" badge, TickerExplorer "Issue" badge, EngineSettings "Paper" badge. Literal-caps → sentence case: SignalTrend CONSENSUS/LEADER + Sell/Buy legend, Header Pause All/Continue All/Stop All, OpportunityRadar venue "All", LiveMatrix Perp + Best badges, StrategyPerformance "Active", TickerExplorer Perp/Off, PredictionHub Buy Yes/Buy No. TickerLogMonitor filter chips + level badges keep caps **values** (filter keys `ALL`/`TRADES`/`EXEC`/etc.) but render transformed via `lvl.charAt(0) + lvl.slice(1).toLowerCase()`. EngineSettings mode badge capitalizes `tradingMode` ("Paper"/"Real"). Kept uppercase: unit codes (USD, BPS), venue/strategy codes (HL/LGT/SS), brand (ARBITRAGER), ticker symbols, search-input `uppercase` transform.

**Tooltip rework**: `InfoTooltip` was `absolute`-inside-`relative` — clipped by any ancestor `overflow-hidden`/`overflow-y-auto` (e.g. Dislocation Matrix "?" at a container edge). Now renders via `createPortal` to `document.body` at `position: fixed` with viewport clamping (8px margin), auto top/bottom flip when space is short, arrow tracks trigger center, closes on scroll. Styling: dark `bg-slate-900/95` → light-blue `bg-accent-dim/95` + `border-accent-line` + `text-slate-700` (z-100, above modals). SignalTrendGadget chart hover readout restyled to match. Kept dark intentionally: `bg-slate-900` code block (serve command) and modal scrims.

**Dropdown-menu canon**: all 4 floating menus now share inset-item styling — container `p-1` (was `py-1`, so item highlights get a white gutter instead of touching the border), items `rounded-sm` (concentric: 6−4=2), header labels `-mx-1 px-3.5` and separators `-mx-1` so dividers still span edge-to-edge. Widths bumped: OpportunityRadar venue w-44→w-52, SpotArb asset w-40→w-48, PositionsPanel sort w-44→w-52, TickerExplorer sort w-48→w-56. SpotArb asset filter converted from single-select chip row to this dropdown multi-selector (client-side `assetFilters` on symbol base — backend `scan_spot_opportunities` is a stub that ignores the filter anyway).

### 9.13 Venue connectivity truth pass — Vanta admission, explicit Ping, live market universe

**Vanta docs mismatch**: Vanta's own docs state its perpetual API is Orderly's; the adapter was pointing at the stale `api-evm.orderly.org` host and undocumented GET routes. `VantaAdapter` rewritten for `POST {base}/v1/public/query` (unauthenticated, weight-1): `marketSummary` (markets/ticker/funding), `orderbook` (max_level 100, empty sides returned as an empty book — validation omits it), `rateLimitStatus` probe. All numeric fields parse decimal strings without f64 round-trips; missing bid/ask/funding/next_funding_time fail closed; `PERP_<BASE>_USDC` exact-match only (broker-suffixed symbols excluded). Defaults updated to `https://api.orderly.org` (state.rs, adapter, EngineSettings fallbacks); user-configured URLs preserved. Vanta is now admitted by `get_venues` in paper mode (`Vanta (Orderly, Read-Only)`) with a simulator-backed paper exchange ($100k explicitly paper equity) — reversing the 9.11 deferral.

**Explicit Ping**: new `refresh_venue_latencies` command clears `latency_probe_at` and re-runs `get_system_status` so all enabled venues (adapter `ping()` or public probe) re-measure on demand. EngineSettings header gets a `Ping` button (RefreshCw, disabled while pinging, refreshes `liveVenues` from `active_venues`). `PublicProbe` gained an optional POST body so Vanta probes `rateLimitStatus` (2xx only); other venues stay GET.

**List semantics**: exchange rows now prioritize measured RTT for any enabled venue — `Off` / `{ms} ms` / `No response`; no more `NO ADAPTER`/`N/A`/uppercase `OFF`. Titles distinguish admitted (`Market data active · measured API round-trip`) vs unsupported (`API reachable …, but market quotes are not integrated` / `… probe returned no response`) vs off. Equity line reads `Paper $X` / `No paper` with honest titles; both rows `text-[11px]`. Dot semantics kept: green = admitted market-data adapter, amber = enabled but no quotes integration, gray = off.

**Dynamic market universe**: new `get_market_universe` command fans `fetch_markets` out to all admitted adapters concurrently (per-venue failure omits that venue, fail closed), returns active-perp `BASE-QUOTE` symbols sorted/deduped per venue. Frontend `applyMarketUniverse(catalog, universe)` intersects the 52-row catalog with actual reported markets — a ticker needs ≥2 real reporting venues and its `venues` array is replaced by the truth. App polls it immediately + every 30s; `filterAvailableTickers`/static venue claims no longer decide availability. `tickerUnavailabilityReason` no longer hardcodes three venue names.

**Verification**: `pixi run check`, `pixi run test`, `npx tsc --noEmit`, `npm test`, `git diff --check` all green at handoff. Fixture parser tests cover standard-market accept, broker-suffix rejection, orderbook parsing, and fail-closed ticker/funding — no network.

### 9.14 Per-ticker adaptive sliding window + durable baselines (2026-09-15)

**Config shape**: `StrategyParamsDto` gained `slidingWindowMode` (`manual`|`auto`) + `slidingWindowSecs` (camelCase, serde defaults `manual`/`86400` so old records load). `save_strategy_params` rejects unknown modes and secs outside 300..2592000 (5m–30d) instead of clamping, and canonicalizes the legacy `sliding_window` string from seconds (largest exact unit: d→h→m→s). Defaults/factory reset are now manual `86400`/`24h` everywhere (state.rs, `defaults_for`, `TickerStrategyConfigDto::default`, frontend normalize).

**`get_matrix_baselines` command**: validates symbol/mode/manual window/percentile (50–99), pulls the trailing 30 days of persisted `spread_samples` under the monitor's three symbol aliases (base, `BASE-USDC`, `BASE-USD`) with `(ts_ms, venue_id)` dedup, builds aligned pair series via `build_pair_series` (30s gap), and per ordered venue pair reports mean + nearest-rank percentile gross bps over the effective window — only when ≥20 observations and ≥90% window coverage, else `None` stats with truthful counts. Auto mode qualifies pairs at ≥40 aligned obs + ≥1800s span + `OuModelParams::fit` half-life finite/positive + R² ≥ 0.6, recommends `2×half-life` clamped 30m–7d, and takes the deterministic upper median across pairs; no qualifying pair → `auto_window_secs=None`, effective = manual, `using_manual_fallback=true`. Nothing is fabricated; the WFO minimal-data path is not used.

**Matrix consumption**: `buildCrossVenueMatrix` takes an optional trailing `durableBaselines` arg; when supplied it never records/consults `RollingSpreadTracker`, subtracts fees+closing friction from durable gross stats, and sets `baselineReady` — insufficient pairs render `Δ —`, keep truthful `sampleCount`, and cannot execute divergence entries (convergence unwinds of held units stay allowed). `LiveMatrix` polls the snapshot per ticker every 15s, derives the effective window from it, and passes an empty list when absent (fail closed). `baseline_mean_spread_bps` is omitted from execution candidates when not ready.

**UI**: `TickerConfigModal` Signal Trigger swaps the 10s–15m preset buttons for a number+unit (Minutes/Hours/Days) input plus an `Auto from history` `SwitchToggle`; a debounced (300ms) snapshot fetch powers the status line (`Manual trailing baseline · …` / `Auto-selected … from N qualified venue pairs · median half-life …` / `Collecting sufficient history · using manual fallback …`). `SignalTrendGadget` formats arbitrary effective seconds via shared `formatDuration`; `get_signal_series` clamp raised 7d→30d to match retention. Shared helpers (`durationParts`/`durationSeconds`/`formatDuration`/`parseDuration`) live in `matrixQuant.ts`; `SlidingWindowOption` is a plain string alias.

**Verification**: Rust tests cover manual sufficiency/insufficiency, OU-median auto selection, fallback, validation, and alias dedup; frontend tests cover durable-override, friction conversion, fail-closed divergence, unwind-without-baseline, duration helpers, and normalize defaults. Full gates run at handoff.

### 9.15 Backend-owned Single Spread route economics (2026-09-15)

**UI removals**: `TickerConfigModal` lost the left-sidebar `{base} Preview` summary (Min Lot / Margin / Venues — sidebar is now settings tabs only) and the blue static Nominal Hurdle gadget from Signal Trigger (fixed 7.0 fee + 0.6 friction assumptions deleted along with the dead `Info` import). Signal Trigger is four gadgets: Target Profit, Sliding Window, Window Percentile, Execution Mode.

**Route economics seam**: one pure `compute_route_economics(book_buy, book_sell, qty, buy_fee, sell_fee, target)` in `commands.rs` now serves both `scan_spatial_opportunities` and execution preflight. For configured qty it requires all four legs to fill fully on current books — entry buy (buy-venue asks), entry sell (sell-venue bids), expected exit sell (buy-venue bids), expected exit buy (sell-venue asks) — else the route is not costable and is omitted (scan) or rejected (execution). Friction = round-trip commission `2×(buy_fee+sell_fee)` + entry slippage + exit slippage + closing spread (sum of both venues' `(ask−bid)/mid`); `required_gross = friction + target`, `expected_net = top_gross − friction`, `est_profit = expected_net × mid notional`. `slippage_ok` = all four legs pass `verify_pre_execution_slippage` (entry legs on IOC-bounded books); `depth_ok` = bounded entry depth ≥ qty × min_depth_ratio. `is_executable = expected_net ≥ target && slippage_ok && depth_ok`. `effective_venue_fee_bps(state, venue)` centralizes the fee schedule + Lighter/Robinhood +0.5 BPS Plus-tier surcharge (default account); `monitor.rs` now calls it, so scan/monitor/execution cannot drift.

**Execution preflight**: `execute_spatial_arbitrage_inner` re-fetches fresh books, recomputes route economics, and rejects unless `slippage_ok`/`depth_ok` hold and `expected_net_bps ≥ strategy_params["futures"].target_profit_bps`. The caller's `nominal_hurdle_bps` is kept for IPC/log shape but no longer admits trades; `trigger_spread_bps`/`expected_net_usd` in logs use recomputed values.

**Frontend**: `SpatialOpportunityDto`/`MatrixCellDto` carry the full cost breakdown + `costReady`. `buildCrossVenueMatrix` takes trailing `routeEconomics` — matched (case-insensitive venue IDs) cells use backend gross/net/friction/required-gross, convert durable gross baselines to net by subtracting `expected_friction_bps`, and require `costReady + baselineReady + slippage_ok + depth_ok + gross ≥ required + net ≥ percentile` for divergence (convergence unwind unchanged). Uncosted cells render `—` with neutral styling and exact-breakdown/`Route economics unavailable` titles. `LiveMatrix` passes `opportunities` always; the Hurdle chip shows the selected route's `requiredGrossBps` (else min across costed cells, else `—`); table rows send the backend `required_gross_bps`. `App.refreshAllData` scans at the configured futures target + `deriveUnitSize(unitSizeUsd)` quantity and now depends on `strategyParams`.

**Target default**: Target Profit default/fallback/reset is **1 BPS** everywhere (state.rs `defaults_for` + `TickerStrategyConfigDto::default`, configurator initial/load/reset, api normalize, LiveMatrix, App refresh, TickerLogMonitor `?? 1.0`); persisted user values are untouched.

**Tests**: Rust — exact friction math, multi-level VWAP slippage, incomplete-depth fail-closed, plus-tier fee sharing, scan gating (target/slippage/depth), target-vs-hurdle admission. Frontend — backend override, missing-route fail-closed + neutral visuals, flag gating, unwind-without-costs. Full gates run at handoff.

### 9.16 Configurator/UI polish + spec issue #7 (2026-09-15)

**TickerConfigModal structure**: section tabs in the left panel now carry their `?` InfoTooltip inline beside the tab name (no more explanation line under names; section-level header icons removed). The "Hurdle & Triggers" tab/section renamed **Signal Trigger**. The Quick Configuration Presets bar (Conservative/Balanced/Aggressive + Sparkles icon) deleted outright. Section-level outer cards removed — every setting is its own floating gadget card (Signal Trigger: Target Profit / Sliding Window / Window Percentile / Execution Mode after the 9.15 removals; Sizing & Grid, Leverage & Margin, Carry, Spatial similarly split). Selected nav row uses `bg-accent text-white` (was a one-off `bg-white text-blue-600` anomaly).

**TickerExplorer selection**: selected ticker card uses the light-blue accent-dim treatment (`bg-accent-dim/70 border-accent/60` + `border-l-accent` 3px rail) and it now wins over the disabled gray when the ticker bot is Off — previously Off styling overrode selection.

**OpportunityRadar filter bar parity**: bar `gap-2 py-2` → `gap-3 py-3` to match FundingRadar/SpotArb (`gap-3`/`py-3`, `p-4`); field inputs widened (Symbol w-20→w-28 fits `BTC-USDC`, Min Exp/Min Win w-12→w-16).

**SignalTrendGadget**: dynamic first chip now renders just the duration (`1m`), `BOT` prefix removed per user request — meaning preserved in the tooltip.

**Spec published**: **GitHub Issue #7** — *Backend-owned route economics for Single Spread: dynamic per-route friction, universal configurator, 1 BPS target default* (`ready-for-agent`). Documents the landed 9.15 behavior as a verification contract: both test seams named explicitly (pure `compute_route_economics`/`route_execution_gates` primary + IPC regression on `scan_spatial_opportunities`/`execute_spatial_arbitrage_inner`), 22 user stories, out-of-scope list (live trading, live fee fetching, maker modeling, per-ticker params).

**Working tree state & open items**: (1) everything above is uncommitted local work — no PR/merge state; (2) Vite Fast Refresh warnings persist for non-component exports (`EXCHANGES_ALPHABETICAL`, `ACCOUNT_CREDENTIAL_FIELDS`) — cosmetic, non-blocking; (3) `get_market_quotes` per-quote `fee_bps` still uses the base schedule (display-only, not execution economics); (4) dev env relaunched this session via `pixi run desktop` after a stale Vite held :1420 — kill stray `vite` processes before relaunching.

---

## 10. Session Handoff — 2026-09-16 (WS Feeds, Durable Retention, Radar Canon, Venue Short Names)

### 10.1 Theme
Two user-driven workstreams: (a) price ingestion moves WebSocket-first with
REST fallback plus always-on history under user-controlled retention;
(b) visual consistency sweep (radar configurator canon, short venue names,
legend/ticker cleanups from prior turns).

### 10.2 WebSocket feeds + retention (spec `SPEC_WS_FEED_AND_RETENTION.md`)
- Rate-limit truth verified against live venue docs: WS does not bypass
  limits (Lighter rate-limits WS alongside abused REST) — it relieves them
  (Lighter books cost 2 REST slots today; HL one info conn/IP push-on-change;
  Orderly ~1 s snapshot pushes at `wss://ws-evm.orderly.org/ws/stream/{account_id}`).
- New `crates/arbitrager-exchanges/src/ws.rs` (pure parse helpers, fixture
  tests) + `frontend/src-tauri/src/feed.rs` per-venue supervisors writing to
  `MarketDataHub`; REST `fetch_orderbooks_impl` untouched as fallback;
  disconnects go `stale` via hub TTL. Vanta WS skipped without account_id.
- Retention: `spread_samples_max_days`/`spread_samples_max_gb` (`None` =
  unlimited, `0`→`None`) on `EngineSettingsDto` + TS mirror, V7 `kv` table,
  boot-load, hourly enforce (time cutoff + `page_count×page_size` batch
  evict); Execution-tab Historical Price Data card. Replaces hardcoded
  30-day/4M-row prune.
- Fail-closed throughout; live execution still disabled.

### 10.3 Radar configurator canon + venue short names
- `OpportunityRadar` drawer + ribbon recomposed to `TickerConfigModal` /
  `FundingRadar` canon (modal chrome, section sidebar, gadget cards,
  SwitchToggle, InfoTooltips, Reset Defaults, ribbon chips + action group).
- `get_venues` display names shortened (`Hyperliquid`, `Lighter`,
  `Robinhood Lighter`, `Vanta`); long forms kept as `canonical_venue_id`
  aliases. Follow-ups from prior turns also landed: hover-synced legend
  (no side box, no Buy/Sell tags), ticker toggle + Off badge + sliders
  button removed, right-click hide, tab typography step-down.

### 10.4 Verification (all green at handoff)
`cargo test --workspace` (263 pass incl. 142 desktop + 44 exchanges),
`cargo clippy --workspace -- -D warnings` clean, `tsc --noEmit` clean,
`npm test` 71 pass. Working tree uncommitted; rebuild desktop binary before
relaunching.

---

## 11. Session Handoff — 2026-09-16 (Aster, Ondo Perps, GRVT, edgeX Venue Support)

New adapters in `crates/arbitrager-exchanges/src/`: `aster.rs` (fapi/v3,
quote parsed from exchangeInfo — USDT/USD1/U, never hardcoded),
`ondoperps.rs` (`BASE→BASE-USD.P`, `success:true` envelope, 1100 ms
coordinator), `grvt.rs` (depth allowlist {10,50,100} per live 3031 probe),
`edgex.rs` (docs-derived paths, live host `edgex-prod-v2.edgex.exchange`,
metadata-resolved contract ids). All REST-only (WS stubbed),
account/order/leverage fail-closed. Admitted to `get_venues`: Aster + Ondo
Perps (both serve live public books, probed 2026-09-15). GRVT/edgeX wired
(probes, fees, accounts UI) but unadmitted pending payload-shape
verification. Ondo Finance proper (USDY/OUSG mint-redeem) is NOT an exchange
— documented only (§9 of integration guide). Fees: aster/ondo 3.5,
grvt 2.0, edgex 4.5 BPS. Verification: workspace 291 pass (142 desktop +
72 exchanges), clippy clean, tsc clean, npm 71 pass. Uncommitted; rebuild
desktop binary before relaunching.

---

## 12. Session Handoff — 2026-09-16 (Equity-Bounded Step Sizing)

`StrategyParamsDto` drops authored `unit_size_usd`/`max_accumulated_units`
for `step_size_mult` (× venue min lot, default 10), `max_step_usd` (cap,
default $2,500, lower wins, quantized up to venue step, floored at
min-compatible), and `max_ticker_equity_pct` (default 2% of total paper
equity). Max steps auto-derive: `max(1, floor(equity×pct/stepUsd))`,
read-only in Sizing & Grid alongside the effective-step preview.
Canonical `deriveStepSizing` feeds both scan and execution qty; backend
`execute_spatial_arbitrage_inner` now enforces the grid cap fail-fast
(previously accepted-but-ignored `max_units`). Accumulated-units lifecycle
unchanged. Verification: workspace 297 pass (146 desktop + 72 exchanges),
clippy clean, tsc clean, npm 77 pass. Uncommitted; rebuild desktop binary
before relaunching.

---

## 13. Session Handoff — 2026-09-16 (Auto-Tune Phase 1, Recommend-Only)

New pure module `crates/arbitrager-math/src/autotune.rs`: 70/30
chronological walk-forward over alias-aware spread history; 7 candidates
fit-only (window 2×OU half-life; target friction+margin; percentile
inverted-CDF snapped down; step/maxStep from capacity ladder + 0.5%
participation cap; equity inverse-vol [2,20]; leverage decrease-only);
per-parameter validation (E[net]>0 + Wilson-lower P_win, R²/half-life
stability, regime gate aborts-all); rejects stay HELD on manual values,
never fabricated. IPC `get_autotune_recommendation(symbol)` (live-book
ladder from hub cache only, no network). UI: Auto-tune button in futures
hurdle header (disabled <24h history or zero qualifying pairs), read-only
`AutotunePreviewModal` with fit-vs-validated rows + Copy/Close only — no
auto-apply writes. Prerequisite fix included: reset default
`maxTickerEquityPct` 2→20 to match `defaults_for`. Verification: 14 Rust
suites ok, clippy `--all-targets -D warnings` clean, tsc clean, npm
83 pass. Phase 2 (one-click apply) gated on Phase-1 acceptance data; no
schedules or background recalibration. Uncommitted; rebuild desktop binary
before relaunching.

---

## 14. Session Handoff — 2026-09-16 (Session-Aware Seasonality, Phase 1)

New `arbitrager-math/src/session.rs`: UTC session clock (Asia 00–08 /
Europe 08–13 / overlap 13–17 / US-late 17–22 / dead 22–24 + weekend),
funding masks (±120s around 00/08/16 + 04/12/20 + hourly), DST-aware US
cash hours + seeded NYSE holidays, winsorized empirical-Bayes shrinkage
(n0=150, provisional flags, weekend 2× floors), EWMA fast vol (λ=0.96).
Migration V8: `venue_volume_samples` (sampler on volume refresh) +
`session_profiles`/`session_hour_bins` (hourly 7-day rollup, prune-safe,
alias-normalized). Monitor `evaluate_pair`: additive entry adjust (2×
cap), sizing via allow × inverse-vol × depth, confidence gate,
stand-downs with explicit block reasons (funding → Ondo-closed → 3×
median → fast-vol halve-then-halt). IPC `get_session_profile` + Radar
badge/column, SignalTrend regime overlay, configurator SessionProfileCard
(display only). Verification: 348 Rust tests pass, clippy all-targets
clean, tsc clean, npm 93 pass. Profiles need a week of accumulation
before promotion review; Phase 2 (HAR anchor, autotune coupling) later.
Uncommitted; rebuild desktop binary before relaunching.

---

## 15. Session Handoff — 2026-09-16 (Gap-Closure Program)

Closed GitHub #1/#2/#4/#6/#7 (verified story-by-story) and #5 (bot
built); #3 left open (presets + full analytics scope remain). Win
probability wired into scan (shared episode/OU helper, informational).
GRVT + edgeX live-verified and admitted (8 paper venues; portfolios
rebased 600k→800k). Two-way exits now depth-walked; signal audit log
(v9). Autonomous convergence bot (paper-only, opt-in, gated entries,
managed exits, full audit; v11 positions; fixed radar ledger CHECK).
Seasonality Phase 2 (HAR anchor, promotion scheduler) + autotune
one-click apply (v12). Triangle backend (native scan/execute, paper);
convergence lifecycle with entry baselines (v13) — one-pair cap kept by
design (shared paper margin can't attribute per pair). Cloud deploy
bundle (ops/deploy: systemd template, env sample, backup, runbook).
Also: WAF-challenge cool-down (30m) ending the retry storm; WS-primary
(provenance + single-flight + jitter). Verification: 459 workspace tests
pass, clippy all-targets clean, tsc clean, npm 103 pass. Uncommitted;
rebuild desktop binary before relaunching.

---
*Security Notice: All API keys, secrets, private keys, and passwords are strictly excluded. The credential vault is encrypted with Argon2id + AES-256-GCM and `keychain.toml` is permanently ignored.*
