# Failure Modes, Bugs, Solutions & Operational Runbook

> Operational — failure modes, fixes, pre-flight checklist, incident response. Still the playbook for deploys and incidents.


This document captures the failure modes, production bugs identified during the analysis of open-source arbitrage codebases, the controls designed for them, and an operational runbook.

> **Current capability boundary:** the controls below describe the engineered/paper path and the safety architecture. Working orders are reconciled to terminal fill/cancel/expiry by the startup+5s supervisor and recovered after restart, and automated residual-leg resolution plus bounded cancel retry exist as opt-in mechanisms, but without venue canary evidence new live strategy entries remain disabled. Do not interpret this runbook as proof that live trading, automatic unwinds, or remote signing are currently enabled. `docs/CAPABILITIES.md` is authoritative for what each path can do today.

---

## 1. Bugs Discovered in Reference Repositories & Engineered Solutions

### 1.1 Bug 1: Legging Risk / Asymmetric Execution Failure
- **Symptom:** The bot buys on Exchange A, but the order on Exchange B is rejected or times out due to network jitter. The bot is left holding a naked directional position, suffering catastrophic drawdowns if the market dumps.
- **Root Cause:** Sequential order dispatch (`await place_a(); await place_b();`) without atomic failure rollback.
- **Engineered control:**
  1. **Concurrent Dispatch:** Dispatches both legs simultaneously using `tokio::join!` where the strategy path is enabled.
  2. **Emergency compensation design:** If Leg B rejects while Leg A fills, `EmergencyUnwindHandler::unwind_leg` is designed to submit a reduce-only compensation order. Its terminal result must still be verified at the venue; unresolved residual exposure is preserved and surfaced rather than treated as success.
  3. **Circuit Breaker Trip:** Consecutive execution discrepancies trip the `CircuitBreaker`, halting new order entries until operator review.

---

### 1.2 Bug 2: 429 Too Many Requests & Sudden Exchange IP Bans
- **Symptom:** Polling account balance or orderbook every 100ms triggers Cloudflare or exchange rate limiters, resulting in HTTP 429 responses and dropped connections.
- **Root Cause:** Uncoordinated polling across multiple strategy loops.
- **Our Solution:**
  1. **AccountRequestCoordinator:** As reverse-engineered from PandaZhai and hardened in Rust, requests pass through a token-bucket coordinator with minimum interval throttling (e.g. 150ms).
  2. **TTL In-Memory Cache:** Cached balances are returned within TTL (e.g. 2000ms) without hitting the network.
  3. **Exponential Backoff with Jitter:** On 429 or 5xx errors, backoff automatically scales exponentially:
     $$\text{Backoff}(k) = \min(\text{Backoff}_{\max}, \text{Backoff}_{\text{base}} \times 2^k)$$

---

### 1.3 Bug 3: Quote Staleness & Skewed Multi-Exchange Timestamp Desynchronization
- **Symptom:** Orderbooks from Exchange A and Exchange B are compared using wall-clock time, but Exchange B's feed has delayed packets by 500ms, triggering phantom arbitrage signals on already-consumed prices.
- **Root Cause:** Relying on local receive timestamps rather than event-driven sequence tags and depth validation.
- **Our Solution:**
  1. **Depth VWAP Verification:** Evaluates whether target size $Q$ is actually available in the orderbook at acceptable slippage before firing.
  2. **Rolling Spread Baseline:** Compares against a rolling window of recent spreads; sudden anomalous spikes exceeding 250 BPS are filtered out as toxic price artifacts.

---

### 1.4 Bug 4: Funding Rate Period Mismatch (1-Hour vs 8-Hour)
- **Symptom:** Bot computes $F_A - F_B$ directly. Since Hyperliquid quotes 1-hour funding ($0.03\%$) while Lighter quotes 8-hour funding ($0.01\%$), naive calculation believes the spread is only $0.02\%$, when in reality Hyperliquid's 8-hour equivalent is $0.24\%$, miscalculating carry returns by an order of magnitude.
- **Root Cause:** Failure to normalize funding periods.
- **Our Solution:**
  `FundingCarry` normalizes all rates to continuous hourly rates before comparison:
  $$r_{\text{hourly}} = \frac{F}{\text{Interval Hours}}$$
  $$\text{APR} = r_{\text{hourly}} \times 8{,}760$$

---

### 1.5 Bug 5: Net Delta Exposure Drift & Liquidation Cascades
- **Symptom:** Subtle differences in contract lot sizes or decimal precision cause long and short quantities to diverge slightly (e.g. +1.02 BTC vs -0.98 BTC). Over hundreds of trades, cumulative unhedged delta grows until account equity is wiped out.
- **Root Cause:** Absence of global portfolio delta reconciliation.
- **Our Solution:**
  1. **DeltaGuard:** Continuously computes net signed quantity and USD exposure per asset across all venues.
  2. **Delta Limits:** If $| \text{Net USD Exposure} | > \text{Max Allowed}$ (e.g. $500), opening trades are blocked until positions are rebalanced.
  3. **MarginMonitor:** Tracks distance to liquidation on each leg; triggers alerts if buffer drops below 15%.

---

### 1.6 Bug 6: Strategy Auto-Filter Missing Venue Lockout & Hub Cache Starvation
- **Symptom:** When setting `Auto Min 24h Volume` (e.g. 500,000 USD) or `Auto Min Mid Price` (e.g. 0.5 USD), all 52 tickers in the terminal disappeared.
- **Root Causes:**
  1. **Unreporting Venue Lockout:** The volume filter failed closed if *any* in-scope venue reported no volume data. Because almost all tickers have Vanta in scope, and Vanta lacked a volume endpoint (or rate-limited), every ticker was disqualified.
  2. **Hub Orderbook Cache Starvation:** Mid prices were pulled strictly from the live orderbook hub cache (`hub_mid_usd`). The hub only caches books for the single ticker actively selected and polled in the UI. All other 51 tickers returned `None`, triggering an instant missing-metric block.
- **Our Solution:**
  1. **Reporting-Venue Aggregation:** Evaluates the MIN volume across venues that *actively report volume* (`min_reported >= floor`). Unreporting or rate-limited venues contribute nothing to aggregates without locking out liquid assets.
  2. **3-Tier Price Resolution:** Resolves mid price via live orderbook mid -> exchange mark price -> authoritative benchmark catalog reference prices (`benchmark_base_price` in Rust and `getTickerBenchmarkPrice` in TypeScript). Sub-$0.50 meme coins (PEPE, BONK, SHIB, DOGE) are reliably filtered while major assets (BTC, ETH, SOL) stay visible.
  3. **Vanta 24h Turnover Endpoint:** Implemented `fetch_volumes_24h` on `VantaAdapter` to extract `24h_amount` from Vanta's `marketSummary` endpoint.

---

### 1.7 Bug 7: Decimal Multiplication Overflow in Large-Size / Extreme Funding Rates
- **Symptom:** Scanner or carry bot panics with `Multiplication overflowed` in `rust_decimal::arithmetic_impls`.
- **Root Cause:** Naive arithmetic `trade_size_usd * net_hourly * dec!(24)` or `net_hourly * dec!(24) * dec!(365)` overflows the maximum 96-bit integer capacity of `Decimal` when evaluated against large test trade sizes or extreme annualized rates.
- **Our Solution:**
  1. **Checked Multiplication:** Switched to `.checked_mul()` and `.checked_div()` with `Decimal::MAX` clamp fallback.
  2. **Bounded Annualization:** Safely converts hourly rates to floating-point representation before compounding APY (`(1.0 + r).powf(8760.0) - 1.0`).

---

## 2. Operational Runbook

### 2.1 Pre-Flight Deployment Checklist
1. **Verify Environment:**
   ```bash
   pixi run check
   pixi run test
   ```
2. **Review Risk Limits in Configuration:**
   - `max_daily_drawdown_usd`: Maximum dollar loss before full system shutdown.
   - `max_slippage_bps`: Maximum allowable slippage (typically 10-15 BPS).
   - `max_unhedged_usd_per_asset`: Allowed delta drift tolerance ($200 - $500).
3. **Execute High-Speed Simulation:**
   ```bash
   pixi run run-sim
   ```
4. **Inspect Live Spreads & Funding Rates:**
   ```bash
   pixi run run-monitor
   pixi run run-funding
   ```

---

### 2.2 Incident Response Procedures

| Trigger Event | Automated System Response | Operator Action |
|---|---|---|
| **Leg B Reject** | Leg A automatically unwound with Market order | Verify both exchange balances show 0 delta. |
| **Circuit Breaker Tripped** | All new orders halted, process stays active | Inspect logs for root cause; once resolved, run reset. |
| **Margin Warning (< 15%)** | Warning logged, opening trades restricted | Transfer collateral to the margin-deficient exchange. |
| **HTTP 429 Rate Limit** | Request coordinator enforces exponential backoff | Lower poll frequency in config to increase interval. |
| **Signal `unresolved` / `imbalanced`** | Reconciliation supervisor polls every 5 s; stale working orders get a bounded cancel (3 attempts); entries stay blocked on the ticker | The radar page shows a red attention strip listing each escalated signal with its note. Verify positions at both venues, then use **Reconcile Orders Now** (Portfolio monitor) to force a fresh poll pass. If the venue confirms the order closed but the ledger disagrees, resolve manually and check logs for the identity-mismatch reason. |
| **Residual one-sided exposure (imbalanced entry)** | The supervisor automatically submits reduce-only flattens for the uncovered residual as `flat-{parent}-*` child signals (routed through the resolved Auto Position Exit policy — limit-first with market fallback), then reconciles them to terminal state; bounded to 3 attempts | If attempts are exhausted, the signal stays `unresolved`/`imbalanced` — flatten the residual manually at the venue, then run **Reconcile Orders Now**. |

### 2.3 Order Reconciliation

The desktop app runs a reconciliation supervisor (`reconcile_pending_orders`)
that executes an immediate pass at startup — restart recovery for orders
accepted before shutdown — and then every 5 seconds. Each pass polls every
persisted nonterminal leg via the venue's `fetch_order`, identity-checks the
response (venue + symbol + order id), applies fills/fees monotonically, and
derives the signal status:

- all legs filled → `balanced` (entry) or `closed` (close family)
- all legs ended empty → `failed` (entry only — nothing executed)
- filled leg beside an empty leg → `imbalanced` (residual exposure, ticker stays blocked)
- anything unproven past 15 minutes → `unresolved` (keeps polling; later passes can still resolve)

Synthetic paper order ids (`sim-%`) are excluded from venue polling. An
operator can force an immediate pass via **Reconcile Orders Now** on the
Portfolio monitor (command `reconcile_orders_now`, returns a poll summary).

### 2.4 Live entry canary procedure

Live strategy entries are **opt-in**: off by default, bounded, and only on
the spatial two-leg path (triangle and funding live dispatch is not built).

To run one minimal real-money entry:

1. Unlock the credential vault; set Trading Mode to **Live Trading**.
2. Engine Settings → Execution: enable **Enable live strategy
   entries**; set **Live max notional per leg** just above both venues'
   minimums (e.g. $15–25). Residual auto-flatten is unconditional (no
   toggle since 2026-09-22) — a mixed outcome always self-heals via
   reduce-only flatten children.
3. Pick a symbol with **no open position on either venue** — the entry
   refuses if `fetch_account` shows one. Keep existing positions untouched:
   the system never sweeps account positions; it only manages signal legs
   it created.
4. In the canary panel (visible only when live entries are enabled in Live
   mode): symbol, buy venue, sell venue, qty per leg, accepted loss bps
   (≤50 — this pays the spread+fees to test mechanics). Run it.
5. Watch: `signal_executions`/`signal_legs` rows (venue order ids must be
   populated), the reconciliation supervisor (5s polls → `balanced`), the
   venue accounts (two small positions), and `execution_log`.
6. Close via the external-close / flatten path and confirm `closed` after
   venue polling and a fresh account reread.

Expected canary cost is the spread + taker fees on both legs — typically a
few cents at minimal size. Any `imbalanced`/`unresolved` outcome blocks the
ticker, appears on the radar attention strip, and is handled per §2.2.
