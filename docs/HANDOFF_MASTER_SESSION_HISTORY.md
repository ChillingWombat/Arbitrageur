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

# 3. Run frontend unit tests (22 passing vitest tests)
cd frontend
npm test -- --run
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
*Security Notice: All API keys, secrets, private keys, and passcodes are strictly excluded. The credential vault is encrypted with Argon2id + AES-256-GCM and `keychain.toml` is permanently ignored.*
