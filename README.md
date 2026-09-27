# ⚡ Arbitrageur: Institutional-Grade Modular Cross-Market Arbitrage Engine

[![Rust](https://img.shields.io/badge/language-Rust_2021-orange.svg)](#)
[![Pixi](https://img.shields.io/badge/environment-Pixi-blue.svg)](#)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-green.svg)](#)
[![Tests](https://img.shields.io/badge/tests-passing-brightgreen.svg)](#)
[![Tauri](https://img.shields.io/badge/desktop-Tauri_v2-24C8DB.svg)](#)
[![React](https://img.shields.io/badge/frontend-React_19-61DAFB.svg)](#)

A high-performance, modular cross-venue quantitative arbitrage engine and native desktop terminal engineered in **Rust** and **Tauri v2 / React 19**. Designed for multi-asset execution across decentralized perpetuals, centralized exchanges, spot AMMs, options desks, interest-rate markets, and prediction contracts.

---

## 📑 Table of Contents

- [Core Principles & Safety Guarantees](#-core-principles--safety-guarantees)
- [System Architecture](#-system-architecture)
- [Crate Ecosystem](#-crate-ecosystem)
- [Trading Strategies & Execution Pathways](#-trading-strategies--execution-pathways)
- [Venue Coverage & Adapters (33+ Venues)](#-venue-coverage--adapters-33-venues)
- [Quantitative Finance & Microstructure Engine](#-quantitative-finance--microstructure-engine)
- [Security Architecture & Key Custody](#-security-architecture--key-custody)
- [User Interface & Desktop Terminal](#-user-interface--desktop-terminal)
- [Quick Start & Developer Guide](#-quick-start--developer-guide)
- [Verification & Quality Assurance](#-verification--quality-assurance)
- [Audited Reference Implementations](#-audited-reference-implementations)
- [License](#-license)

---

## 🛡️ Core Principles & Safety Guarantees

> [!IMPORTANT]
> **Live Mode is the Product**: Real venues, real orders, real capital. Arbitrageur is engineered for production live execution, with public demo endpoints for testnet verification. The platform enforces fail-closed execution safety across every layer.

- **Zero Synthetic Data Contamination**: If an exchange credential, network route, or orderbook is missing, unresponsive, or invalid, the engine strictly fails closed. Synthetic quotes, fabricated book levels, or dummy fills are never generated in production or browser mode.
- **Fail-Closed Dual-Environment Isolation (`testnet` vs `live`)**:
  - `TradingEnvironment::{Testnet, Live}` is strictly scoped across state stores, active adapters, feeds, and orders.
  - Adapters with demo endpoints (`tn_*`) connect exclusively to venue testnets; live adapters (`live_*`) connect to mainnets.
  - Venues without a public sandbox (Coinbase, Aster, EVM AMMs, Robinhood EVM) are cleanly gated and excluded from testnet execution rather than simulated.
- **`live_entry_gate` Access Control**:
  - Live strategy entries dispatch only when Trading Mode is set to `Live`, the credential vault is unlocked, and every leg's adapter implements order submission, authenticated `fetch_order` status polling, automated strategy transport, and holds valid live credentials.
  - Canary-proven live two-leg execution path (e.g. Hyperliquid ↔ Lighter ETH-USDC).
- **Automated Residual-Leg Resolution & Bounded Cancel Retry**:
  - Two-leg executions coordinate concurrently via `tokio::join!`.
  - Imbalanced or partial fills trigger automated reduce-only residual unwinds (passive limit-first with fallback to market) rather than unhedged exposure.
  - Unverifiable orders trigger immediate circuit breaker halts and escalate to `unresolved` state for operator review.
- **Continuous 5-Second Reconciliation Supervisor**:
  - A persistent background supervisor polls working orders through venue `fetch_order` endpoints at startup and every 5 seconds.
  - Ensures monotonic accounting transitions (`New` → `PartiallyFilled` → `Filled` / `Canceled` / `Rejected`).
- **Real-Time Risk Tripwires & Breakers**:
  - Background equity monitoring every 15 seconds across all admitted venues.
  - Configurable daily dollar drawdown limits, percentage halts, consecutive failure tripwires, and DeltaGuard dollar neutrality enforcement.

---

## 🏗️ System Architecture

Arbitrageur is built on a non-blocking asynchronous Tokio runtime with zero-cost abstractions, lock-free concurrency, and strict separation between domain primitives, quantitative math, venue protocols, risk management, execution coordination, and UI state.

```
                                  +------------------------------------+
                                  |     arbitrageur-desktop (Tauri)    |
                                  |   (IPC, SQLite Store, Feed Loop)   |
                                  +-----------------+------------------+
                                                    |
                         +--------------------------+--------------------------+
                         |                                                     |
             +-----------v------------+                           +------------v-----------+
             |  arbitrageur-strategy  |                           |    arbitrageur-risk    |
             |  - Spatial Price Arb   |                           |  - Circuit Breaker     |
             |  - Funding Carry Arb   |<--------------------------+  - Delta Guard         |
             |  - Stat-Arb Pairs (OU) |                           |  - Margin Monitor      |
             |  - Options Put-Call    |                           +------------+-----------+
             |  - Triangular Cycles   |                                        |
             +-----------+------------+                                        |
                         |                                                     |
             +-----------v------------+                                        |
             |  arbitrageur-execution |<---------------------------------------+
             |  - Atomic Coordinator  |
             |  - Passive Unwind      |
             |  - Emergency Flatten   |
             |  - Virtual Ledger      |
             +-----------+------------+
                         |
             +-----------v------------+                           +------------------------+
             |  arbitrageur-exchanges |                           |    arbitrageur-math    |
             |  - 33+ Live Adapters   |                           |  - 2D Kalman Filter    |
             |  - WS Feed Supervisors |                           |  - OU Process MLE      |
             |  - Rate Throttling     |                           |  - Bertram Thresholds  |
             |  - Lighter Signer FFI  |                           |  - Funding Carry APY   |
             +-----------+------------+                           +-----------+------------+
                         |                                                    |
                         +--------------------------+-------------------------+
                                                    |
                                        +-----------v------------+
                                        |    arbitrageur-core    |
                                        | (Types, Domain, Order) |
                                        +------------------------+
```

---

## 📦 Crate Ecosystem

| Crate | Path | Primary Responsibility | Key Abstractions |
|---|---|---|---|
| **`arbitrageur-core`** | [`crates/arbitrageur-core`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-core) | Domain primitives, precision decimal arithmetic, market models, unified adapter trait. | `OrderBook`, `BookLevel`, `Ticker`, `FundingRate`, `OrderRequest`, `OrderResponse`, `ExchangeAdapter`, `MarketDataHub` |
| **`arbitrageur-math`** | [`crates/arbitrageur-math`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-math) | Quantitative finance, time series models, statistical solvers, calibration routines. | `HedgeRatioKalmanFilter`, `OuModelParams`, `compute_bertram_thresholds`, `evaluate_funding_carry`, `evaluate_spatial_spread` |
| **`arbitrageur-exchanges`** | [`crates/arbitrageur-exchanges`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges) | Connectors for CEX, DEX, AMM, and hybrid venues; protocol signing; rate limiting. | `HyperliquidAdapter`, `LighterAdapter`, `OkxAdapter`, `BinanceAdapter`, `DeribitAdapter`, `EvmAmmAdapter`, `BorosAdapter` |
| **`arbitrageur-risk`** | [`crates/arbitrageur-risk`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-risk) | Real-time circuit breaker tripwires, delta exposure guards, margin monitoring. | `CircuitBreaker`, `DeltaGuard`, `MarginMonitor`, `CircuitBreakerConfig` |
| **`arbitrageur-execution`** | [`crates/arbitrageur-execution`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-execution) | Concurrent multi-leg dispatch, automated residual unwind, smart order routing. | `AtomicArbitrageCoordinator`, `PassiveUnwindEngine`, `EmergencyUnwindHandler`, `SmartOrderRouter`, `VirtualLedger` |
| **`arbitrageur-strategy`** | [`crates/arbitrageur-strategy`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-strategy) | Autonomous strategy engines, signal evaluation, and convergence supervisors. | `SpatialPriceArbStrategy`, `FundingRateArbStrategy`, `StatArbPairsStrategy`, `OptionsParity`, `ArbitrageEngine` |
| **`arbitrageur-app`** | [`crates/arbitrageur-app`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-app) | Headless CLI, TUI, benchmarking runners, simulated test benches. | `Cli`, `run_benchmarks`, `run_monitor`, `run_simulate`, `run_funding_scan` |
| **`arbitrageur-desktop`** | [`frontend/src-tauri`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri) | Tauri v2 backend, SQLite state storage, encrypted vault, supervisors, IPC bridge. | `AppEngineState`, `StateStore`, `SecureVault`, `commands`, `reconciliation`, `feed`, `auto_exec` |

---

## 📈 Trading Strategies & Execution Pathways

### 1. Cross-Venue Perpetual Spatial Arbitrage
- **Mechanism**: Captures orderbook dislocations across venues by simultaneously buying on the underpriced venue and selling on the overpriced venue (`tokio::join!`).
- **Microstructure Depth**: Computes Volume-Weighted Average Price (VWAP) across orderbook rungs to prevent execution on phantom top-of-book depth.
- **Route Economics Gate**: Dynamically evaluates entry/exit fees, bid/ask crossings, and slippage budgets; aborts if expected net yield falls below target profit (default 1 BPS).
- **Execution Lifecycle**: Matched pairs remain open until spread convergence into the friction band, stop loss, or timeout, supervised every 5 seconds.

### 2. Delta-Neutral Funding Rate Carry & Basis
- **Mechanism**: Simultaneously holds long perp / short perp (or long spot / short perp) across diverging funding rates to collect continuous periodic carry payouts.
- **Compounding Math**: Truthful hourly continuous normalization, APR conversion, and APY compounding with overflow guards.
- **Min-APR Exit Supervisor**: Continuously monitors open carry positions every 60 seconds; EMA-smoothed decay below the hurdle triggers automated, orderly position unwinds.

### 3. Options ↔ Perpetual Parity (Deribit)
- **Mechanism**: Exploits put-call parity violations on European options against the underlying index/perpetual:
  $$C(S, K, T) - P(S, K, T) = S_0 - K \cdot e^{-rT}$$
- **Execution Types**:
  - **Conversion**: Long Underlying + Long Put + Short Call (synthetic short vs long spot).
  - **Reverse Conversion (Reversal)**: Short Underlying + Short Put + Long Call.
  - **Structured Option Packages**: Box Spreads (synthetic cash lending/borrowing), Jelly Rolls, and Negative-Risk Butterflies.
- **Risk Control**: Three sequential legs under atomic pre-dispatch checks; partial fills resolve as `imbalanced` for residual handling.

### 4. Single-Venue Triangular Arbitrage
- **Mechanism**: Exploits cyclical cross-rate pricing inefficiencies across three pairs on the same venue (e.g., `A/B` $\to$ `B/C` $\to$ `C/A`).
- **Atomic Dispatch**: Dispatches bounded IOC orders sequentially with pre-flight cashflow currency normalization and VWAP slippage verification.

### 5. Statistical Arbitrage (Pairs Cointegration)
- **Mechanism**: Models spread mean-reversion using a 2D recursive online Kalman filter ($\beta_t$ hedge ratio) combined with Ornstein-Uhlenbeck continuous SDE MLE parameter estimation.
- **Threshold Search**: Bertram-style numerical search optimizing expected return per unit time under transaction costs.

### 6. Fixed-Yield & Prediction Market Research
- **Pendle Boros**: Arbitrum YU interest rate swaps for funding rate fixed-yield hedging.
- **Prediction Markets**: Polymarket (CTF CLOB) and Kalshi (CFTC) cross-venue binary contract parity evaluation ($P_A(\text{YES}) + P_B(\text{NO}) < 1.00 - \text{fees}$).

---

## 🌐 Venue Coverage & Adapters (33+ Venues)

Arbitrageur supports 33+ exchange connectors spanning perpetual DEXs, centralized derivatives, spot AMMs, and fixed-yield protocols:

| Category | Venues | Features & Execution Readiness |
|---|---|---|
| **Tier-1 Perp DEXs** | **Hyperliquid**, **Lighter**, **Robinhood Lighter**, **Vanta (Orderly)**, **Aster**, **PopDEX** | Direct signed REST/WS order placement, custom FFI cryptographic signers, order polling, WebSocket feed supervisors. |
| **Tier-1 Derivatives CEXs** | **Binance**, **OKX**, **Deribit**, **Kraken**, **Coinbase**, **Alpaca** | Sub-millisecond signed REST order routing, validated `fetch_order` polling, portfolio margin (Deribit), demo sandbox support. |
| **DexPerp Long-Tail** | **dYdX Chain**, **Pacifica**, **StandX**, **Ondo**, **GRVT**, **Derive**, **Aevo**, **Vest**, **QFEX**, **Nado**, **EVEDEX**, **Paradex**, **edgeX**, **Extended**, **Helix/Injective** | High-performance protocols covering StarkEx SNIP-12, EIP-712, Cosmos SDK gRPC, and Ed25519 signing. |
| **Spot AMMs (EVM)** | **Uniswap V2/V3**, **PancakeSwap V2/V3**, **SushiSwap**, **QuickSwap**, **Camelot**, **Aerodrome**, **Velodrome** | On-chain Quoter reads, executable VWAP book synthesis, wallet-signed batched swaps with on-chain receipt proof. |
| **Fixed-Yield / Rates** | **Pendle Boros (Arbitrum)** | YU rate swap orderbook ladders, agent-approved EIP-712 signed execution. |
| **Prediction Research** | **Polymarket**, **Kalshi** | Real CLOB orderbook streaming, binary parity calculation (scaffold / research). |

---

## 🧮 Quantitative Finance & Microstructure Engine

Implemented in pure Rust (`arbitrageur-math`):

- **Continuous Mean-Reversion (Ornstein-Uhlenbeck SDE)**:
  $$dX_t = \theta (\mu - X_t) dt + \sigma dW_t$$
  - Closed-form Maximum Likelihood Estimation (MLE) of speed $\theta$, long-term mean $\mu$, and volatility $\sigma$.
  - Exact half-life calculation: $t_{1/2} = \frac{\ln 2}{\theta}$.
  - Hitting-time differential solver for optimal exit boundaries.
- **Bertram-Style Optimal Arbitrage Thresholds**:
  - Numerical optimization maximizing expected Sharpe and annualized return per unit of time under bid-ask crossing and fee friction $c$.
- **2D Online Kalman Filter**:
  - Dynamically updates hedge ratio $\beta_t$ and intercept $\alpha_t$ at $O(1)$ complexity with guaranteed covariance matrix symmetrization:
  $$y_t = \alpha_t + \beta_t x_t + \epsilon_t$$
- **Microstructure VWAP Slippage Engine**:
  - Evaluates cumulative volume-weighted execution price through orderbook ladders, rejecting trades where book depth cannot absorb sizing without adverse impact.
- **Multi-Quote Depeg Protection**:
  - Ingests real-time stablecoin FX feeds (USDT, USDC, USDS) with configurable basis point deviation tripwires, preventing execution on depegged synthetic pairs.

---

## 🔐 Security Architecture & Key Custody

1. **Encrypted Credential Vault**:
   - Master credential vault encrypted locally via **AES-256-GCM** with **Argon2id** password key derivation.
   - Enforces `0600` file permissions on Unix (`~/.arbitrageur/vault.enc`) and `0700` directory permissions.
   - **Zero Plaintext Leaks**: No whole-bundle credential read IPC; field-scoped reveals require explicit password verification.
2. **Hardware & Stronghold Custody**:
   - Shared EVM wallet private keys are isolated inside **Iota Stronghold** snapshots (`~/.arbitrageur/vault.hold`) with separate encryption keys and memory protection.
3. **Automated Security Overlays & MFA**:
   - Full UI backdrop blur and entry lockout when the vault is locked.
   - Automatic 60-second inactivity lock timer on navigation away from credential settings.
   - Optional RFC-6238 TOTP two-factor authentication for vault unlock and key management.
4. **Signer Binary Integrity**:
   - External shared libraries (such as `liblighter_signer.so`) are verified via SHA-256 checksums prior to runtime dynamic linking (`dlopen`), preventing binary tampering.

---

## 🖥️ User Interface & Desktop Terminal

The frontend is a desktop trading terminal built with **React 19**, **TypeScript**, and **Tailwind CSS**, embedded via **Tauri v2**:

- **⚡ Live Spatial Matrix**: Real-time cross-venue L2 orderbook ladder visualization with VWAP pricing, net dislocation indicators, and direct execution ticketing.
- **📊 Opportunity Radar**: Autonomous spread range scanner with real-time spread tracking, configurable entry/exit percentiles, and automated bot controls.
- **📈 Funding Carry Radar**: Normalized hourly rates, APR/APY compounding calculator, break-even amortization time, and automated carry bot controls.
- **🎯 Options Arbitrage Hub**: Put-call parity scanner for Deribit options with interactive conversion, reversal, and structured package trading cards.
- **🔬 Stat-Arb Lab**: Interactive 2D Kalman filter hedge ratio ($\beta_t$) charts, z-score sparklines, and cointegration testing.
- **🛡️ Portfolio & Risk Monitor**: Consolidated multi-venue equity attribution, margin utilization, live DeltaGuard net exposure, and manual kill switch.
- **⚙️ Dynamic Ticker Explorer**: Dynamic discovery of `BASE-QUOTE` markets across live venues with liquidity filtering and per-asset strategy configurator.

---

## 🚀 Quick Start & Developer Guide

### Prerequisites

Ensure you have installed:
- [Rust 2021](https://rustup.rs/) (`rustc --version` >= 1.78)
- [Node.js](https://nodejs.org/) (`node --version` >= 20.0)
- [Pixi](https://pixi.sh/) (Recommended environment manager)

### 1. Installation & Environment Setup

Using Pixi:
```bash
git clone https://github.com/ChillingWombat/Arbitrageur.git
cd Arbitrageur
pixi install
```

Or using standard toolchains:
```bash
cargo build
cd frontend && npm install && cd ..
```

### 2. Running Quantitative Benchmarks
Measure Kalman filter latency, OU MLE parameter fitting, and threshold searches:
```bash
pixi run bench
# or: cargo run --release --bin arbitrageur-app -- bench
```

### 3. Launching the Desktop Trading Workstation
Run the full Tauri desktop application in development mode:
```bash
pixi run desktop
# or: cargo tauri dev
```

### 4. Compiling the Production Release Binary
Build an optimized release desktop binary with the custom protocol feature:
```bash
cargo build --release --bin arbitrageur-desktop --features custom-protocol
```
> [!TIP]
> The `--features custom-protocol` flag is required for production Tauri packaging so the webview correctly serves compiled frontend assets rather than looking for a dev server.

---

## 🧪 Verification & Quality Assurance

Arbitrageur maintains high test coverage across all crates and frontend components.

Run all test suites:

```bash
# 1. Frontend component & quant tests
cd frontend && npm test

# 2. Desktop backend test suite (560+ tests)
cargo test -p arbitrageur-desktop

# 3. Exchange adapters test suite (270+ tests)
cargo test -p arbitrageur-exchanges

# 4. Math, core, execution, risk, and strategy crates
cargo test -p arbitrageur-core -p arbitrageur-math -p arbitrageur-risk -p arbitrageur-execution -p arbitrageur-strategy

# 5. Format and Linter verification
cargo fmt --check
cargo clippy --workspace --all-targets
```

---

## 🔬 Audited Reference Implementations

Arbitrageur integrates quantitative, architectural, and operational insights derived from rigorous audits of 10 key open-source trading and cryptographic repositories:

1. `lihanyu81/PopDEX---Lighter-Arbitrage-Tool-From-PandaZhai`: Baseline position reconciliation, rate-limited request coordination, and Lighter ZK signing.
2. `taoli-tools/taoli-tools-signer`: Isolated cryptographic key management and multi-chain DEX router allowlists (including Robinhood EVM).
3. `taoli-tools/taoli-tools-container`: Container sandboxing, secure browser profiles, and process isolation.
4. `cryptocj520/bphltaoli`: Multi-exchange funding carry logic, cooldown periods, and depth ratio checks.
5. `cryptocj520/crypto-trading-open`: Enterprise exchange adapter architecture and multi-leg order mapping.
6. `realfishsam/prediction-market-arbitrage-bot`: Prediction market synthetic parity models and contract pairing.
7. `lihanyu81/polymarket_lp_tool`: Rust market-making bot architecture and EIP-712 order signing.
8. `hudson-and-thames/arbitragelab`: Re-implemented Ornstein-Uhlenbeck MLE, Bertram optimal thresholds, and Kalman filters.
9. `cryptocj520/meteora`: Dynamic fee DLMM pool arbitrage patterns.
10. `cryptocj520/pancake`: Universal router multi-chain swap paths and on-chain receipt verification.

---

## 📜 License

Arbitrageur is open-source software dual-licensed under:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

You may choose to use this software under either license at your discretion.
