# ⚡ Arbitrager: Ultra-Fast Modular Cross-Market Arbitrage Engine

[![Rust](https://img.shields.io/badge/language-Rust_2021-orange.svg)](#)
[![Pixi](https://img.shields.io/badge/environment-Pixi-blue.svg)](#)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-green.svg)](#)
[![Tests](https://img.shields.io/badge/tests-passing-brightgreen.svg)](#)

A high-performance, modular cross-market arbitrage platform engineered in **Rust** for simultaneous trading across multiple decentralized and centralized venues.

The system targets **Spatial Price Differences** (orderbook spread capture) and **Funding Rate Differences** (delta-neutral perpetual carry), expanding progressively across:
1. **Blockchain Coins & Perpetual DEXs:** Hyperliquid, Lighter (zk-DEX), Vanta.exchange (Orderly Network), Robinhood EVM.
2. **Prediction Markets:** Polymarket (CTF CLOB) and Kalshi (CFTC-regulated).
3. **Traditional Markets:** Statistical arbitrage, basis trading, and cointegrated pairs.

---

## Current Safety Status

This repository is a **research and paper-trading platform only**. Read this before running anything:

- **Live trading is disabled and fails closed.** Order signing, submission, and cancellation are not implemented for any venue. Every real/live order action and every unsupported native execution path (spot, cross-ticker, funding, prediction) returns an explicit error rather than simulating success; spatial and single-venue triangular (A→B→C→A) **paper** execution on the built-in simulator are supported.
- **The desktop app admits only Hyperliquid, Lighter, Robinhood Lighter, and Vanta (Orderly)**, and only their **read-only public market-data feeds** in paper mode. Lighter funding-rate ingestion is unavailable. These unauthenticated requests disclose your IP address and the requested symbols to the venue endpoints.
- **Arcus, PopDEX, Polymarket, and Kalshi adapters are scaffolds.** They are not wired into native scanning or execution and cannot place orders.
- **Ticker availability is derived from live `fetch_markets` intersections** across admitted venues — not from the static 52-row catalog's venue claims. A ticker is offered only when at least two venues actually report the market.
- **Signal baselines use persisted authentic spread samples.** The sliding window accepts a manual duration from 5 minutes to 30 days (default 24 hours), or per-ticker **auto mode**: the median across qualifying venue pairs of `2 × OU half-life`, floored at 30 minutes and capped at 7 days, behind R²/data gates. When history is insufficient, auto mode uses the explicitly labeled manual fallback — and pairs without enough durable baseline coverage cannot trigger divergence execution.
- **Browser mode has no synthetic demo engine.** All engine and quote calls require the desktop application's IPC; in the browser they fail closed with the desktop-required error rather than fabricating execution, balances, quotes, P&L, risk, or calibration. Tauri command errors propagate to the UI — there is no fallback path.
- **Paper spatial execution** uses freshly fetched, identity- and structure-validated orderbooks, bounded IOC (immediate-or-cancel) simulation, a maximum 5x leverage, and margin/delta/depth/slippage checks — plus a backend-computed round-trip route-economics gate: expected entry and exit slippage, both venues' current bid/ask crossing, and round-trip commissions must leave expected net at or above the configured target profit (default 1 BPS) before dispatch. Only **one open pair per symbol** is supported by design (shared per-venue paper accounts cannot attribute margin per pair, and flatten is symbol-scoped and would cross-close independent pairs). Positions carry a persisted entry baseline, and the `check_spatial_convergence` IPC unwinds converge/stop/timeout exits through the flatten path, freeing the slot for re-entry; the Spread Radar bot has its own equivalent lifecycle.
- **Credential vault:** local AES-256-GCM encryption with Argon2id key derivation, `0600` file / `0700` directory permissions on Unix, and write-only secret entry from the UI (there is no credential-read IPC). Even so, **only ever enter read-only or minimum-permission API credentials — never funded signing keys**.

Nothing here is production-ready. Real-time WebSocket ingestion (Hyperliquid/Lighter/Vanta) feeds the MarketDataHub with REST fallback; the UI still polls the hub on a ~3s cadence.

---

## 🌟 Key Features & Innovations

- **Low-Latency Math in Rust:** Nanosecond/microsecond-scale calculation latency (historical machine-specific examples below — not guaranteed).
- **Concurrent Two-Leg Coordination:** Parallel order dispatch (`tokio::join!`) with best-effort emergency compensation; unresolved residual exposure trips the circuit breaker and is surfaced for operator review rather than hidden.
- **Microstructure Depth Verification:** Calculates true Volume-Weighted Average Price (VWAP) across orderbook levels to prevent execution on phantom liquidity.
- **Risk Guardrails:**
  - **Circuit Breaker:** Dollar drawdown halts, percentage stops, consecutive failure tripwires.
  - **DeltaGuard:** In-memory signed net delta exposure tracker per asset.
  - **MarginMonitor:** Maintenance margin ratio monitoring and liquidation distance buffer warnings (< 15%).
- **Hardened Rate Limiting:** `AccountRequestCoordinator` with token bucket throttling, in-memory TTL caching, and exponential backoff on HTTP 429 errors.
- **Modern Pixi Environment:** Unified environment and workflow management with zero-config reproducibility.

---

## 📐 Mathematical & Quantitative Finance Core

Built on rigorous academic quant algorithms implemented in pure Rust (`arbitrager-math`):
- **Ornstein-Uhlenbeck (OU) Mean-Reversion Process:** Continuous SDE $dX_t = \theta (\mu - X_t) dt + \sigma dW_t$ with closed-form MLE parameter estimation and half-life calculation $t_{1/2} = \frac{\ln 2}{\theta}$.
- **Bertram-Style Threshold Search:** Numerical search for trade entry and exit levels targeting improved expected return per unit time under transaction costs $c$ (inspired by Bertram 2010; not a closed-form analytic solver).
- **2D Online Kalman Filter:** Recursively estimates dynamic hedge ratio $\beta_t$ and intercept $\alpha_t$ in $O(1)$ operations with zero allocation.
- **Funding Carry Compounding:** Hourly continuous rate normalization, APR/APY compounding ($\text{APY} = (1 + \Delta r_{\text{hourly}})^{8760} - 1$), and transaction fee break-even amortization time.
- **Prediction Market Parity Research:** Cross-venue binary contract comparison ($P_A(\text{YES}) + P_B(\text{NO}) < 1.00 - \text{fees}$). A $1 payout is conditional on both contracts having identical settlement terms; displayed returns are indicative, never guaranteed.

---

## 🏗 Modular Architecture

```
Arbitrager/
├── pixi.toml                             # Pixi environment & task orchestration
├── Cargo.toml                            # Cargo workspace manifest
├── README.md                             # System documentation
├── docs/
│   ├── 01_QUANT_MATH_RESEARCH.md         # Comprehensive quantitative finance treatise
│   ├── 02_REFERENCE_PROJECTS_ANALYSIS.md # Reverse-engineering audit of 10 open-source repos
│   ├── 03_SYSTEM_ARCHITECTURE.md         # Concurrency, state machines & threat model
│   ├── 04_EXCHANGE_INTEGRATIONS_GUIDE.md # Hyperliquid, Lighter, Vanta, Robinhood, Polymarket
│   └── 05_BUGS_SOLUTIONS_AND_RUNBOOK.md  # Failure modes, mitigation strategies & runbook
└── crates/
    ├── arbitrager-core/                  # Domain types, Decimal math, unified ExchangeAdapter trait
    ├── arbitrager-math/                  # Kalman filter, OU MLE, Bertram thresholds, carry APY
    ├── arbitrager-exchanges/             # Hyperliquid, Lighter, Vanta, Robinhood, Simulated
    ├── arbitrager-risk/                  # CircuitBreaker, DeltaGuard, MarginMonitor
    ├── arbitrager-execution/             # AtomicArbitrageCoordinator, EmergencyUnwindHandler
    ├── arbitrager-strategy/              # SpatialPriceArb, FundingRateArb, StatArbPairs
    └── arbitrager-app/                   # CLI binary with monitor, simulate, funding, bench commands
```

---

## 🚀 Quick Start with Pixi

### 1. Prerequisites
Ensure `pixi` and `cargo` are installed:
```bash
pixi --version
cargo --version
```

### 2. Verify and Run Tests
```bash
pixi run check
pixi run test
```

### 3. Run Quantitative Benchmarks
```bash
pixi run bench
```
*Example output from one historical development machine — machine-specific and not guaranteed:*
```text
1. Kalman Filter 2D update (1,000,000 iterations):
   Per-update latency: 19.15 nanoseconds (52.21M updates/sec)
2. Ornstein-Uhlenbeck MLE fitting (10,000 fits on 5000-point series):
   Per-fit latency: 71.98 microseconds
3. Bertram-Style Threshold Search (10,000 solves):
   Per-solve latency: 3.33 microseconds
```

### 4. Run Paper Trading Simulation
Research/demo only: simulates spatial spread and funding carry trading between in-process simulated venues (no live execution):
```bash
pixi run run-sim
```

### 5. Scan Funding Rate Carry Opportunities
Research demo using static example rates: displays normalized hourly rates, APR, APY, break-even hours, and estimated daily income (not live venue data):
```bash
pixi run run-funding
```

### 6. Run Statistical Arbitrage Pair Tracker
Tracks cointegrated asset pairs with dynamic Kalman filter hedge ratios on a synthetic series:
```bash
pixi run run-stat
```

### 7. Scan Prediction Markets
Research demo evaluating cross-venue binary contract parity between Polymarket and Kalshi; any payout is conditional on identical settlement terms (not live venue execution):
```bash
pixi run run-prediction
```

---

## 🖥️ Modern Desktop Frontend (Tauri v2 + TypeScript + React)

Arbitrager includes a native, low-latency quantitative trading workstation frontend engineered with **Tauri v2**, **TypeScript**, **React**, and **Tailwind CSS**.

### 🌟 Frontend Capabilities
- **⚡ Spatial Matrix:** Read-only public L2 orderbooks from Hyperliquid and Lighter (the only admitted venues) with depth visualization, VWAP calculation, and one-click **paper** execution against the built-in simulator.
- **📈 Funding Carry Radar:** Funding carry research view with normalized rates, APR, compounded APY, and break-even estimates; native funding execution is disabled (Lighter funding ingestion is unavailable).
- **🎯 Prediction Parity Hub:** Cross-venue parity research for Polymarket CTF CLOB and Kalshi CFTC contracts ($P_A(\text{YES}) + P_B(\text{NO}) < 1.00 - \text{fees}$); payouts are conditional on identical settlement terms and shown returns are indicative only.
- **🔬 Stat-Arb Lab:** In-memory 2D Kalman filter dynamic hedge ratio ($\beta_t$) and z-score sparkline chart, Bertram-style numerical threshold search, and Ornstein-Uhlenbeck AR(1) MLE fitting.
- **🛡️ DeltaGuard & Risk Operations:** Current in-memory signed net delta tracking per asset, circuit breaker tripwires (dollar & percentage drawdown), maintenance margin proximity, and manual emergency kill switch.
- **🧪 Simulation Studio:** High-frequency paper trading engine with continuous auto-run loop, tick-by-tick order trace, and emergency unwind visualization.
- **⚙️ Credential Vault:** Local encrypted credential storage (AES-256-GCM + Argon2id); remote HSM signing and live order submission are not implemented.

### 🚀 Running the Frontend

#### 1. Web Dev Server (Browser Mode)
```bash
pixi run dev-frontend
```
Open `http://localhost:1420` in your browser. In browser mode, desktop IPC and order submission are unavailable: all engine and quote calls (execution, balances, quotes, P&L, risk, calibration) fail closed with the desktop-required error instead of serving synthetic data.

#### 2. Build Production Web Assets
```bash
pixi run build-frontend
```

#### 3. Launch Native Tauri Desktop Application
```bash
pixi run desktop
```
*(For optimized release desktop binary, run `pixi run desktop-release`)*

---

## 🔬 Audited Reference Projects

The system integrates insights gained from reverse-engineering and auditing 10 key open-source repositories:
1. `lihanyu81/PopDEX---Lighter-Arbitrage-Tool-From-PandaZhai`: Extracted PyInstaller ELF binary; adopted step-based baseline position reconciliation and `AccountRequestCoordinator`.
2. `taoli-tools/taoli-tools-signer`: Adopted remote HSM key management and multi-chain DEX router allowlists (including Robinhood EVM).
3. `taoli-tools/taoli-tools-container`: Container isolation and secure browser profiles.
4. `cryptocj520/bphltaoli`: Multi-exchange funding carry logic, cooldown periods, and depth ratios.
5. `cryptocj520/crypto-trading-open`: Enterprise exchange adapter architecture and multi-leg pair mapping.
6. `realfishsam/prediction-market-arbitrage-bot`: Prediction market synthetic parity and fuzzy market pairing.
7. `lihanyu81/polymarket_lp_tool`: Rust MM bot architecture and EIP-712 order signing.
8. `hudson-and-thames/arbitragelab`: Re-implemented OU process MLE, Bertram thresholds, and Kalman filters.
9. `cryptocj520/meteora`: Dynamic fee DLMM pool arbitrage patterns.
10. `cryptocj520/pancake`: Universal router multi-chain swap paths.

---

## 📜 License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
