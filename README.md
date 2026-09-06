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

## 🌟 Key Features & Innovations

- **Ultra-Low Latency in Rust:** Sub-microsecond calculation latency (Kalman filter update in **19.15 ns**, Ornstein-Uhlenbeck MLE fitting in **71.98 µs**, Bertram threshold solver in **3.33 µs**).
- **Atomic Multi-Leg Coordination:** Concurrent parallel order execution (`tokio::join!`) with automated emergency unwind to eliminate legging risk.
- **Microstructure Depth Verification:** Calculates true Volume-Weighted Average Price (VWAP) across orderbook levels to prevent execution on phantom liquidity.
- **Enterprise Risk Guardrails:**
  - **Circuit Breaker:** Dollar drawdown halts, percentage stops, consecutive failure tripwires.
  - **DeltaGuard:** Real-time cross-exchange signed net delta exposure tracker enforcing true delta neutrality.
  - **MarginMonitor:** Maintenance margin ratio monitoring and liquidation distance buffer warnings (< 15%).
- **Hardened Rate Limiting:** `AccountRequestCoordinator` with token bucket throttling, in-memory TTL caching, and exponential backoff on HTTP 429 errors.
- **Modern Pixi Environment:** Unified environment and workflow management with zero-config reproducibility.

---

## 📐 Mathematical & Quantitative Finance Core

Built on rigorous academic quant algorithms implemented in pure Rust (`arbitrager-math`):
- **Ornstein-Uhlenbeck (OU) Mean-Reversion Process:** Continuous SDE $dX_t = \theta (\mu - X_t) dt + \sigma dW_t$ with closed-form MLE parameter estimation and half-life calculation $t_{1/2} = \frac{\ln 2}{\theta}$.
- **Bertram (2010) Optimal Analytic Thresholds:** Solves for trade entry and exit levels that analytically maximize expected return per unit time under transaction costs $c$.
- **2D Online Kalman Filter:** Recursively estimates dynamic hedge ratio $\beta_t$ and intercept $\alpha_t$ in $O(1)$ operations with zero allocation.
- **Funding Carry Compounding:** Hourly continuous rate normalization, APR/APY compounding ($\text{APY} = (1 + \Delta r_{\text{hourly}})^{8760} - 1$), and transaction fee break-even amortization time.
- **Prediction Market Synthetic Parity:** Binary Arrow-Debreu arbitrage ($P_A(\text{YES}) + P_B(\text{NO}) < 1.00 - \text{fees}$) and complete set Dutch book linear programming.

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

### 3. Run Microsecond Quantitative Benchmarks
```bash
pixi run bench
```
*Benchmark Output:*
```text
1. Kalman Filter 2D update (1,000,000 iterations):
   Per-update latency: 19.15 nanoseconds (52.21M updates/sec)
2. Ornstein-Uhlenbeck MLE fitting (10,000 fits on 5000-point series):
   Per-fit latency: 71.98 microseconds
3. Bertram Optimal Threshold Analytic Solver (10,000 solves):
   Per-solve latency: 3.33 microseconds
```

### 4. Run Paper Trading Simulation
Simulates concurrent spatial price spread and funding carry trading between Hyperliquid, Lighter, and Vanta:
```bash
pixi run run-sim
```

### 5. Scan Funding Rate Carry Opportunities
Displays normalized hourly rates, APR, APY, break-even hours, and daily income:
```bash
pixi run run-funding
```

### 6. Run Statistical Arbitrage Pair Tracker
Tracks cointegrated asset pairs with dynamic Kalman filter hedge ratios:
```bash
pixi run run-stat
```

### 7. Scan Prediction Markets
Evaluates synthetic binary arbitrage between Polymarket and Kalshi:
```bash
pixi run run-prediction
```

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
