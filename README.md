# Arbitrageur (In Development)

Arbitrageur is an experimental cross-market arbitrage engine and desktop terminal written in Rust with a Tauri v2 / React frontend. It scans order books and funding rates across decentralized exchanges, centralized platforms, and automated market makers to identify price dislocations and carry opportunities.

The project is in early development. While core mathematical models, market data pipelines, and several execution flows are implemented, parts of the system remain experimental scaffolds.

---

## Current Status and Development Stage

Arbitrageur is a trading and research tool under active development. Keep several practical constraints in mind:

- **Early development**: Many features are still being tested, refined, or refactored. Treat every execution path as experimental.
- **Pre-1.0 versioning**: Package metadata is currently `0.1.0`. Release tags must match the workspace version, and the first published release is `v0.1.0`.
- **Fail-closed design**: The engine does not generate synthetic order books, fake balances, or simulated quotes if credentials or network feeds are missing. If an endpoint is unreachable, the system stops and reports an error instead of guessing.
- **Dual-environment isolation**: The engine separates `testnet` and `live` environments. Testnet modes submit real orders against exchange demo endpoints (such as Hyperliquid testnet, Lighter testnet, Binance testnet, OKX demo mode, or Alpaca paper). Venues without public sandbox environments are excluded from testnet rather than simulated locally.
- **Live execution gate**: Live order submission requires an explicit toggle to Live Mode, an unlocked credential vault, valid live API keys, and an adapter that supports both order submission and order status polling (`fetch_order`). An automated reconciliation supervisor monitors working orders every 5 seconds to track fills and cancellations.
- **Residual exposure handling**: Multi-leg trades run concurrently. If one leg fills while another rejects or partially fills, the execution engine attempts an automated reduce-only unwind on the filled leg to limit unhedged exposure. If an order cannot be confirmed with the exchange, it escalates to an unresolved state for manual inspection.

---

## Strategy Modules

### Spatial Price Arbitrage
Monitors order books across multiple venues for the same trading pair. When the bid on one exchange exceeds the ask on another by more than estimated fees and slippage, the engine can enter both legs concurrently. It calculates volume-weighted average price (VWAP) across depth levels so that trades do not fire against thin liquidity.

### Funding Rate Carry and Basis Trading
Tracks periodic funding payments on perpetual futures contracts. It identifies delta-neutral carry trades (holding long and short perpetual positions on different exchanges, or pairing spot with a short perpetual). An exit supervisor checks positions every minute and unwinds them if net yield falls below entry targets.

### Options Put-Call Parity
Monitors European options on Deribit for violations of put-call parity:

$$C(S, K, T) - P(S, K, T) = S_0 - K \cdot e^{-rT}$$

Identifies conversions (long spot, long put, short call), reverse conversions (short spot, short put, long call), and multi-leg spreads such as box spreads and butterflies.

### Single-Venue Triangular Arbitrage
Scans for cyclic price discrepancies across three trading pairs on a single exchange (for example, BTC/USDC, ETH/BTC, ETH/USDC). Orders execute sequentially with limit checks to verify profitability after accounting for fees.

### Statistical Arbitrage & Probability-Driven Invalidation
Tracks cointegrated pairs using an online two-dimensional Kalman filter to estimate dynamic hedge ratios. It fits price spreads to an Ornstein-Uhlenbeck mean-reverting process using maximum likelihood estimation to estimate half-life and live mean-reversion probability ($P_{\text{win}}$). Positions exit dynamically when the statistical thesis invalidates ($P_{\text{win}} < 0.50$) or when spreads converge, completely eliminating rigid wall-clock timers and premature carry churn.

### Prediction Market Research
Scans event contracts across Polymarket and Kalshi for binary pricing inconsistencies ($P(\text{YES}) + P(\text{NO}) < 1.00 - \text{fees}$). These adapters are currently research scaffolds without automated trade execution.

---

## Supported Venues

The codebase contains adapters for 33+ venues across different categories. Support levels vary depending on venue API stability and whether order status polling is available:

- **Perpetual DEXs**: Hyperliquid, Lighter (with vendored signer), Robinhood Lighter, Vanta (Orderly Network), Aster, PopDEX.
- **Centralized Exchanges & Brokerages**: Binance (USDT-M perps), OKX, Deribit (options and perps), Interactive Brokers (IBKR equity/index options), Kraken, Coinbase (spot), Alpaca (equities and crypto spot).
- **Decentralized Perpetual Protocols**: dYdX Chain, Pacifica, StandX, Ondo Perps, GRVT, Derive, Aevo, Vest Markets, QFEX, Nado, EVEDEX, Paradex, edgeX, Extended, Helix/Injective.
- **EVM Automated Market Makers**: Uniswap V2/V3, PancakeSwap V2/V3, SushiSwap, QuickSwap, Camelot, Aerodrome, Velodrome.
- **Interest Rate Swaps**: Pendle Boros (Arbitrum YU rate markets).
- **Prediction Markets (Research only)**: Polymarket, Kalshi.

For detailed status on each adapter, see [docs/CAPABILITIES.md](docs/CAPABILITIES.md) and [docs/EXCHANGES.md](docs/EXCHANGES.md).

---

## Repository Structure

The workspace is organized into eight crates:

- `crates/arbitrageur-core`: Core data structures (`OrderBook`, `Ticker`, `OrderRequest`, `Position`), precision math wrappers, and the `ExchangeAdapter` trait.
- `crates/arbitrageur-math`: Quantitative algorithms including Kalman filter updates, Ornstein-Uhlenbeck MLE fitting, Bertram threshold calculations, and funding carry APY math.
- `crates/arbitrageur-exchanges`: Venue adapters, rate limiters, WebSocket ingestion routines, and signing implementations.
- `crates/arbitrageur-risk`: Risk checks, portfolio margin monitors, and circuit breakers (daily loss limits, consecutive error counters, DeltaGuard exposure bounds).
- `crates/arbitrageur-execution`: Multi-leg execution coordinator, passive limit unwind engine, and virtual ledger tracking.
- `crates/arbitrageur-strategy`: Strategy signal generators and convergence tracking routines.
- `crates/arbitrageur-app`: Standalone command-line interface, benchmarking utilities, and local simulation harnesses.
- `frontend/src-tauri`: Desktop application backend (`arbitrageur-desktop`) providing IPC commands, local SQLite storage, encrypted vault management, and WebSocket feed supervision.
- `frontend/src`: Trading terminal user interface built with React 19, TypeScript, and Tailwind CSS.

---

## Security and Credential Storage

API credentials and private keys are protected on the local filesystem:

1. **Local AES-256-GCM Vault**: Exchange credentials are encrypted on disk (`~/.arbitrageur/vault.enc`) using Argon2id key derivation. The file is created with restricted Unix permissions (`0600`).
2. **Iota Stronghold Storage**: Private keys for shared EVM wallet operations are stored separately in an Iota Stronghold snapshot (`~/.arbitrageur/vault.hold`).
3. **Session Auto-Lock**: The user interface locks the credential view after 60 seconds of inactivity or when navigating away from the settings tab.
4. **Two-Factor Authentication**: Supports optional RFC-6238 TOTP verification when unlocking the vault or editing keys.

---

## Getting Started

### Prerequisites

You will need:
- Rust (pinned by `rust-toolchain.toml` — 1.96.0)
- Node.js (v22.6 or newer — the frontend test suite uses
  `node --experimental-strip-types`; CI pins v24)
- Pixi (optional, recommended for managing dependencies and tasks)

### Supported platforms

CI compile-checks every crate on **Linux, macOS and Windows**, and runs the
Rust and frontend test suites on Linux.

Per-platform system dependencies (only needed to build the Tauri webview):

| Platform | Requirements |
|---|---|
| Linux | `libgtk-3-dev libwebkit2gtk-4.1-dev librsvg2-dev libdbus-1-dev` (Debian/Ubuntu) |
| macOS | Xcode command-line tools (no extra packages) |
| Windows | MSVC build tools + WebView2 (the installer downloads it if missing) |

Two components remain Linux-only:

- **Lighter signer** — only `liblighter_signer.so` is vendored and
  digest-pinned; Lighter is unavailable on other platforms until a
  Windows/macOS signer is published.
- **Headless daemon deployment** — `ops/deploy/` ships systemd units. The
  headless CLI itself compiles everywhere; only the service templates are
  Linux-specific.

### Building the Project

Clone the repository:
```bash
git clone https://github.com/ChillingWombat/Arbitrageur.git
cd Arbitrageur
```

If you use Pixi:
```bash
pixi install
pixi run desktop
```

If you use standard Cargo and npm:
```bash
# Build backend crates
cargo build

# Install frontend dependencies
cd frontend
npm install
cd ..

# Run desktop app in development mode
cargo tauri dev
```

### Running Tests

Run the test suites across the repository:

```bash
# Run frontend unit and component tests
cd frontend && npm test && cd ..

# Run backend desktop tests
cargo test -p arbitrageur-desktop

# Run exchange adapter tests
cargo test -p arbitrageur-exchanges

# Check code formatting and run clippy
cargo fmt --check
cargo clippy --workspace --all-targets
```

### Building for Release

Installers are produced by `tauri build`, which runs the frontend build,
compiles the desktop crate with `custom-protocol`, and packages every format
the host OS supports:

| Host | Output |
|---|---|
| Linux | `.AppImage`, `.deb`, `.rpm` |
| macOS | `.dmg` |
| Windows | NSIS `.exe` (`.msi` if enabled) |

```bash
cd frontend
npm ci
npm run tauri build
# artifacts: frontend/src-tauri/target/release/bundle/
```

To produce only an optimized desktop binary without packaging, build with the
`custom-protocol` feature so that the webview loads compiled frontend assets
directly:

```bash
cargo build --release --bin arbitrageur-desktop --features custom-protocol
```

---

## References and Background

The design incorporates lessons learned from examining several open-source trading bots and quantitative packages:

- `lihanyu81/PopDEX---Lighter-Arbitrage-Tool-From-PandaZhai`: Step-based reconciliation and rate-limited API coordination.
- `taoli-tools/taoli-tools-signer`: Isolated signer patterns and router allowlists.
- `cryptocj520/bphltaoli`: Multi-exchange funding rate tracking and depth ratio verification.
- `cryptocj520/crypto-trading-open`: Adapter design patterns and multi-leg order mapping.
- `hudson-and-thames/arbitragelab`: Implementation references for Ornstein-Uhlenbeck fitting and Kalman filtering.
- `realfishsam/prediction-market-arbitrage-bot`: Contract pair matching concepts.

---

## License

This project is dual-licensed under:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

Choose whichever license best fits your needs.
