# Spec: Interactive Brokers Paper Trading Integration & Options Strategy Expected Yield Hurdle

## Problem Statement

Automated arbitrage traders on Arbitrageur faced two major operational and economic challenges:

1. **Interactive Brokers (IBKR) Paper Trading Integration Failure**:
   - The user needed to connect their IBKR paper trading account (`DUQ183957`) on Linux without running heavy, legacy desktop GUI applications ("IB Gateway" / "TWS") that cluttered desktop windows and required manual daily restarts.
   - The platform lacked a dedicated headless Web API client portal adapter, causing requests to either fail with unhandled TLS/session errors or get mistakenly treated as blockchain "testnet" accounts rather than institutional TradFi paper trading.
   - Gateway authentication probes returned cryptic `HTTP 403 Forbidden: Error 403 - Access Denied` and `HTTP 411 Length Required` errors despite successful browser SSO logins, triggering 30-second backoff cooldown locks that zeroed out portfolio equity and blinded risk monitoring.
   - The paper trading environment attempted to poll cryptocurrency orderbook snapshots (e.g. BTC via Paxos conids) that paper accounts lacked permissions for, spamming logs with recurring 403 errors and degrading public market data lanes.

2. **Suboptimal Options Bot Dispatches & Missing Yield Hurdle Filter**:
   - Autonomous options strategies (synthetic put-call parity conversions/reversals, box spreads, jelly rolls, and convexity butterflies) lacked a minimum expected return filter based on annualized holding duration.
   - Opportunities with positive gross edge in basis points (e.g., 5 bps on a 60-day expiry) tie up capital for extended horizons at an annualized return far below the risk-free rate or opportunity cost of capital (yielding < 1.5% APR).
   - Without an annualized expected yield hurdle, bots engaged in capital-inefficient churn, paying exchange taker fees and margin collateral on low-yielding long-duration contracts.
   - The Options Configurator modal lacked a parameter input for operators to specify a hurdle rate, and opportunity cards did not visualize the annualized expected yield metric.

---

## Solution

A dual-tier enhancement establishing robust institutional TradFi paper trading connectivity and duration-aware options profitability gating:

1. **Headless Client Portal Web API Adapter & Gateway Security Protocol**:
   - Deployed the official lightweight headless Client Portal Web API gateway (`clientportal.gw`) on `127.0.0.1:5000`, completely replacing legacy desktop GUI applications.
   - Configured mandatory HTTP request headers (`User-Agent: Arbitrageur/1.0`) on all outbound REST calls, satisfying Akamai edge security requirements and eliminating `403 Forbidden` access denials.
   - Streamlined keepalive probes to execute clean `GET /v1/api/tickle` requests, avoiding Vert.x `HTTP 411 Length Required` body-parsing traps.
   - Established honest **Paper Trading** environment semantics: classified paper accounts under `is_paper = true`, verified account ID resolution (`DUQ183957`), and hydrated real-time paper balances ($1,008,580.13 AUD) with venue-aware `paper balance` status labels.
   - Implemented asset-class gating: US Equities and ETFs (`AAPL`, `MSFT`, `NVDA`, `SPY`, `QQQ`, `IWM`) are enabled for paper monitoring, while crypto spot pairs are restricted to live trading accounts with Paxos permissions, eliminating market snapshot permission errors.

2. **Annualized Expected Yield Hurdle Filter for Options Strategies**:
   - Introduced an annualized expected yield metric calculated directly from basis-point net edge and time to expiration:
     $$\text{expected\_yield\_pct} = \left(\frac{\text{edge\_bps}}{100}\right) \times \left(\frac{8760}{\text{hours\_to\_expiry}}\right)$$
   - Established a default **10.0% Annualized Expected Yield Hurdle** (`min_expected_yield_pct = 10.0`) across all options scanners, bots, and dispatch executors.
   - Gated opportunity discovery: opportunities yielding below the configured threshold (default 10% APR) are automatically suppressed from bot dispatch and scanner results.
   - Enhanced the Options Configurator modal (`OptionsConfigModal`) with an explicit **Expected Yield Hurdle (%)** field with full persistence in SQLite.
   - Enriched `OptionsParityCard` and `OptionsPackageCard` in `OptionsArbitrage` to prominently display the calculated `Expected yield` percentage alongside Net Edge.

---

## User Stories

1. As an institutional trader, I want to connect my Interactive Brokers paper trading account without running desktop GUI gateway applications, so that my trading desk operates purely headless in the background.
2. As a desktop operator, I want Arbitrageur to clearly label IBKR balances as "Paper Balance" rather than blockchain "Testnet Balance", so that institutional accounting terminology remains accurate.
3. As a platform user, I want the IBKR Web API adapter to send a valid `User-Agent` header on every request, so that gateway security edge rules never reject my requests with `HTTP 403 Access Denied`.
4. As an algorithmic trader, I want gateway keepalive probes to use `GET /v1/api/tickle`, so that keepalive pings never fail with `HTTP 411 Length Required`.
5. As a risk manager, I want Arbitrageur to ingest my real IBKR paper Net Liquidation and Available Funds (e.g. $1,008,580.13 AUD), so that my risk overview reflects exact buying power.
6. As a systems auditor, I want paper trading adapters to only monitor authorized equity and ETF contracts, so that unpermitted crypto pairs do not pollute logs with 403 snapshot errors.
7. As an options arbitrageur, I want all options opportunities to calculate an annualized expected yield (APR %), so that I can compare trades with different maturities on an apples-to-apples basis.
8. As a bot operator, I want the options trading bot to enforce a default 10% expected yield hurdle, so that capital is never locked into trades yielding less than 10% annualized.
9. As a quant trader, I want to configure the expected yield hurdle in the Options Strategy modal, so that I can adjust the threshold between 0% and 10,000% based on prevailing interest rate regimes.
10. As an automated desk controller, I want options execution re-verification to enforce the expected yield hurdle immediately prior to order dispatch, so that fading yields do not result in execution slippage.
11. As a desktop user reviewing opportunity cards in Options Arbitrage, I want to see the Expected Yield percentage displayed in green for profitable yields, so that high-velocity opportunities stand out.
12. As a trader loading saved bot configurations, I want legacy configs without a specified yield hurdle to cleanly default to 10.0%, ensuring backwards compatibility without runtime exceptions.
13. As a developer running regression tests, I want all Rust backend unit and integration tests to pass cleanly without warnings, ensuring robust execution safety.
14. As a frontend maintainer, I want all TypeScript component tests and service suites to pass cleanly, verifying UI state invariants.

---

## Implementation Decisions

### IBKR Adapter Architecture
- **Client Construction**:
  `reqwest::ClientBuilder` configured with `.user_agent("Arbitrageur/1.0")`, `.danger_accept_invalid_certs(true)` for local Vert.x self-signed TLS, and a 12-second timeout.
- **Probe Logic**:
  `ExchangeAdapter::ping` probes `GET {baseUrl}/tickle` first, falling back to `GET {baseUrl}/iserver/auth/status`.
- **Session Priming**:
  `fetch_account()` issues a preliminary `GET {baseUrl}/portfolio/accounts` to prime the Client Portal portfolio session context before fetching `/summary` and `/positions/0`.
- **Balance Ingestion**:
  `parse_summary_balance` inspects `netliquidation`, `netliquidationvalue`, and `availablefunds` objects, parsing numeric `amount` fields and retaining native currency tags (`AUD`, `USD`).
- **Asset Class Gating**:
  `fetch_markets` and `initial_conids` gate crypto spot pairs (`BTC`, `ETH`) on `!self.is_paper`. Paper mode exclusively monitors top US Equities and ETFs (`AAPL`, `MSFT`, `GOOGL`, `AMZN`, `NVDA`, `TSLA`, `META`, `SPY`, `QQQ`, `IWM`).

### Options Expected Yield Hurdle
- **Annualization Mathematical Formula**:
  $$\text{expected\_yield\_pct} = (\text{edge\_bps} / 100.0) \times (8760.0 / \text{hours\_to\_expiry})$$
- **Hurdle Default Constant**:
  `DEFAULT_MIN_EXPECTED_YIELD_PCT = 10.0` defined in `options_bot.rs`, `options_parity.rs`, and `options_packages.rs`.
- **Data Transfer Objects**:
  - `OptionsParityOpportunityDto` and `OptionsPackageOpportunityDto` carry `expected_yield_pct: f64`.
  - `OptionsParityScanParams`, `OptionsPackageScanParams`, `OptionsParityExecutionParams`, and `OptionsPackageExecutionParams` carry `min_expected_yield_pct: Option<f64>`.
  - `OptionsBotConfigDto` and `OptionsBotPolicy` carry `min_expected_yield_pct: f64` with `serde(default = "default_min_expected_yield_pct")`.
- **Pre-Dispatch Verification**:
  `execute_options_parity_impl` and `execute_options_package_impl` re-scan market depth at dispatch time and abort if `opp.expected_yield_pct < yield_hurdle`.
- **Configurator UI**:
  `OptionsConfigModal.tsx` renders a dedicated **Expected Yield Hurdle (%)** input field within the Bot Parameters section, constrained to `[0, 10000]` with inline tooltips.
- **Opportunity Cards**:
  `OptionsParityCard` and `OptionsPackageCard` display an `<DetailField label="Expected yield" value={...} />` element styled with `text-profit`.

---

## Testing Decisions

### Testing Principles
- Test external system behaviors rather than internal implementation details:
  - Assert that HTTP requests carry required headers and succeed against gateway specs.
  - Assert that opportunities meeting the hurdle are returned, while opportunities below the hurdle are filtered out.
  - Assert that UI elements render expected labels and input values.

### Testing Seams
1. **Exchange Adapter Seam (`crates/arbitrageur-exchanges/src/ibkr.rs`)**:
   - `test_adapter_initialization`: Verifies paper vs. live mode flags, default base URLs, and credential presence.
   - `test_parse_summary_and_positions`: Verifies extraction of Net Liquidation and Available Funds from live Vert.x gateway payloads.
   - `test_symbol_helpers` & `test_status_mapping`: Verifies venue symbol normalization and order status transitions.
2. **Options Scanner & Bot Dispatch Seam (`frontend/src-tauri/src/`)**:
   - `options_parity::tests::scan_finds_parity_edge_on_stub`: Verifies that `min_expected_yield_pct` filters out low-yielding opportunities and retains qualifying ones.
   - `options_packages::tests::package_expected_yield_annualization_math`: Verifies analytical APR calculation across short and long expiries.
   - `options_bot::tests::config_roundtrips_and_validates`: Verifies hurdle validation (`[0, 10000]`), rejection of invalid values, and backward-compatible deserialization of legacy configurations.
3. **Frontend Component Seam (`frontend/src/components/`)**:
   - `OptionsConfigModal.test.ts`: Verifies rendering of the Expected Yield Hurdle input field and parameter binding.
   - `OptionsArbitrage.test.ts`: Verifies rendering of Expected Yield metric badges in opportunity rows.

---

## Out of Scope

- FIX protocol gateway connectivity (the platform explicitly standardizes on the Client Portal REST/WebSocket Web API for headless desktop operations).
- Automated paper deposit / faucet minting (IBKR paper account funding is managed via Account Management portal).
- Non-linear options Greeks surface hedging (delta-neutral hedging is governed by existing delta rebalancers).
- Complex multi-leg exotic options outside the 5 cataloged strategies (Conversions, Reversals, Box Spreads, Jelly Rolls, Butterflies).

---

## Further Notes

- The Client Portal gateway runs locally as a system daemon or user process at `https://127.0.0.1:5000`. Credentials and 2FA are authenticated once per trading day via mobile push notification at `/sso/Dispatcher`.
- The credential vault (`000000`) persists encrypted keys in `~/.arbitrageur/vault.enc`, with credentials loaded into adapter memory upon unlock.
