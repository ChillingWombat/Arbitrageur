# System Specification: Next-Generation Cross-Market Arbitrage Engine & Terminal (V2)

## Problem Statement

Quantitative traders and institutional market participants operating across decentralized perpetual exchanges (Hyperliquid, Lighter, Vanta, Robinhood Arbitrum) and prediction markets face fragmented liquidity, severe execution friction, and visual clutter in existing interfaces:

1. **Header Redundancy and Clutter**: Previous layouts separated the active asset identity from quantitative strategy parameters across stacked horizontal bars, duplicating critical figures like the nominal hurdle rate and consuming excessive vertical viewport space on trading screens.
2. **Global vs Asset-Specific Window Mismatch**: Placing a global sliding window switcher directly on the main monitoring ribbon forced a single time horizon across all trading pairs, whereas high-volatility pairs (e.g. meme tokens) require tighter rolling windows (10s/30s) while deep liquidity assets (e.g. BTC, ETH) necessitate 1m, 5m, or 15m baselines.
3. **Security Vulnerability on Exchange Credential Inspection**: Exposing exchange private keys, API secrets, and account indices whenever the settings panel was viewed without strict timeout protection created shoulder-surfing and unattended session risks. Furthermore, banner cards took up excessive vertical real estate.
4. **Synthetic Data Contamination**: Many open-source arbitrage tools fabricate synthetic quotes or fake liquidity when credentials are missing, causing catastrophic slippage and liquidation when deployed in live environments. Traders need a strict fail-closed guarantee where paper simulation operates exclusively against live, authentic orderbooks.
5. **Awkward Navigation Flow**: In configuration settings, exchanges were presented prior to general execution risk guardrails, while trading mode indicators were lost in top corners rather than anchored near action controls.

---

## Solution

A modular, high-performance cross-market arbitrage engine and desktop terminal built in Rust (backend core) and React/Tauri (frontend), designed with a minimalist light-and-blue interface (inspired by ddeng.me) that delivers institutional-grade security, mathematical rigor, and flawless ergonomic clarity:

1. **Unified Ticker & Strategy Header Ribbon**: Merges asset identity, top-of-book quotes, top dislocation opportunity, trade count, net PnL, and quantitative strategy parameters (Target Profit, Hurdle Rate, Sliding Window, Execution Mode, Grid Units, and Safe Leverage) into a single horizontal ribbon with zero duplicate information.
2. **Per-Ticker Strategy Configurator with Sliding Window**: Relocates the rolling sliding window selector (10s, 30s, 1m, 5m, 15m) into the per-asset strategy modal, persisting duration alongside target profit BPS, window percentiles, grid sizing, and leverage.
3. **Full-Cover Vault Lockout & 60-Second Auto-Lock**: Replaces bulky vault banner cards with an automated security overlay. When the vault is locked, the API exchange credentials page is completely greyed out and blurred behind a backdrop, presenting a compact, centered password entry card. Exiting the settings modal activates an automatic 60-second lock timer, and closing or exiting the application instantly locks the in-memory AES-256-GCM vault.
4. **Fail-Closed Zero Synthetic Data Architecture**: If credentials are not configured or invalid, the venue adapter strictly reports disconnected and returns zero synthetic quotes, preventing phantom dislocations.
5. **Ergonomic Settings Flow**: Reorders settings navigation tabs with "Execution" as the primary view, eliminates redundant tags and venue subtitles, and repositions the Trading Mode indicator to the bottom-left anchor bar.

---

## User Stories

### Ticker & Strategy Header Ribbon
1. As an arbitrageur, I want a single unified horizontal header ribbon in the matrix view, so that I can see all asset information and active strategy parameters without wasting vertical screen space.
2. As a trader, I want to see the active symbol and PERP tag clearly highlighted, so that I immediately know which contract is active.
3. As a quantitative trader, I want to see the top-of-book mid price and book spread updated in real-time, so that I understand current market liquidity before executing.
4. As an algorithmic trader, I want a top dislocation chip displaying the highest net BPS spread and venue route, so that I can immediately identify the most lucrative opportunity.
5. As an execution trader, I want to click on the top dislocation chip to auto-select the corresponding matrix cell and dock the order ticket, so that I can execute trades with minimal latency.
6. As a portfolio manager, I want to see the total number of executions and net realized PnL for the active ticker, so that I can track strategy profitability over time.
7. As a risk manager, I want the nominal hurdle rate displayed only once in the header, so that I am not distracted by redundant information.
8. As a strategy operator, I want to see the active target profit in basis points, so that I know the minimum profit margin required for execution.
9. As an algorithmic trader, I want to see the active rolling sliding window and percentile threshold in the ribbon, so that I understand the statistical baseline being applied.
10. As a quantitative researcher, I want to see the current execution mode (Single-Way divergence vs Double-Way convergence), so that I know whether position unwinds will be triggered.
11. As an operator, I want to see the current accumulated grid units versus the maximum allowable units, so that I can prevent overexposure.
12. As a capital allocator, I want to see the effective safe leverage and required USD margin per unit, so that I know the portfolio's margin efficiency and liquidation buffer.
13. As an automated trader, I want an Auto Bot toggle switch in the ribbon, so that I can activate and deactivate automated scanning and execution for that ticker.
14. As an operator, I want a "Configurator" button in the ribbon, so that I can open the quantitative parameter modal with a single click.
    (2026-09-18: renamed from "Configure Strategy"; all seven strategy ribbons use "Configurator" with an icon–text gap.)

### Sliding Window & Strategy Configuration
15. As a trader, I want to configure the rolling sliding window duration (10s, 30s, 1m, 5m, 15m) inside the strategy configurator, so that each trading pair can use its optimal time baseline.
16. As a high-frequency trader, I want to set a 10-second or 30-second window on volatile assets, so that the engine rapidly detects ephemeral price dislocations.
17. As an institutional trader, I want to set a 5-minute or 15-minute window on major assets (BTC, ETH), so that temporary orderbook flickers do not generate false triggers.
18. As a quantitative analyst, I want the sliding window setting to persist in the backend database per ticker, so that my choices remain intact between application restarts.
19. As a trader, I want presets (Conservative, Balanced, Aggressive) in the configurator that set target profit, percentile, window, grid, and leverage in one click, so that I can rapidly deploy standard parameters.
20. As a risk analyst, I want the configurator to calculate the exact nominal hurdle breakdown (fees + closing friction + target profit), so that I know the minimum gross spread required for positive expectancy.

### Security, Vault Lockout & Auto-Lock
21. As a security-conscious trader, I want the exchange API credentials page to be completely greyed out and blurred when the vault is locked, so that sensitive keys are never visible to bystanders.
22. As a user, I want a clean, centered white prompt card asking for the password when the vault is locked, so that I can unlock it effortlessly without leaving the page.
23. As a user, I want to press Enter in the password input to immediately unlock the vault, so that my workflow remains keyboard-friendly.
24. As an institutional operator, I want the vault to automatically re-lock 60 seconds after closing the settings modal, so that credentials do not remain exposed in memory if I step away.
25. As a security architect, I want the vault to automatically lock upon application exit or window unload, so that no decrypted secrets persist on shutdown.
26. As an operator, I want the top-right vault indicator removed from the settings header, so that the navigation bar remains clean and uncluttered.
27. As a trader, I want a manual "Lock Vault" button available on the exchange page when unlocked, so that I can re-lock sensitive credentials immediately after viewing.
28. As a developer, I want all sensitive keys stored locally using AES-256-GCM encryption with Argon2 key derivation, so that credentials never leak to git or external servers.

### Navigation, Ergonomics & Information Architecture
29. As a user, I want the "Execution" tab to appear first in Settings and be the default active tab, so that I can review risk parameters before exchange credentials.
30. As a user, I want navigation tabs to be divided by clean vertical borders with blue active indicator bars, so that tab states are unmistakable.
31. As a user, I want exchange subtitles (e.g. "CLOB DEX", "L1 Perp DEX") removed from the exchange selector list, so that the list is clean and scannable.
32. As a user, I want the redundant "{activeVenuesCount} / 9 Venues Enabled" pill removed from the exchange subheader, so that the interface does not repeat information already present on the tab badge.
33. As an operator, I want the Trading Mode indicator (Mode: PAPER / LIVE) relocated to the bottom-left of the settings page next to the action buttons, so that it anchors the entire execution state.
34. As a user, I want info tooltips (? icon) to expand downwards below the icon, so that tooltips placed in top headers or modal titles never clip off the screen.

### Real Execution & Zero Synthetic Data
35. As an institutional trader, I want a strict fail-closed guarantee where no synthetic orderbooks or prices are fabricated, so that paper trading executes solely against authentic exchange depth.
36. As an API user, I want unconfigured exchanges to cleanly report disconnected status, so that the strategy engine does not calculate phantom arbitrage opportunities.
37. As a multi-exchange arbitrageur, I want support for Hyperliquid, Lighter, Vanta (Orderly), Robinhood Arbitrum, Arcus, PopDEX, Polymarket, and Kalshi, so that I can execute across all major venues.
38. As an operator, I want local untracked credentials (e.g. in `keychain.toml`) to load seamlessly without ever being tracked in version control, so that private keys remain private.

### Multi-Ticker Funding Rate Arbitrage Monitor
39. As a quantitative carry trader, I want the Funding Rate tab to continuously monitor and rank delta-neutral carry opportunities across all shared, non-hidden tickers simultaneously, rather than being locked to the single spread strategy's active ticker.
40. As an operator, I want each ranked opportunity card to clearly display its contract symbol (e.g. `[BTC-USDC]`, `[ETH-USDC]`), so that I know exactly which instrument is targeted.
41. As a trader, I want in-tab asset filter pills (`ALL`, `BTC`, `ETH`, `SOL`, etc.) above the carry cards, so that I can focus on opportunities for a specific asset with one click.
42. As a carry trader, I want clicking any opportunity card to select and project it in the Carry Yield Amortization Simulator, so that I can visualize break-even hours, trading fees, and projected net carry over 1 to 90 days.
43. As an automated trader, I want the Funding Carry bot lifecycle controls to operate strategy-wide (`strategyId: "funding"`), executing the highest net APY opportunity meeting minimum APR across all monitored tickers.

### Strategy Auto-Filters: Exchange 24h Volume & Price Resolution
44. As a risk manager, I want the `Auto Min 24h Volume` filter to evaluate authentic 24h trading turnover reported by the exchanges (e.g. Hyperliquid `dayNtlVlm`, Lighter `daily_quote_token_volume`, Binance `quoteVolume`), rather than internal system execution samples.
45. As an operator, I want `Auto Min 24h Volume` to aggregate across venues that actively report volume (`min_reported >= floor`), so that an exchange adapter lacking a volume endpoint or experiencing temporary rate-limits does not disqualify high-volume assets like BTC or ETH.
46. As an algorithmic trader, I want `Auto Min Mid Price` to evaluate against a 3-tier price hierarchy (live orderbook mid -> exchange mark price -> benchmark catalog price), so that background tickers not currently polled in the hub orderbook cache are not mistakenly hidden.
47. As a trader, I want setting `Auto Min Mid Price = 0.5` to automatically hide sub-$0.50 meme tokens (e.g. PEPE, BONK, SHIB, DOGE) while keeping major assets (BTC $78k+, ETH $2.4k+, SOL $100+) visible and tradable.
48. As a system operator, I want math operations calculating funding APR, APY, and daily income to use checked decimal arithmetic, preventing multiplication overflows on extreme values or large trade sizes.

### Modern DEX Account Credentials & Copilot
49. As a Decibel trader, I want Aptos Ed25519 accounts (`account_address` + `ed25519_private_key`) to be recognized by `has_credentials`, so that saved Decibel credentials display as configured rather than "no credential configured".
    STATUS 2026-09-18 audit: NOT IMPLEMENTED — no `account_address`/`ed25519_private_key` handling exists in the tree (`DecibelAdapter` in `crates/arbitrageur-exchanges/src/decibel.rs` keys on api_key/wallet_address/private_key; no `credentials.rs` file exists). The 2026-09-18 session-16 handoff entry claiming this fix is incorrect (see handoff errata).
50. As an AI Copilot user, I want duplicate Gemini API entries removed from settings, so that AI assistant configuration is clean and unambiguous.
    STATUS 2026-09-18 audit: NOT APPLICABLE — no Copilot settings surface exists anywhere in this tree (no `CopilotSettingsModal.tsx`, zero `Copilot`/`Gemini` references in frontend or backend). The 2026-09-18 session-16 handoff entry appears to describe a different project (see handoff errata).

---

## Implementation Decisions

### Architectural Seams
The system maintains a single primary seam between the Rust Core Engine (responsible for secure key vaulting, exchange WebSocket/REST connectivity, quantitative matrix math, and atomic execution coordination) and the React/Tauri Presentation Layer. Communication occurs strictly via typed Tauri IPC commands and events.

### State & Type Shapes
The `TickerStrategyConfigDto` interface in both Rust and TypeScript is extended with a rolling sliding window duration:

```typescript
export type SlidingWindowOption = "10s" | "30s" | "1m" | "5m" | "15m";

export interface TickerStrategyConfigDto {
  symbol: string;
  targetProfitBps: number;
  windowPercentile: number;
  maxAccumulatedUnits: number;
  currentAccumulatedUnits: number;
  executionMode: "SINGLE_WAY" | "DOUBLE_WAY";
  unitSize: number;
  userTargetLeverage: number;
  slidingWindow?: SlidingWindowOption;
  sliding_window?: string;
}
```

In the Rust state definition:
```rust
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TickerStrategyConfigDto {
    pub symbol: String,
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    pub max_accumulated_units: u32,
    pub current_accumulated_units: u32,
    pub execution_mode: String,
    pub unit_size: f64,
    pub user_target_leverage: u32,
    #[serde(default = "default_sliding_window")]
    pub sliding_window: String,
}
```

### Vault Security & Lifecycle State Machine
1. **Initial / Locked State**: `vaultStatus.is_unlocked == false`. The exchange credentials panel renders a full-height overlay with CSS `backdrop-blur-xs` and a centered white unlock card.
2. **Unlock Flow**: User enters password -> invokes `api.unlockVault(password)` -> sets in-memory encryption key -> returns `is_unlocked: true` -> overlay unmounts -> credentials render.
3. **Auto-Lock Timeout**: Closing the settings modal starts a 60-second countdown (`setTimeout(60000)`). If the user reopens settings before expiration, the timeout is cleared. If the timer fires, `api.lockVault()` is called, immediately purging decrypted keys from memory.
4. **App Termination**: A `beforeunload` event handler invokes `api.lockVault()` synchronously or via fire-and-forget, guaranteeing that memory is wiped when the process ends.

### Multi-Ticker Funding Rate Radar & Execution Engine
1. **Decoupled Architecture**: `FundingRadar.tsx` does not bind to the single spread strategy's `selectedSymbol`. Instead, it accepts `availableTickers` and periodically scans all shared non-hidden tickers simultaneously.
2. **Batch IPC Scanning**:
   - `scan_funding_opportunities` accepts `symbol: Option<String>`, `symbols: Option<Vec<String>>`, and `trade_size_usd: f64`.
   - `scan_funding_opportunities_batch_impl` iterates across target tickers, queries cached funding rates from `MarketDataHub`, aggregates cross-venue carry permutations via `evaluate_funding_carry`, and sorts descending by `net_apy_pct`.
   - If an empty symbol or no symbols are provided, it automatically discovers all shared markets across $\ge 2$ active venues.
3. **In-Tab Filter & Amortization Projection**:
   - In-tab asset filter tabs (`ALL`, `BTC`, `ETH`, `SOL`, etc.) allow filtering opportunity cards.
   - Clicking an opportunity highlights the card and sets it as the active selection for the interactive Amortization Simulator, dynamically projecting gross funding collected, trading fees, and net yield across 1, 14, 30, 60, and 90-day holding periods.
4. **Strategy-Wide Bot Lifecycle**:
   - Automated funding carry trading runs via `useBotLifecycle({ strategyId: "funding" })` at the strategy level, evaluating the highest-yielding opportunity meeting the `minAprPct` hurdle.
5. **Same-Exchange Spot/Perp Basis (Cash-and-Carry)**:
   - The scan additionally emits per-venue `BASIS` opportunities (long spot + short perp on the SAME exchange) via `evaluate_spot_perp_basis`; reverse (long perp / short spot) needs borrow and is never emitted.
   - Each basis opp carries signed live divergence `basis_bps = (perp - spot) / spot * 10000`, spot/perp reference mids, and the same depth-aware capacity walk (spot asks vs perp bids) and entry-cross friction as carry.
   - Venues without spot books (Hyperliquid/Vanta refuse spot) fail closed — no fabricated basis. Gated by `funding_enable_basis` (default ON) + optional `funding_min_basis_bps` floor; the APR hurdle still applies downstream.
6. **Informational Win Probability (Carry + Basis)**:
   - `funding_win_probability` scores `coverage / (1 + coverage)` with `coverage = 24h carry / round-trip fee` (memoryless 24h-hold assumption: 4h breakeven ~0.86, 24h ~0.50, 72h ~0.25), lifted slightly by predicted-rate agreement and positive entry basis, clamped to [0.02, 0.98].
   - Experimental heuristic, displayed with "—" fallback; NEVER gates ordering or executability (same contract as the spread monitor's empirical field). An empirical upgrade is unblocked by the funding history below.
7. **Funding-Rate History Log (schema v18)**:
   - `funding_rate_samples(venue_id, symbol, ts_ms[minute-bucketed], rate_8h, interval_hours)` records every scan's observed prints (REPLACE-idempotent); read via `funding_rate_series`, pruned by the shared retention pass. Training substrate for the deferred persistence / half-life estimator.
8. **Scan Perf**: per-venue perp books are snapshot-parsed once per scan (O(V) hub clones) instead of per carry pair (O(V^2)).
9. **Orderbook Gradient**: VWAP walks already price level-gaps into every capacity tier, and the new `book_gradient::entry_gradient` makes the MARGINAL cost explicit — bps per extra $1k summed over both legs (fixed fractional 10-step walk, level-lump independent), plus top-5 imbalance and microprice. Reported per funding/basis opp as `depth_slope_bps_per_1k` (None when thin — never a fabricated zero). Still point-in-time: no replenishment, impact beyond the visible book, or time decay.

### Strategy Auto-Filters & Price Resolution Architecture
1. **Real Exchange 24h Volume Aggregation**:
   - Queries authentic exchange 24h quote volume endpoints: Hyperliquid `dayNtlVlm`, Lighter `daily_quote_token_volume`, Vanta `marketSummary` (`24h_amount`), Binance `quoteVolume`, OKX `volCcy24h`, and Kraken `vol24h`.
   - Evaluates the minimum volume across venues that **actively report volume** (`min_reported >= floor`). Venues lacking volume endpoints (or experiencing temporary rate-limits) contribute nothing to aggregates without failing closed or locking out high-volume assets.
2. **3-Tier Price Resolution**:
   - Resolves mid prices using a strict 3-tier hierarchy:
     1. Live orderbook mid from `market_data_hub` (`hub_mid_usd`).
     2. Cached exchange mark/last price from 24h market feeds.
     3. Authoritative benchmark catalog price (`benchmark_base_price` in Rust, `getTickerBenchmarkPrice` in TypeScript).
   - This ensures background tickers not currently polled in the hub orderbook cache are evaluated against their true market price rather than being blocked as missing data.
   - Setting `Auto Min Mid Price = 0.5` reliably filters sub-$0.50 meme coins (PEPE, BONK, SHIB, DOGE) while keeping major assets (BTC, ETH, SOL) visible.
3. **Safe Decimal Arithmetic**:
   - `evaluate_funding_carry` in `arbitrageur-math` utilizes `checked_mul` and `checked_div` for funding APR, APY, and daily income calculations, eliminating panic risks on extreme inputs or stress test values.

---

## Testing Decisions

### What Makes a Good Test
- Tests must verify external behavioral contracts, not internal component implementation details.
- Unit tests verify pure quantitative functions: minimum compatible lot sizing, safe leverage calculation, sliding window delta computation, and hurdle rate calculation.
- IPC command integration tests verify that unconfigured environments return empty quote vectors (fail-closed) and that saving and loading strategy configs preserves all parameters.

### Modules Tested
1. **Tauri Engine Commands** (`commands.rs`):
   - `test_ticker_strategy_config_crud_and_validation`: Verifies saving, retrieving, and validating strategy configuration with sliding window parameters.
   - `test_unconfigured_state_returns_zero_synthetic_data`: Verifies that unconfigured venues return zero synthetic orderbooks or fabricated prices.
   - `test_vault_initialize_unlock_lock`: Verifies encryption, password verification, memory lock, and status reporting.
2. **Frontend Quant & Service Layer** (`matrixQuant.ts`, `contractSpecs.ts`, `tickers.ts`):
   - Contract spec calculation for lot sizes and leverage safety across Hyperliquid, Lighter, Vanta, and Robinhood.
   - 2D matrix calculation incorporating taker fees, closing spread friction, and rolling sliding window delta.
   - Filtering of available tickers based on 2+ active, connected venues.

---

## Out of Scope
- Direct custody of funds: Arbitrageur connects to smart contracts and exchange APIs via user-owned keys; it does not operate custodial wallets.
- Centralized server syncing: All configurations, vaults, and execution logs remain purely local on the user's workstation.
- Automated token approvals: ERC-20 approvals must be performed via standard wallet signatures prior to swap routing.

---

## Further Notes
- The minimalist blue and light theme follows the design system derived from [ddeng.me](https://ddeng.me/), featuring high-contrast slate typography (`#0f172a`), subtle slate borders (`#e2e8f0`), and vibrant brand accent blue (`#2563eb`).
- Private keys and testing credentials must never be committed to git repositories. All local testing keys are preserved solely in git-ignored local storage and `keychain.toml`.
