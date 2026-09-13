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
1. **WebSocket Orderbook Streaming Integration**:
   - Hyperliquid already features direct REST/WS endpoints.
   - Wire persistent WebSockets for Lighter zkDEX, Vanta (Orderly), and Robinhood EVM to achieve continuous sub-10ms quote updates.
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
- **Default window = the bot's configured trailing window** (`BOT Xm` chip, `botWindowSec` prop fed by `WINDOW_SECONDS[windowOption]`); presets extend to **5m/15m/1h/4h/1d/3d/7d**.
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
*Security Notice: All API keys, secrets, private keys, and passwords are strictly excluded. The credential vault is encrypted with Argon2id + AES-256-GCM and `keychain.toml` is permanently ignored.*
