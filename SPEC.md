# Arbitrageur System Specification (merged)
> Merged 2026-09-19 from the five SPEC_*.md files.
>

> Session 41 (2026-09-20): settings save UX — saved-success banner removed (button shows transient `Succeed!`), `Save Configuration`→`Save Changes` (fixed `w-36`), `System Setting`→`System Settings` everywhere; button labels audited to Title Case (`Halt Trading`, `By Ticker/Venue`, `Via Wallet`).
>
> Session 40 (2026-09-20): Safety settings are now operator-controllable (System Setting → Safety tab: circuit-breaker halt limits, DeltaGuard exposure cap, margin thresholds; live-applied + kv-persisted). Breaker baseline re-anchors on paper↔live switches / paper resets instead of false-tripping (fixes recurring -$196k flat-book halt). Portfolio Monitor is the single reset control; Header/sidebar resets removed; CB-tripped vs halted wording unified to Halted / circuit breaker tripped. One spec, five parts — newest decisions win on conflict. Per-part status reflects the live tree; the session handoff log (`docs/HANDOFF_MASTER_SESSION_HISTORY.md`) is the day-to-day record.
>
> Session 73 (2026-09-20): the ticker universe is now discovered from live venue-reported perpetual markets; `PERP_TICKERS` is metadata/benchmark fallback only, not an allowlist. Exact `BASE-QUOTE` markets need ≥2 admitted reporting venues. Bulk venue mark prices seed dynamic sizing and auto-filter evaluation. USDC and USDT listings remain distinct instruments.
>
> Session 74–77 (2026-09-20 remediation): capability vocabulary is now separated into market/account reads, signing/submission, cancellation, order-status polling, terminal fill reconciliation, paper strategy support, and live strategy-entry readiness. All eleven live-submission adapters implement validated `fetch_order` polling and the desktop reconciliation supervisor re-scans working orders at startup and every 5s with monotonic, identity-checked ledger updates. Automated residual-leg resolution (opt-in `auto_resolve_residual_exposure`) and bounded cancel-retry are implemented; no adapter is live-entry ready because sandbox/canary evidence does not exist. Live flatten position reads resolve venue-native account symbols through adapter-owned normalization and fail closed on unknown/ambiguous mappings. `docs/CAPABILITIES.md` is the authoritative current-state matrix; older absolute claims in this file are historical requirements/intent.
## Parts & status
1. **Core engine (V2)** — ✅ Implemented for authenticated reads, market data, paper strategy paths, and terminal/restart order reconciliation — live submission exists at adapter transport level with opt-in residual auto-resolution and bounded cancel retry. **Live strategy entries are opt-in** (`live_entry_enabled` + per-leg notional cap, spatial two-leg path only) for minimal-size canary validation pending sandbox/canary evidence.
2. **Session-aware seasonality layer** — 🟡 Partially implemented — session profiles + `get_session_profile` IPC exist; full phased rollout pending.
3. **Spread Range Monitor (Opportunity Radar)** — ✅ Implemented as OpportunityRadar — note the display-vs-engine split: the Min Exp table floor lives on the gadget strip, not the Configurator.
4. **Terminal navigation safeguards** — ✅ Implemented — layout grid, portfolio monitor, settings nav, pause/continue, 5X leverage default.
5. **WebSocket feeds & retention** — ✅ Implemented at the desktop feed-supervisor layer for Hyperliquid, Lighter, Robinhood-Lighter, and Vanta — always-on recorder, retention settings + enforcement. `ExchangeAdapter::subscribe_orderbook` remains a trait-level stub; venues outside the supervised four use REST polling.

---

## Part 1 — Core engine (V2)

> Status: ✅ Implemented for research and paper strategy paths — adapter transport can submit on eleven venues and working orders are reconciled to terminal state by the startup+periodic supervisor, with opt-in residual auto-resolution and bounded cancel retry implemented; live spatial strategy entries are gated behind an explicit operator opt-in and per-leg notional cap for canary validation.

## System Specification: Next-Generation Cross-Market Arbitrage Engine & Terminal (V2)

### Problem Statement

Quantitative traders and institutional market participants operating across decentralized perpetual exchanges (Hyperliquid, Lighter, Vanta, Robinhood Arbitrum) and prediction markets face fragmented liquidity, severe execution friction, and visual clutter in existing interfaces:

1. **Header Redundancy and Clutter**: Previous layouts separated the active asset identity from quantitative strategy parameters across stacked horizontal bars, duplicating critical figures like the nominal hurdle rate and consuming excessive vertical viewport space on trading screens.
2. **Global vs Asset-Specific Window Mismatch**: Placing a global sliding window switcher directly on the main monitoring ribbon forced a single time horizon across all trading pairs, whereas high-volatility pairs (e.g. meme tokens) require tighter rolling windows (10s/30s) while deep liquidity assets (e.g. BTC, ETH) necessitate 1m, 5m, or 15m baselines.
3. **Security Vulnerability on Exchange Credential Inspection**: Exposing exchange private keys, API secrets, and account indices whenever the settings panel was viewed without strict timeout protection created shoulder-surfing and unattended session risks. Furthermore, banner cards took up excessive vertical real estate.
4. **Synthetic Data Contamination**: Many open-source arbitrage tools fabricate synthetic quotes or fake liquidity when credentials are missing, causing catastrophic slippage and liquidation when deployed in live environments. Traders need a strict fail-closed guarantee where paper simulation operates exclusively against live, authentic orderbooks.
5. **Awkward Navigation Flow**: In configuration settings, exchanges were presented prior to general execution risk guardrails, while trading mode indicators were lost in top corners rather than anchored near action controls.

---

### Solution

A modular, high-performance cross-market arbitrage engine and desktop terminal built in Rust (backend core) and React/Tauri (frontend), designed with a minimalist light-and-blue interface (inspired by ddeng.me) that delivers institutional-grade security, mathematical rigor, and flawless ergonomic clarity:

1. **Unified Ticker & Strategy Header Ribbon**: Merges asset identity, top-of-book quotes, top dislocation opportunity, trade count, net PnL, and quantitative strategy parameters (Target Profit, Hurdle Rate, Sliding Window, Execution Mode, Grid Units, and Safe Leverage) into a single horizontal ribbon with zero duplicate information.
2. **Per-Ticker Strategy Configurator with Sliding Window**: Relocates the rolling sliding window selector (10s, 30s, 1m, 5m, 15m) into the per-asset strategy modal, persisting duration alongside target profit BPS, window percentiles, grid sizing, and leverage.
3. **Full-Cover Vault Lockout & 60-Second Auto-Lock**: Replaces bulky vault banner cards with an automated security overlay. When the vault is locked, the API exchange credentials page is completely greyed out and blurred behind a backdrop, presenting a compact, centered password entry card. Exiting the settings modal activates an automatic 60-second lock timer, and closing or exiting the application instantly locks the in-memory AES-256-GCM vault.
4. **Fail-Closed Zero Synthetic Data Architecture**: If credentials are not configured or invalid, the venue adapter strictly reports disconnected and returns zero synthetic quotes, preventing phantom dislocations.
5. **Ergonomic Settings Flow**: Reorders settings navigation tabs with "Execution" as the primary view, eliminates redundant tags and venue subtitles, and repositions the Trading Mode indicator to the bottom-left anchor bar.

---

### User Stories

#### Ticker & Strategy Header Ribbon
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

#### Sliding Window & Strategy Configuration
15. As a trader, I want to configure the rolling sliding window duration (10s, 30s, 1m, 5m, 15m) inside the strategy configurator, so that each trading pair can use its optimal time baseline.
16. As a high-frequency trader, I want to set a 10-second or 30-second window on volatile assets, so that the engine rapidly detects ephemeral price dislocations.
17. As an institutional trader, I want to set a 5-minute or 15-minute window on major assets (BTC, ETH), so that temporary orderbook flickers do not generate false triggers.
18. As a quantitative analyst, I want the sliding window setting to persist in the backend database per ticker, so that my choices remain intact between application restarts.
19. As a trader, I want presets (Conservative, Balanced, Aggressive) in the configurator that set target profit, percentile, window, grid, and leverage in one click, so that I can rapidly deploy standard parameters.
    - **2026-09-20 status:** still open. Named risk presets are intentionally gated until the project has a versioned outcome corpus; fabricating tiers would violate the fail-closed/data-first mandate. `docs/PARAMETER_GUIDE.md` now provides the manual cold-start rules and current software defaults.
20. As a risk analyst, I want the configurator to calculate the exact nominal hurdle breakdown (fees + closing friction + target profit), so that I know the minimum gross spread required for positive expectancy.

#### Security, Vault Lockout & Auto-Lock
21. As a security-conscious trader, I want the exchange API credentials page to be completely greyed out and blurred when the vault is locked, so that sensitive keys are never visible to bystanders.
22. As a user, I want a clean, centered white prompt card asking for the password when the vault is locked, so that I can unlock it effortlessly without leaving the page.
23. As a user, I want to press Enter in the password input to immediately unlock the vault, so that my workflow remains keyboard-friendly.
24. As an institutional operator, I want the vault to automatically re-lock 60 seconds after closing the settings modal, so that credentials do not remain exposed in memory if I step away.
25. As a security architect, I want the vault to automatically lock upon application exit or window unload, so that no decrypted secrets persist on shutdown.
26. As an operator, I want the top-right vault indicator removed from the settings header, so that the navigation bar remains clean and uncluttered.
27. As a trader, I want a manual "Lock Vault" button available on the exchange page when unlocked, so that I can re-lock sensitive credentials immediately after viewing.
28. As a developer, I want all sensitive keys stored locally using AES-256-GCM encryption with Argon2 key derivation, so that credentials never leak to git or external servers.

#### Navigation, Ergonomics & Information Architecture
29. As a user, I want the "Execution" tab to appear first in Settings and be the default active tab, so that I can review risk parameters before exchange credentials.
30. As a user, I want navigation tabs to be divided by clean vertical borders with blue active indicator bars, so that tab states are unmistakable.
31. As a user, I want exchange subtitles (e.g. "CLOB DEX", "L1 Perp DEX") removed from the exchange selector list, so that the list is clean and scannable.
32. As a user, I want the redundant "{activeVenuesCount} / 9 Venues Enabled" pill removed from the exchange subheader, so that the interface does not repeat information already present on the tab badge.
33. As an operator, I want the Trading Mode indicator (Mode: PAPER / LIVE) relocated to the bottom-left of the settings page next to the action buttons, so that it anchors the entire execution state.
34. As a user, I want info tooltips (? icon) to expand downwards below the icon, so that tooltips placed in top headers or modal titles never clip off the screen.

#### Real Execution & Zero Synthetic Data
35. As an institutional trader, I want a strict fail-closed guarantee where no synthetic orderbooks or prices are fabricated, so that paper trading executes solely against authentic exchange depth.
36. As an API user, I want unconfigured exchanges to cleanly report disconnected status, so that the strategy engine does not calculate phantom arbitrage opportunities.
37. As a multi-exchange arbitrageur, I want support for Hyperliquid, Lighter, Vanta (Orderly), Robinhood Arbitrum, Arcus, PopDEX, Polymarket, and Kalshi, so that I can execute across all major venues.
    - **2026-09-20 status:** superseded by `docs/CAPABILITIES.md`. Venue support is path-specific (reads, submission, cancellation, paper execution, research scaffold); it does not imply universal execution or live strategy readiness.
38. As an operator, I want local untracked credentials (kept at `~/.config/arbitrageur/keychain.toml`, mode 0600 on a native filesystem — never on the shared NTFS mount where chmod is a no-op) to load seamlessly without ever being tracked in version control, so that private keys remain private.
    - **2026-09-20 status:** superseded by the encrypted vault design. Credentials now live in `~/.arbitrageur/vault.enc` (AES-256-GCM + Argon2id); reveal is field-scoped only and no whole-account credential-read IPC is registered.

#### Multi-Ticker Funding Rate Arbitrage Monitor
39. As a quantitative carry trader, I want the Funding Rate tab to continuously monitor and rank delta-neutral carry opportunities across all shared, non-hidden tickers simultaneously, rather than being locked to the single spread strategy's active ticker.
40. As an operator, I want each ranked opportunity card to clearly display its contract symbol (e.g. `[BTC-USDC]`, `[ETH-USDC]`), so that I know exactly which instrument is targeted.
41. As a trader, I want in-tab asset filter pills (`ALL`, `BTC`, `ETH`, `SOL`, etc.) above the carry cards, so that I can focus on opportunities for a specific asset with one click.
42. As a carry trader, I want clicking any opportunity card to select and project it in the Carry Yield Amortization Simulator, so that I can visualize break-even hours, trading fees, and projected net carry over 1 to 90 days.
43. As an automated trader, I want the Funding Carry bot lifecycle controls to operate strategy-wide (`strategyId: "funding"`), executing the highest net APY opportunity meeting minimum APR across all monitored tickers.

#### Strategy Auto-Filters: Exchange 24h Volume & Price Resolution
44. As a risk manager, I want the `Auto Min 24h Volume` filter to evaluate authentic 24h trading turnover reported by the exchanges (e.g. Hyperliquid `dayNtlVlm`, Lighter `daily_quote_token_volume`, Binance `quoteVolume`), rather than internal system execution samples.
45. As an operator, I want `Auto Min 24h Volume` to aggregate across venues that actively report volume (`min_reported >= floor`), so that an exchange adapter lacking a volume endpoint or experiencing temporary rate-limits does not disqualify high-volume assets like BTC or ETH.
46. As an algorithmic trader, I want `Auto Min Mid Price` to evaluate against a 3-tier price hierarchy (live orderbook mid -> exchange mark price -> benchmark catalog price), so that background tickers not currently polled in the hub orderbook cache are not mistakenly hidden.
47. As a trader, I want setting `Auto Min Mid Price = 0.5` to automatically hide sub-$0.50 meme tokens (e.g. PEPE, BONK, SHIB, DOGE) while keeping major assets (BTC $78k+, ETH $2.4k+, SOL $100+) visible and tradable.
48. As a system operator, I want math operations calculating funding APR, APY, and daily income to use checked decimal arithmetic, preventing multiplication overflows on extreme values or large trade sizes.

#### Modern DEX Account Credentials & Copilot
49. As a Decibel trader, I want Aptos Ed25519 accounts (`account_address` + `ed25519_private_key`) to be recognized by `has_credentials`, so that saved Decibel credentials display as configured rather than "no credential configured".
    STATUS 2026-09-18 audit: NOT IMPLEMENTED — no `account_address`/`ed25519_private_key` handling exists in the tree (`DecibelAdapter` in `crates/arbitrageur-exchanges/src/decibel.rs` keys on api_key/wallet_address/private_key; no `credentials.rs` file exists). The 2026-09-18 session-16 handoff entry claiming this fix is incorrect (see handoff errata).
50. As an AI Copilot user, I want duplicate Gemini API entries removed from settings, so that AI assistant configuration is clean and unambiguous.
    STATUS 2026-09-18 audit: NOT APPLICABLE — no Copilot settings surface exists anywhere in this tree (no `CopilotSettingsModal.tsx`, zero `Copilot`/`Gemini` references in frontend or backend). The 2026-09-18 session-16 handoff entry appears to describe a different project (see handoff errata).

#### Dynamic Ticker Universe
51. As a multi-venue trader, I want the ticker list to be discovered from live venue-reported perpetual markets rather than a hardcoded allowlist, so that newly listed or previously omitted instruments such as `ZEC-USDC` and `MSTR-USDT` appear without a code change.
52. As an operator, I want the static `PERP_TICKERS` catalog to remain available only as display metadata and benchmark-price fallback, so that a stale catalog can never hide a venue market that is actually live and executable.
53. As a risk manager, I want exact `BASE-QUOTE` identity preserved across venues, so that `MSTR-USDC` and `MSTR-USDT` are never silently merged into one instrument without an explicit cross-stablecoin basis model.

---

### Implementation Decisions

#### Architectural Seams
The system maintains a single primary seam between the Rust Core Engine (responsible for secure key vaulting, exchange WebSocket/REST connectivity, quantitative matrix math, and atomic execution coordination) and the React/Tauri Presentation Layer. Communication occurs strictly via typed Tauri IPC commands and events.

#### State & Type Shapes
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

#### Security Hardening (2026-09-19 audit mitigations)
1. **Field-scoped credential reveal**: the account card never pulls the whole secret bundle into the renderer. `get_account_credential_field` returns one whitelisted field per explicit eye-icon reveal (vault-gated, fail-closed); revealed values are dropped on save, lock, or account switch, with presence-flag placeholders (`hasApiKey`, `hasApiSecret`, …) covering the rest.
2. **Zeroizing secret storage**: every venue adapter holds API keys, secrets, passphrases, and private keys in `RwLock<Option<Zeroizing<String>>>` (wiped on drop); only unavoidable transient copies at signing/header boundaries remain, each marked in-source.
3. **Plaintext key hygiene**: no live keys under the repo tree; local test keys live only at `~/.config/arbitrageur/keychain.toml` (0600). Import into the vault UI, then shred the file and rotate exposed API keys.
4. **Headless bind guard**: the placeholder daemon refuses non-loopback binds unless `ARBITRAGEUR_ALLOW_LAN=1` is explicitly set.
5. **Display-vs-engine split**: settings that only change what a gadget shows (e.g. the Spread Radar table's Min Exp floor) live on the gadget filter strip, never in a strategy Configurator. Engine gates (qualification floors, entry hurdles, scan-universe visibility filters, routing, risk/limits) stay in the Configurator.

#### Vault Security & Lifecycle State Machine
1. **Initial / Locked State**: `vaultStatus.is_unlocked == false`. The exchange credentials panel renders a full-height overlay with CSS `backdrop-blur-xs` and a centered white unlock card.
2. **Unlock Flow**: User enters password -> invokes `api.unlockVault(password)` -> sets in-memory encryption key -> returns `is_unlocked: true` -> overlay unmounts -> credentials render.
3. **Auto-Lock Timeout**: Closing the settings modal starts a 60-second countdown (`setTimeout(60000)`). If the user reopens settings before expiration, the timeout is cleared. If the timer fires, `api.lockVault()` is called, immediately purging decrypted keys from memory.
4. **App Termination**: A `beforeunload` event handler invokes `api.lockVault()` synchronously or via fire-and-forget, guaranteeing that memory is wiped when the process ends.

#### Multi-Ticker Funding Rate Radar & Execution Engine
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
10. **Current Position Gadget**: third funding-dock tab (left of Execution
    History) bound to `get_open_positions` filtered to strategy "funding";
    popup shows legs/notional/fees/unrealized/signals + SVG net-PnL trend
    (solid realized, dashed illustrative projection only with backend
    expectedNetUsd). Popup APY is labelled Compounded APY; capacity (not
    trade size) is the headline size.
9. **Orderbook Gradient**: VWAP walks already price level-gaps into every capacity tier, and the new `book_gradient::entry_gradient` makes the MARGINAL cost explicit — bps per extra $1k summed over both legs (fixed fractional 10-step walk, level-lump independent), plus top-5 imbalance and microprice. Reported per funding/basis opp as `depth_slope_bps_per_1k` (None when thin — never a fabricated zero). Still point-in-time: no replenishment, impact beyond the visible book, or time decay.

#### Dynamic Market Universe & Reference Prices (2026-09-20)
1. **Venue-Discovered Universe**:
   - `get_market_universe` fans out to every admitted adapter and returns active `MarketType::Perpetual` symbols as canonical `BASE-QUOTE`.
   - Frontend `applyMarketUniverse(catalog, universe)` builds the union of venue-reported symbols; the static `PERP_TICKERS` rows are used only to enrich known names and as a fallback benchmark price.
   - A symbol is admitted when the **exact** canonical symbol is reported by at least two admitted venues. The catalog can no longer subtract or gate a live venue market.
2. **Exact Quote Semantics**:
   - `MSTR-USDC`, `MSTR-USDT`, and other quote variants remain separate instruments. Cross-stablecoin merging requires an explicit basis/FX model and per-venue symbol routing; it is never inferred from a shared base ticker.
   - Broker-suffixed Orderly markets such as `PERP_MSTR_USDC_mythos` remain rejected unless an adapter can prove the canonical alias and route orders to the exact venue symbol.
3. **Bulk Reference-Price Seeding**:
   - New `ExchangeAdapter::fetch_market_prices()` returns a best-effort map keyed by `MarketMetadata::venue_symbol`; it is not an execution price and missing entries are never fabricated.
   - Implemented bulk mark/last-price feeds: Hyperliquid `metaAndAssetCtxs.markPx`, Lighter `orderBookDetails.mark_price`, Vanta `marketSummary.mark_price`, Binance `premiumIndex.markPrice`, OKX `mark-price.markPx`, Aster `premiumIndex.markPrice`, Kraken `tickers.markPrice` (last-price fallback).
   - The frontend uses the median positive venue price for `PerpTicker.basePrice`, falling back to the static benchmark only when no live price is available.
4. **Auto-Filter Integration**:
   - The strategy filter's price resolution now consumes the same bulk venue prices: live hub mid → venue mark price → benchmark catalog fallback.
   - Hub lookup probes `BASE`, `BASE-USDC`, `BASE-USDT`, and `BASE-USD`, so dynamically discovered USDT-quoted markets are not blocked solely because the legacy catalog was USDC-centric.

#### Strategy Auto-Filters & Price Resolution Architecture
1. **Real Exchange 24h Volume Aggregation**:
   - Queries authentic exchange 24h quote volume endpoints: Hyperliquid `dayNtlVlm`, Lighter `daily_quote_token_volume`, Vanta `marketSummary` (`24h_amount`), Binance `quoteVolume`, OKX `volCcy24h`, and Kraken `vol24h`.
   - Evaluates the minimum volume across venues that **actively report volume** (`min_reported >= floor`). Venues lacking volume endpoints (or experiencing temporary rate-limits) contribute nothing to aggregates without failing closed or locking out high-volume assets.
2. **3-Tier Price Resolution**:
   - Resolves mid prices using a strict 3-tier hierarchy:
     1. Live orderbook mid from `market_data_hub` (`hub_mid_usd`; `BASE`, `BASE-USDC`, `BASE-USDT`, `BASE-USD`).
     2. Bulk venue mark/last price from `ExchangeAdapter::fetch_market_prices()`.
     3. Authoritative benchmark catalog price (`benchmark_base_price` in Rust, `getTickerBenchmarkPrice` in TypeScript).
   - This ensures background tickers not currently polled in the hub orderbook cache are evaluated against their true market price rather than being blocked as missing data.
   - Setting `Auto Min Mid Price = 0.5` reliably filters sub-$0.50 meme coins (PEPE, BONK, SHIB, DOGE) while keeping major assets (BTC, ETH, SOL) visible.
3. **Safe Decimal Arithmetic**:
   - `evaluate_funding_carry` in `arbitrageur-math` utilizes `checked_mul` and `checked_div` for funding APR, APY, and daily income calculations, eliminating panic risks on extreme inputs or stress test values.
4. **Lighter 8-Hour Funding Prints (2026-09-20 fix)**:
   - Lighter `/api/v1/funding-rates` `rate` is an **8-hour aggregated print** (tracks the venue UI "8hr" figure; hourly ~= rate / 8 — verified: LINK venue print -0.0336% vs UI 1hr -0.0039%). Adapters record `interval_hours = 8`; mislabeling it as hourly inflated every Lighter-legged carry quote ~8x (LINK showed 277% APR / 1499% APY instead of ~45-60%) and faked identical +0.067%/8h rows across all majors (both venues' placeholder floors netting to dust). Same fix covers the Robinhood deployment (shares the Lighter parser).
   - One-time backfill: schema v20 rescales pre-fix `lighter`/`robinhood` rows (`rate_8h / 8`, `interval_hours = 8`); Hyperliquid hourly semantics confirmed against its UI (0.0013%/h) and left unchanged.
9. **Live Submission Rollout (2026-09-20, bounded)**:
   - Binance USDT-M: place/cancel/leverage over signed `/fapi` (HMAC-SHA256, venue-native contract units incl. scaled `1000PEPE` symbols, 5 BPS taker accounting) + HMAC/sign/parse unit tests. OKX swaps: place/cancel/leverage over signed `/api/v5` (base64 HMAC preimage, contract-size conversion via `ctVal`, fill polling) + sign/contracts/parse tests. Vault credential plumbing for both already existed. 2026-09-20 update: Lighter live submission is IMPLEMENTED (vendored `liblighter_signer.so` Go signer, nextNonce → Sign → sendTx, `live_order_submission_implemented()=true` incl. Robinhood deployment; vault `api_key_index` plumbed through `state.rs`). Remaining: Arcus (no public API exists — stays fail-closed by design); then the reconciled two-leg live dispatcher. Decibel live submission IMPLEMENTED 2026-09-20 (aptos-sdk entry-function tx, mainnet vectors verified); Decibel reads (balances/positions/funding) rebuilt same day on official /api/v1 paths (book WS-only upstream). Later 2026-09-20: execution failures recorded to a persisted (v21) Log Monitor feed on all six execute commands; funding scanner gained a Venue/Min-Exp/Min-Win strip (defaults mirror bot thresholds) + decay-aware Expected $ replacing flat Daily; Win% hold-aligned with Expected; US100==TECH100 merged to NAS100; Risk Control tab (subcategories, equal tabs, global max leverage 10x ceiling).
   - Current boundary: adapter submission exists and every live-submission venue implements `fetch_order` order-status polling reconciled by the startup+5s supervisor, and automated residual-leg resolution plus bounded cancel retry exist behind an opt-in flag. Live mode defaults to authenticated account reads and explicit risk-reducing closes; spatial two-leg entries are admitted only when the operator enables `live_entry_enabled` with a per-leg notional cap — the canary surface for venue evidence. `docs/CAPABILITIES.md` is the authoritative venue/strategy matrix.
8. **Live-Mode Honesty (2026-09-20)**:
   - `ExchangeAdapter::live_order_submission_implemented` (default false) is true for adapters whose `place_order` genuinely submits: Hyperliquid, Lighter, Robinhood-Lighter, Vanta, Aster, PopDEX, Binance, OKX, Coinbase, Kraken, and Decibel. The old `supports_live_execution` flag is aspirational and must not gate promises.
   - `ExchangeAdapter::live_order_reconciliation_implemented` (default false) remains unimplemented across admitted venues, so `live_ready` / `strategy_entry_ready` are false even when `submission_ready` is true. Live-mode strategy dispatch refuses with the reconciliation caveat; Paper Starting Balance input is disabled/greyed in Live Trading mode.
   - `ExchangeAdapter::order_status_polling_implemented` (default false) is true for every live-submission venue (Binance, Aster, OKX, Kraken, Vanta, Coinbase, Hyperliquid, Lighter, Robinhood, Decibel, PopDEX) and surfaces as `orderPollingReady` on the capability DTO — it means an authoritative `fetch_order` exists, not that entries are permitted.
7. **Paper Funding Carry Execution (2026-09-20)**:
   - `execute_funding_carry_impl` is no longer a stub: paper-only dispatch (live mode refused) mirroring the triangle executor — rescan + match the pair, min-APR hurdle, execution lock, dispatch-time books for both legs, blocking-signal + existing-position guards, per-venue margin checks, full-fill-or-bust paper legs, equity accounting, and ONE two-legged `funding` signal so the Funding dock aggregates a single open carry position. Below-hurdle and stale pairs refuse with reasons instead of executing.
6. **Triangle Scanner Integrity (2026-09-20)**:
   - PopDEX `venue_symbol` mapped ANY `BASE-QUOTE` to `{BASE}USDT` by stripping the quote, so the `BTC-XRP` middle leg was priced off the BTC book and fabricated +3800 BPS cycles. New `require_usd_quote` guard fails closed in `fetch_orderbook` / `fetch_ticker` / `fetch_funding_rate` (bare bases keep the legacy mapping; HL/Lighter/Vanta already reject or resolve honestly). A single-quote venue therefore yields zero triangle opps — correct, since A-B markets don't exist there.
   - Triangle tab: table renamed to **Opportunity Scanner** (no count); row click opens a blurred detail popup (legs with price/fee, gross/fees/net, est profit, latency, synthetic route, Fill Cycle).
5. **Carry Detail Popup — Leg Prints + Trend (2026-09-20)**:
   - Every carry/basis opp carries `long_rate_8h_pct` / `short_rate_8h_pct` (8h-normalized current prints; basis long leg is spot = 0). Rendered as "Long/Short funding" fields in the popup.
   - New `get_funding_trend` IPC serves per-leg 8h funding history + venue-neutral ticker mid history (minute buckets, ≤240 pts, 72h default) for the popup SVG: teal long / orange short (%/8h, left axis), blue ticker mid (right axis), dashed gray net gap on shared timestamps, dashed green model-expected decay from the current net to the min-APR exit threshold over `expected_hold_hours`, dotted red exit threshold. Empty history renders the current prints only — never interpolated. Popup overlay keeps `backdrop-blur-sm` so the scanner grid stays out of focus behind it.

---

### Testing Decisions

#### What Makes a Good Test
- Tests must verify external behavioral contracts, not internal component implementation details.
- Unit tests verify pure quantitative functions: minimum compatible lot sizing, safe leverage calculation, sliding window delta computation, and hurdle rate calculation.
- IPC command integration tests verify that unconfigured environments return empty quote vectors (fail-closed) and that saving and loading strategy configs preserves all parameters.

#### Modules Tested
1. **Tauri Engine Commands** (`commands.rs`):
   - `test_ticker_strategy_config_crud_and_validation`: Verifies saving, retrieving, and validating strategy configuration with sliding window parameters.
   - `test_unconfigured_state_returns_zero_synthetic_data`: Verifies that unconfigured venues return zero synthetic orderbooks or fabricated prices.
   - `test_vault_initialize_unlock_lock`: Verifies encryption, password verification, memory lock, and status reporting.
   - `test_base_mark_prices_use_venue_symbol_and_min_across_quotes`: Verifies bulk venue prices resolve through the exact `MarketMetadata::venue_symbol` and collapse to the conservative minimum base price per venue.
2. **Frontend Quant & Service Layer** (`matrixQuant.ts`, `contractSpecs.ts`, `tickers.ts`):
   - Contract spec calculation for lot sizes and leverage safety across Hyperliquid, Lighter, Vanta, and Robinhood.
   - 2D matrix calculation incorporating taker fees, closing spread friction, and rolling sliding window delta.
   - Dynamic ticker discovery: catalog-absent symbols appear when two real venues report them; single-venue symbols stay hidden; live median prices seed base-price fallback; and exact stablecoin quotes remain distinct.

---


#### Rebalance Friction Estimator (Portfolio Monitor gadget)
1. **Purpose**: price venue → venue fund moves (withdraw fee + network gas +
   deposit fee + optional coin-swap friction) so the user can choose rebalance
   vs fresh deposit vs skip. Inputs: from / to / amount / route (direct vs via
   own wallet) / swap flag / expected opportunity profit.
2. **Researched rails** (`frontend/src/services/rebalanceFees.ts`): Hyperliquid
   $1 withdraw / free deposit (verified); Lighter gasless + bridge gas only
   (documented); Variational $0.10 flat (documented); Vanta/Aster/edgeX/GRVT/
   Decibel/Ondo/CEX rows estimated as ranges; PopDEX + Robinhood-crypto
   unpriceable (null, fail-closed). Every figure carries `FeeConfidence`
   (verified / documented / estimated / unknown) + source note + as-of context.
3. **Direct vs via-wallet**: EVM-settling venues + CEXs support withdrawing
   straight to another venue's deposit address, saving one wallet-hop gas leg
   (~$0.25) and waiting time. The estimator prices both and recommends direct
   where both sides support it.
4. **Worth-it rule**: profit >= friction x 2 (safety multiple) => "worth it";
   else "prefer fresh deposit or skip". Null (unpriceable) legs poison the total
   to null — the UI shows "cannot price", never a lowball sum.
5. **Swap friction**: optional leg at adjustable bps (default 15 = taker fee +
   spread) when the moved coin must convert (e.g. SOL -> USDC).

### Out of Scope
- Direct custody of funds: Arbitrageur connects to smart contracts and exchange APIs via user-owned keys; it does not operate custodial wallets.
- Centralized server syncing: All configurations, vaults, and execution logs remain purely local on the user's workstation.
- Automated token approvals: ERC-20 approvals must be performed via standard wallet signatures prior to swap routing.

---

### Further Notes
- The minimalist blue and light theme follows the design system derived from [ddeng.me](https://ddeng.me/), featuring high-contrast slate typography (`#0f172a`), subtle slate borders (`#e2e8f0`), and vibrant brand accent blue (`#2563eb`).
- Private keys and testing credentials must never be committed to git repositories. All local testing keys are preserved solely in git-ignored local storage at `~/.config/arbitrageur/keychain.toml` (mode 0600 on a native filesystem — never on the shared NTFS mount, where chmod is a no-op and the file would stay world-readable).

---

## Part 2 — Session-aware seasonality layer

> Status: ✅ Implemented for profiles, monitor/Radar coupling, HAR anchors, AutoTune coupling, and scheduled promotion review; advanced models remain out of scope.

## System Specification: Session-Aware Seasonality Layer

Status: **IMPLEMENTED 2026-09-20 — session multipliers remain review-gated; nothing goes live-aggressive without promotion evidence**

### Decisions Locked (proposed — confirm before build)

1. **Sessions (UTC)**: Asia `00–08`, Europe `08–13`, US–EU overlap `13–17`
   (peak), US late `17–22`, dead zone `22–24`, plus a separate
   weekend profile. Overlap-first: execution regimes key off these five,
   not naive 8h blocks (which mislabel the 13–17 peak).
2. **Volatility stack**: EWMA fast layer (5-min realized, λ≈0.96) per pair +
   nightly HAR-style daily anchor; no live GARCH (offline benchmark only).
3. **Math discipline**: shrunken 24h bins (±25% modulation max) over 5
   session aggregates; decisions read sessions, never raw hour bins.
   Provisional bins serve session values. Walk-forward promotion only.
4. **Fail-closed**: funding masks (±2 min around 00/08/16 UTC, +04/12/20
   for 4h venues) block entries; stale/dead sessions degrade to neutral
   ×1.0; Ondo equity legs gate on US cash hours; unknown = no adjustment.
5. **Scope**: implemented = profiles + vol features + monitor/Radar
   integration + HAR anchors + AutoTune coupling + promotion jobs.
   GARCH/ML remain research only.

### Problem Statement

Crypto trades 24/7 but flow does not: volume/volatility concentrate in
US–EU overlap (13–17 UTC, ~47% of BTC volume post-ETF), trough 02–06 UTC
and 21–23 UTC, weekends run ~2× spreads on −20–40% volume, and funding
stamps (00/08/16 UTC, hourly vs 8h/4h venue mismatch) inject transient
basis jumps. Today the engine uses one global OU half-life, one hurdle,
and one sizing rule around the clock — overtrading thin hours into fees
and under-sizing the peak window. No hour/session, volume-series, ATR, or
regime store exists (verified by seam survey).

### Solution

#### Session clock (`arbitrageur-math`)

`session_of(ts_ms) -> SessionId` with the five regimes + `is_weekend` +
`in_funding_mask(ts, venue_intervals)` + `us_cash_open(ts)` (Ondo gate,
DST-aware 13:30–20:00/14:30–21:00 UTC + holiday list seeded manually).
Pure, tested, no I/O.

#### Data: volume sampler + profile tables (migration V8)

- New sampler hooks the existing volume refresh (`cached_venue_volumes`):
  persists `(venue, symbol, ts_ms, vol_usd)` hourly (or on TTL refresh —
  cheapest correct cadence) into `venue_volume_samples`.
- `session_profiles(symbol, session_id, n_episodes, med_abs_spread_bps,
  p90_spread_bps, rv_bps, win_rate, mean_hold_s, updated_ts)` — shrunk
  aggregates written by an hourly rollup over `spread_samples` +
  episode segmentation; 24h bins materialize only as ±25% modulators
  inside the same row family (`session_hour_bins`, provisional-flagged).
- Fast vol stays in-memory EWMA per pair (O(1)/tick); profiles are the
  durable part (SQLite survives restart; hub/cache do not).

#### Strategy integration (monitor `evaluate_pair` + sizing)

- Entry threshold: `entry_thresh × session_entry_mult` (additive bps
  preferred; cap 2× base).
- Sizing: `deriveStepSizing` gains session multipliers —
  inverse-vol (`clamp(σ_med/σ_h, 0.5, 1.5)`) × depth
  (`clamp(D_h/D_med, 0.25, 1.0)`) × `session_allow` (overlap 1.0 /
  EU+US 0.8 / Asia 0.5 / dead 0.4 / weekend ×0.5 / funding-mask 0).
- Qualification: session-confidence gate alongside existing gates.
- Stand-downs: funding masks, spread >3× session median, Ondo-closed
  equity legs, fast-vol >3× norm → halve then halt. All render as
  explicit `block_reason`s.

#### IPC + UI

- `get_session_profile(symbol)` → per-session rows + current session +
  active multipliers (snapshot pattern, registered in `main.rs`).
- Radar: session badge/multiplier column; SignalTrend: session overlay
  band; configurator: session-profile picker beside window-mode. AutoTune
  consumes the session API; promotion gates still decide whether a tuned
  value is READY or HELD.

### Failure & Honesty Constraints

- Provisional/insufficient coverage → neutral ×1.0 + flagged, never a
  guessed multiplier (mirrors `using_manual_fallback`).
- Gaps stay gaps (no interpolation into bins); weekends need 2× floors.
- Stress weeks (e.g. mass-liquidation events) are excluded from
  calibration, labeled as stress data.
- Live mode: visible but non-executing, consistent with existing gates.

### Testing Decisions

- Math: fixture hour-bins → shrinkage math, session aggregation, EWMA
  updates, mask predicates, DST boundaries (frozen timestamps).
- Backend: seeded multi-day history → profile rows; multiplier neutrality
  on thin data; stand-down triggers; IPC round-trips; migration V7→V8.
- Frontend: badge/column rendering incl. provisional states; disabled
  states when no profile.

### Out of Scope

Per-hour τ sets (sessions only), GARCH, ML models, quote-activity volume
proxy refinement, holiday-calendar sync, and unreviewed automatic
threshold changes.

### Rollout

Session clock + tables + sampler → rollup + multipliers → IPC + UI →
gates green (`cargo test`, `clippy -D warnings`, `tsc`, `npm test`) →
weekend of live accumulation → first empirical profiles → promotion
review before any threshold goes live-aggressive.

---

## Part 3 — Spread Range Monitor (Opportunity Radar)

> Status: ✅ Implemented as OpportunityRadar — note the display-vs-engine split: the Min Exp table floor lives on the gadget strip, not the Configurator.

## System Specification: Spread Range Monitor (Opportunity Radar)

Status: **IMPLEMENTED — OpportunityRadar/Spread Radar with manual and opt-in autonomous paper lifecycle; live strategy entries remain disabled**

### Decisions Locked (user sign-off)

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

### Problem Statement

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

### Goals & Non-Goals

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

**Non-Goals / current boundary**

- Live/real-money strategy entries — adapter submission transport,
  order-status reconciliation, opt-in residual auto-resolution, and bounded
  cancel retry exist on all live-submission venues. The spatial two-leg
  path additionally has real dispatch behind an operator opt-in
  (`live_entry_enabled` + per-leg notional cap + vault + venue readiness),
  intended for minimal-size canary validation until venue evidence exists.
- The Phase-2 autonomous bot exists as an opt-in **paper** path; it is not a
  live-trading bot.
- New persistence schema — v1 reuses `spread_samples`; a dedicated
  `monitor_signals` table is deferred.

---

### Data Already Available (integration inventory)

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

### Definitions

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

#### Win probability (honest, fail-closed)

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

#### Potential volume for the fund — marginal-capacity ladder

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

#### Expected time to converge

Primary: OU half-life `t½ = ln2/θ` when the fit qualifies.
Fallback: empirical median time-to-revert of winning episodes.
Otherwise `null → "—"`. This `t_hold` also feeds the funding-carry term
above — longer expected holds accrue more funding drag.

#### Expected profit (ranking key)

```
net_capture_bps(q*) = dev − F(q*) − C_f(t_hold)   (signed; negative dev ⇒ losing direction)
expected_profit_bps = net_capture_bps(q*) × P_win
expected_profit_usd = expected_profit_bps × q* × 10⁻⁴
```

Null `P_win` or unknown `t_hold` → the dependent fields are `null` and the
row sorts below ranked rows (marked collecting/degraded), never scored on
an invented number.

---

### Signal Lifecycle

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

### Architecture

#### Backend — `SpreadMonitorEngine` (new, `frontend/src-tauri/src/monitor.rs`)

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

#### IPC surface (as built)

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

#### DTO (as built, camelCase over IPC)

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

#### Frontend — "Radar" nav tab

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

### Phase 2 seam — autonomous opportunities bot

- `SpreadMonitorEngine::qualified_opportunities()` exposes the same rows
  the UI consumes (status == qualified && executable). The future bot polls
  that stream, applies its own risk gates (max concurrent positions,
  per-venue notional caps, circuit breaker, delta guard) and calls the same
  `execute_spatial_arbitrage_impl`.
- `monitor_signals` (lifecycle persistence: status + first/last-seen +
  last edge/dev per pair) landed with v1 so discovery/duration survive
  restarts; a signal-event audit log still lands with the bot.

---

### Failure & Honesty Constraints

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

### Testing Decisions

- **Unit (arbitrageur-math)**: pair-spread series builder, episode
  segmentation + hit-rate estimator, OU probability fallback, capacity math
  on synthetic books.
- **Backend (commands/monitor)**: qualification transitions, null-when-
  insufficient-data, watchlist derivation, blocked-execution path, stale
  handling, restart rebuild from `spread_samples`.
- **Frontend**: row rendering incl. `—` for nulls, filters, sorting,
  collecting-state display, disabled-execute on blocked symbols.

---

### Open Questions for the User

All resolved — see **Decisions Locked** at the top of this document.

---

## Part 4 — Terminal navigation safeguards

> Status: ✅ Implemented — layout grid, portfolio monitor, settings nav, pause/continue, 5X leverage default.

### Problem Statement

Algorithmic arbitrageurs executing high-frequency spatial convergence, funding carry, and statistical arbitrage across decentralized perpetual exchanges (Hyperliquid, Lighter, Orderly/Vanta, PopDEX, Arcus) and prediction markets require uncompromising execution safety, zero simulated-data ambiguity, and continuous real-time risk visibility.

Prior to this specification, operators faced several operational friction points:
1. **Disconnected Navigation & Layout Drift**: Top strategy switchers used floating pill tiles over a recessed background that shifted and broke alignment with the primary ticker selection panel, adding visual noise.
2. **Fragmented Risk & Performance Governance**: Portfolio net delta neutrality (DeltaGuard), liquidation buffers, multi-strategy profit attribution, and Walk-Forward Optimization (WFO) calibration were split across separate secondary pages, impeding holistic risk assessment during volatility spikes.
3. **Constrained Configuration Viewport**: The System Setting dialog used a rigid vertical sidebar that consumed over 200px of horizontal room, compressing API key forms, venue status cards, and multi-venue balance distribution tables into cramped, wrapping containers.
4. **Lack of Non-Destructive Pause Mechanism**: Traders could only stop trading via an emergency killswitch, which forced position unwinds or required complete process termination. There was no mechanism to temporarily suspend order generation while keeping active hedge cycles and open positions intact.
5. **Uncalibrated Margin Defaults**: Initial perpetual positions defaulted to excessive leverage tiers, reducing liquidation safety buffers below institutional risk tolerance.

### Solution

A unified, high-density desktop trading interface and fail-closed execution architecture that provides:
1. **Flush Boundary-Aligned Strategy Switcher**: A minimalist, full-height top strategy bar with vertical dividers, equal column distribution across active strategies, and a crisp bottom accent underline aligned strictly with the ticker panel's right border.
2. **Unified Full-Width Portfolio Monitor**: A dedicated executive dashboard consolidating cross-exchange signed net exposure (DeltaGuard), real-time margin tripwires, multi-strategy PnL attribution, and automated WFO calibration, accessible via a persistent bottom-left control button.
3. **Top-Navigated Full-Width System Settings**: Horizontal section navigation tabs across the top with integrated AES-256-GCM vault security status and trading mode pills, unlocking the entire modal width for exchange credentials and balance monitoring.
4. **Compact Ticker Panel with Fail-Closed Guidance**: A streamlined 288px sidebar with concise guidance (`Requires at least 2 enabled exchanges`), issue-prioritized multi-criteria sorting (Alphabetical, Strategy Trades, Net PnL), and individual bot switches.
5. **Position-Preserving Pause/Continue Engine**: A non-destructive pause switch that suspends automated bot loops and rejects new dispatches while strictly preserving open lots, active positions, and virtual ledger balances.
6. **Standardized 5X Default Leverage**: Automated leverage clamping and API negotiation across all perpetual venues to maintain a minimum 18% liquidation distance.

### User Stories

1. As an arbitrage trader, I want flat, border-divided strategy tabs flush with the application header, so that switching between trading modules feels snappy, institutional, and free of visual clutter.
2. As an arbitrage trader, I want the start divider of the top strategy switcher to align strictly with the right boundary of the ticker panel, so that the terminal displays a clean, continuous vertical grid line from top to bottom.
3. As an arbitrage trader, I want equal-width distribution across all active strategy tabs (`Futures`, `Spot Arb`, `Funding`, `Prediction`, `Stat-Arb`), so that layout widths remain predictable regardless of live opportunity counts.
4. As a risk manager, I want a single unified Portfolio Monitor dashboard combining DeltaGuard net exposure and multi-strategy performance, so that I can evaluate portfolio health in one place without context switching.
5. As an arbitrage trader, I want a persistent Portfolio Monitor access button in the bottom-left panel directly above System Setting, so that I can open executive risk monitoring from any strategy view with one click.
6. As a risk manager, I want the Portfolio Monitor to expand across the full width of the screen on widescreen displays, so that no display real estate is wasted by artificial width clamps.
7. As a risk manager, I want to view a real-time table of cross-exchange signed net exposure per asset with unhedged tolerance limits ($200 default threshold), so that I can verify DeltaGuard is enforcing complete delta-neutrality across all venues.
8. As a risk manager, I want a dedicated Safety Tripwires & Margin Health card displaying margin utilization, distance to liquidation, and consecutive execution error buffers with prominent progress bars, so that I can detect margin exhaustion well before liquidation.
9. As an arbitrage trader, I want multi-strategy PnL attribution breaking down closed cycles, win rates, Sharpe ratios, and annualized APR across strategies, so that I can identify which arbitrage mechanism is generating the highest risk-adjusted yield.
10. As a quantitative researcher, I want to run Walk-Forward Optimization (WFO) parameter calibrations on demand, so that I can determine the optimal spread estimator (VWMA, EMA, SMMA, WMA, SMA) and quote update window for the active ticker.
11. As an operator, I want the System Setting modal to feature a top horizontal navigation bar instead of a left sidebar, so that configuration forms and exchange balance tables have the maximum possible horizontal space.
12. As an operator, I want the System Setting top bar to display the vault encryption status (`Locked` / `Unlocked`) and trading mode (`PAPER` / `LIVE`), so that I know immediately whether private keys are unlocked in memory.
13. As an operator, I want multi-venue balances to display in a spacious grid with health status pills and safety floor indicators, so that I can monitor collateral distribution and prevent single-leg fill failures.
14. As an operator, I want exchange API configuration cards to include venue filter tabs (`All`, `Hyperliquid`, `Lighter`, `Vanta`, `Arcus`, `PopDEX`, `Robinhood`, `Kraken`, `Polymarket`, `Kalshi`), so that I can quickly jump to specific exchange credentials.
15. As an arbitrage trader, I want the ticker explorer panel to be compact (288px width), so that more screen space is dedicated to live orderbook depth and execution matrices.
16. As an operator, I want the ticker explorer to display a concise message (`Requires at least 2 enabled exchanges`) when no exchanges are configured, so that empty states provide clear guidance without taking up excessive vertical room.
17. As an arbitrage trader, I want to sort tickers by Alphabetical (A-Z), Strategy Trades executed, or Total Net Profit/Loss, so that I can prioritize active, profitable markets.
18. As an arbitrage trader, I want tickers with active position issues or balance imbalances to automatically sort to the top of the ticker explorer, so that potential risks receive immediate operator attention.
19. As an arbitrage trader, I want individual switch toggles per ticker to enable or disable automated execution, so that bots only execute trades on markets I have explicitly authorized.
20. As an arbitrage trader, I want a non-destructive `PAUSE` / `CONTINUE` button in the header left of the `STOP` button, so that I can temporarily halt automated execution without liquidating or dumping existing positions.
21. As an arbitrage trader, I want all open lots in the virtual ledger and active exchange positions to remain untouched during engine pause, so that profitable convergence cycles can resume uninterrupted when continued.
22. As an arbitrage trader, I want all perpetual and futures arbitrage dispatches to default to safe 5X leverage, so that liquidation buffers remain protected across volatile cryptocurrency markets.
23. As an operator, I want exchange adapters that lack native leverage API endpoints to gracefully handle leverage requests without crashing, so that execution dispatches succeed across heterogeneous venues.
24. As an auditor, I want a dedicated Ticker Log Monitor tab alongside Ranked Opportunities and Execution History, so that I can review real-time telemetry and debug logs scoped to the active ticker.
25. As an institutional trader, I want 100% fail-closed market data integrity where zero synthetic prices or fake orderbooks are fabricated, so that paper trading executes strictly against real exchange orderbooks.

### Implementation Decisions

#### 1. Terminal Layout & Grid Alignment
- **Top Strategy Switcher**: Replaced disconnected floating pill tiles with flush, full-height tabs separated by vertical borders (`divide-x divide-surface-border bg-surface`).
- **Active Tab Visual Indicator**: Implemented a high-contrast bottom accent underline bar (`h-[2.5px] bg-accent`) matching the institutional reference styling.
- **Strict Header-to-Sidebar Alignment**: Configured the top header's left branding container to strictly match the ticker explorer panel width (288px / `w-72`), ensuring the vertical start divider lines up with the content pane border.
- **Strategy Tab Sizing**: Allocated the remaining horizontal space evenly across the 5 actionable trading strategies (`grid grid-cols-5`) to eliminate layout jump when opportunity counters change.

#### 2. Consolidated Portfolio Monitor
- **Unified Risk & Performance Scope**: Removed individual `DeltaGuard` and `Performance` tabs from top navigation and merged their telemetry into a unified `PortfolioMonitor` module.
- **Dedicated Access Seam**: Added a full-width `Portfolio Monitor` button with an `Activity` icon in the bottom-left panel directly above `System Setting`. The button highlights with an active accent background when the view is active.
- **Full-Width Canvas**: Removed restrictive `max-w-7xl` container constraints, allowing the dashboard to utilize 100% of available viewport width with generous padding (`p-5 lg:p-7`).
- **High-Density Typography**: Scaled executive KPI metric numbers to `text-xl lg:text-2xl font-black font-mono` and table cell paddings to `py-3.5 px-4`.
- **Badge Wrap Prevention**: Protected status pills (e.g. `Delta Neutral`, `Exposure Skew`) and metric headers with `whitespace-nowrap` to prevent awkward line breaks.

#### 3. System Setting Modal Top Navigation
- **Elimination of Sidebar**: Removed the 224px left sidebar in `EngineSettings` in favor of a top horizontal tab bar spanning the modal header.
- **Top Tab Bar Structure**: Arranged 5 section tabs (`Exchanges`, `Balances`, `Execution`, `Network & VPS`, `About`) horizontally with active bottom border indicators.
- **Status Pills Relocation**: Shifted the `Vault: Locked / Unlocked` encryption status badge and `Mode: PAPER / LIVE` trading mode indicator into the right corner of the top section bar.
- **Full-Width Form Views**: Expanded the scrollable content area to 100% of modal width (~1040px), allowing venue filter badges, API key inputs, and multi-venue balance cards to display comfortably without horizontal truncation.

#### 4. Engine Trading Pause / Continue Mechanism
- **Shared State Architecture**: Added an atomic `is_paused: RwLock<bool>` flag to the backend engine state and IPC system status payload.
- **Fail-Closed Execution Gate**: Guarded all trade execution endpoints (spatial, cross-ticker, funding carry, spot, prediction) with a fail-closed check returning `Err("Trading engine is PAUSED. Positions are preserved, but new executions are suspended.")`.
- **Automated Bot Loops**: Guarded frontend auto-bot intervals across spatial matrix, funding radar, prediction hub, and spot bot to immediately no-op when paused.
- **Position Preservation Guarantee**: Ensured that the pause mechanism does not touch, unwind, or clear open lots in `virtual_ledger` or exchange positions.
- **Header Control**: Positioned a `PAUSE` (neutral) / `CONTINUE` (amber pulse) toggle button immediately to the left of the `STOP` button in the top-right header.

#### 5. Standardized 5X Default Leverage & Safeguards
- **Core Trait Contract**: Added `async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()>` to `ExchangeAdapter` with a default `Ok(())` no-op implementation for exchanges lacking leverage configuration APIs.
- **Perpetual Defaults**: Updated default leverage configuration across Hyperliquid, Lighter, and simulated engines from 10X to 5X.
- **Safe Leverage Calculation**: Implemented `computeSafeLeverage` in frontend contract specifications to clamp target leverage to the minimum safe ceiling across both trade legs (`Math.min(minVenueLeverage, targetLeverage)`).

### Testing Decisions

#### What Makes a Good Test
Tests must verify external behavioral contracts, state transitions, and safety bounds rather than implementation details:
- Assert that entering the paused state prevents new trade dispatches while maintaining active lots in the virtual ledger.
- Assert that unconfigured environments return zero synthetic quotes and empty opportunity sets (fail-closed behavior).
- Assert that ticker filtering properly excludes symbols lacking dual-venue connectivity.
- Assert that leverage calculations clamp to safe venue thresholds.

#### Test Coverage & Prior Art
- **Frontend Unit Tests**: Node `--experimental-strip-types` test runner covering:
  - Venue key normalization and lot sizing (`contractSpecs.ts`).
  - Safe leverage and margin calculation bounds.
  - Rolling spread tracking, baseline pre-seeding, and percentile ranks (`matrixQuant.ts`).
  - Real 2D matrix calculation with commission, taker fees, and sliding window deltas.
  - Pause and continue engine state transitions preserving virtual positions (`pauseContinue.test.ts`).
  - Fail-closed ticker filtering based on enabled/connected venues (`tickers.test.ts`).
- **Rust Backend Workspace Tests**: Cargo test suite covering:
  - `arbitrageur_core`: Orderbook VWAP, market data hub broadcast, liquidation distance.
  - `arbitrageur_exchanges`: Adapter depth consumption, rate limiting, credential validation.
  - `arbitrageur_execution`: Virtual ledger cycle netting, atomic execution dispatches.
  - `arbitrageur_math`: WFO estimator calibration (VWMA, EMA, SMMA, WMA, SMA), Kalman filter convergence.
  - `arbitrageur_risk`: DeltaGuard delta-neutrality enforcement, circuit breaker trip conditions.
  - `arbitrageur_desktop`: Tauri IPC command endpoints for status, settings, pause/continue, and risk killswitch.

### Out of Scope

- Implementing direct hardware key signing integrations (Ledger/Trezor) or a production Remote HSM signer. The current `hsm_url`/`hmac_token` settings are configuration fields only, not an integrated signing backend.
- Automatic capital deposits or fiat on-ramping into exchange accounts.
- Automated liquidation of unhedged positions when the user manually closes a single leg via external third-party web interfaces (covered by manual tripwire and DeltaGuard alerts).
- Support for options volatility surface arbitrage (dedicated to future development cycles).

### Further Notes

- **Desktop Deployment**: The application is packaged as a standalone desktop binary utilizing Tauri v2 with WebKitGTK/Wayland support on Linux.
- **Local Vault Storage**: Sensitive API keys and private keys are encrypted locally using AES-256-GCM and stored only in memory while the vault is unlocked.
- **Fail-Closed Verification**: The terminal requires real credentials for all data streaming; entering paper trading mode executes against genuine live Level 2 orderbooks without risking real capital.

---

## Part 5 — WebSocket feeds & retention

> Status: ✅ Implemented at the desktop feed-supervisor layer for the four supervised lanes below — always-on recorder, retention settings + enforcement. `ExchangeAdapter::subscribe_orderbook` remains a trait-level stub; supervised WS ingestion lives in `frontend/src-tauri/src/feed.rs`, not in each venue adapter.

## System Specification: WebSocket Price Feeds & Historical Retention Control

Status: **IMPLEMENTED — supervised WS clients, feed supervisor, always-on recorder, and retention enforcement; venue coverage is limited to Hyperliquid, Lighter, Robinhood-Lighter, and Vanta**

### Decisions Locked (user sign-off)

1. **Transport**: WebSocket-first only where a supervised lane exists.
   Hyperliquid, Lighter, Robinhood-Lighter, and Vanta stream orderbooks into
   `MarketDataHub`; all other admitted venues use validated REST polling. A
   dead or absent WS lane marks its pairs `stale` — never fabricated. This is
   app-level ingestion; the core `ExchangeAdapter::subscribe_orderbook`
   default still fails closed.
2. **Rate-limit truth (verified against live venue docs, 2026-09-16)**:
   WS does **not** bypass rate limits — it **relieves** them. Lighter's docs
   state REST standard = 60 req/min/IP while WS has a separate envelope
   (255 conns/IP, 500 subs/conn, 200 client msgs/min, keepalive frame every
   2 min), and explicitly warn that REST-abuse rate-limits the WS path too
   ("vice versa"). Today's Lighter book poll costs **2 REST calls**
   (`orderBookDetails` + `orderBookOrders`), so moving books to WS frees the
   whole REST budget for execution/market traffic. Same logic holds for
   Hyperliquid (one info connection/IP, push-on-change) and Vanta/Orderly
   (~1s snapshot pushes). Nothing here evades venue policy.
3. **History**: always-on top-of-book recording to the existing SQLite WAL
   `spread_samples`, independent of which UI page is open. SQLite stays the
   store (no DuckDB — the WAL schema, migrations, and math consumers already
   target it).
4. **Retention controls**: System Settings → Execution → *Historical Price
   Data* card with **Max history days** and **Max storage GB**, both
   **default unlimited** (`None`; `0` input normalizes to `None`). Time and
   size limits compose (whichever binds first wins); each is enforced only
   when set.
5. **Durability**: retention settings persist in `state_store` (migration V7)
   and load at boot. Session-only settings were the prior gap — this closes
   it for retention without migrating the whole settings DTO.

### Problem Statement (pre-implementation baseline)

Before this feature, all ingestion was REST polling (`SpreadMonitorEngine`
lanes + `fetch_orderbooks_impl` 350 ms cache-first fallback, hub TTL
400 ms). `ExchangeAdapter::subscribe_orderbook` was — and still is — a
trait-level stub; implementation now lives in the desktop feed supervisor.
Consequences of the old baseline:

- Lighter-family books burn 2 REST slots each against a 60/min standard IP
  budget (45 usable after the 25% tx reserve) — the watchlist cadence
  degrades to 5–300 s/symbol as symbols grow.
- Quotes go stale between polls; `spread_samples` has gaps exactly where
  the newest venues should be.
- Retention is hardcoded (30 days, 4 M rows, hourly prune in
  `fetch_orderbooks_impl`) with no user control and no GB accounting.

### Solution

#### Transport — per-venue WS clients + feed supervisor

A supervisor task per **supported WS venue** owns **one** WS connection and
multiplexes all watchlist symbols on it. Supported lanes are enumerated in
`frontend/src-tauri/src/feed.rs`; adding an admitted venue does not
automatically create a WS lane:

| Venue | URL | Subscribe | Messages |
|---|---|---|---|
| Hyperliquid | `wss://api.hyperliquid.xyz/ws` | `{"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}` | `{"channel":"l2Book","data":{"coin","levels":[bids,asks],"time"}}` push-on-change; server ping ~30 s (tungstenite auto-pong); max one info connection per IP |
| Lighter (+ Robinhood-Lighter) | `wss://<host>/stream` (derive from adapter `base_url` by scheme swap; RH host `api.rh.lighter.xyz`) | `{"type":"subscribe","channel":"order_book/{market_id}"}` (market_id via existing `orderBookDetails` resolution) | snapshot on subscribe, then `type:"update/order_book"` deltas ~50 ms with `begin_nonce`/`nonce` continuity; gap (`begin_nonce != last nonce`) → resubscribe; client frame at least every 2 min keepalive |
| Vanta (Orderly) | authenticated URL `wss://ws-evm.orderly.org/ws/stream/{account_id}` when configured, otherwise the public WS URL helper | `{"id","topic":"PERP_<BASE>_USDC@orderbook","event":"subscribe"}` | depth-100 snapshot push ~1 s: `{topic,ts,data:{symbol,asks:[[px,qty]],bids}}`; malformed/identity-mismatched frames are dropped |

Message handling (shared): parse → reuse each adapter's existing REST
level parsers where shapes match → `validate_adapter_book` identity check →
`MarketDataHub::update_orderbook` → sample hook (top-of-book to recorder).
Reconnect: exponential backoff ≤120 s (mirror lane backoff); while
disconnected the lane's books age out of the 400 ms TTL and pairs go
`stale` via the existing path. REST `fetch_orderbook` stays for cold start
and any symbol without a fresh hub book.

Robinhood EVM (chain/router, no CLOB) keeps its current status: no WS book
feed; the Lighter-protocol RH endpoint uses the Lighter WS row above.

#### Recorder — always-on

The monitor task records every WS-fed top-of-book into `spread_samples`
under the canonical base (`canonical_base`), merging the `BASE`,
`BASE-USDC`, `BASE-USD` aliases exactly as today. Recording is best-effort:
store failures never break ingestion. No new schema for samples.

#### Retention — settings, durability, enforcement

- `EngineSettingsDto` (Rust + TS mirror): `spread_samples_max_days:
  Option<u32>`, `spread_samples_max_gb: Option<f64>`; defaults `None`.
  Validation: reject NaN/negative/infinite; `0` normalizes to `None`.
- Durability: new `key_value` table (migration V7:
  `CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
  `save_engine_settings_impl` upserts `retention.max_days` /
  `retention.max_gb`; `AppEngineState` boot loads them into settings.
- Enforcement (existing hourly prune slot in `fetch_orderbooks_impl`
  extended): if days set → `DELETE WHERE ts_ms < now - days`; if GB set →
  measure via `PRAGMA page_count` × `PRAGMA page_size`, delete oldest rows
  in 100k batches until under budget (one extra batch bound for safety).
  Both unset → prune is a no-op (oldest data accumulates).
- UI: Execution tab *Historical Price Data* card — two numeric inputs with
  "No limit" hints, empty = unlimited; save path reuses
  `save_engine_settings`.

#### IPC surface

- No new commands. `get_engine_settings` / `save_engine_settings` carry
  the two fields; `get_monitor_snapshot` unchanged. Retention state
  observable via existing `spread_samples` counts (SignalTrend footer).

### Failure & Honesty Constraints

- WS gap, malformed frame, unknown symbol, failed validation → drop frame,
  resubscribe/backoff; pairs go `stale`, never interpolated.
- Unknown-fee/unknown-funding rows keep current fail-closed behavior.
- Prune failures are best-effort and logged, never break ingestion.
- Live mode: feeds may run for research visibility; execution stays
  disabled per existing gates.

### Testing Decisions

- **Unit (exchanges)**: fixture JSON per venue → parsed `OrderBook`
  equality; Lighter nonce-gap → resubscribe signal; Vanta broker-suffix
  rejection; WS-URL derivation incl. RH host.
- **Backend (desktop)**: prune honors days-only / GB-only / both / neither
  (in-memory store + temp file size); settings validation (0→None,
  negative/NaN rejected); boot loads persisted retention; IPC round-trip.
- **Frontend**: retention card renders unlimited state, saves `None` on
  empty, clamps invalid input; no other page affected.
- No network in tests — fixtures only (prior art: Vanta fixture parser
  tests, monitor evaluate tests).

### Out of Scope

- Live trading, signing, execution changes.
- WS private/user streams, funding WS channels (funding stays REST/EMA).
- DuckDB or second store; backfill of history older than retention.
- Migrating the rest of `EngineSettingsDto` to durable storage.
- Robinhood-EVM on-chain pool quotes (unchanged).

### Rollout

Land transport + recorder first (REST fallback keeps behavior identical on
failure), then retention settings + enforcement, then this spec's
verification gates: `cargo test -p arbitrageur-desktop`, `cargo clippy
--workspace -- -D warnings`, `tsc --noEmit`, `npm test`.

