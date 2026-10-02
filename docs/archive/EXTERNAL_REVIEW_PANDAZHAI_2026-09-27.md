# External Review — PandaZhai Arbitrage Tool vs Arbitrageur

**Date:** 2026-09-27
**Subject repo:** https://github.com/lihanyu81/PandaZhai-Arbitrage-Tool
**Local clone:** `/tmp/pandazhai` (shallow clone; ephemeral — re-clone if gone)
**Reviewer context:** findings verified against Arbitrageur working tree at HEAD (`b91b747`+).

**Purpose:** another agent will cross-check this document. Every claim is
anchored to a file/line in this repo or a file in the PandaZhai clone so each
can be verified independently. Where a claim could not be fully verified it is
marked `[UNVERIFIED]`.

---

## 1. What PandaZhai actually is

**Critical caveat for the cross-checker:** PandaZhai is a *binary
distribution*, not a source repo. The flagship package
(`packages/four-arbitrage`) ships compiled artifacts; the only real source
present is `agent-wallet-source/` (MIT, PopDEX agent-key helper). All
strategy/algorithm claims below come from the package README and
`RELEASE-*.md` notes — they describe behavior, but the implementation cannot
be inspected. Treat every algorithmic detail as "documented intent," not
verified code.

Relevant files reviewed (all under `/tmp/pandazhai/`):

- `packages/four-arbitrage/README.md` — product overview, v0.5.24
- `packages/four-arbitrage/RELEASE-0.{1..5}.x.md` — 25 release notes
- `packages/arcus-lighter/RELEASE-0.1.1.md` — Arcus adapter reconciliation notes
- `packages/popdex-lighter/RELEASE-0.5.0.md`
- `agent-wallet-source/README.md`

Venue overlap with Arbitrageur is near-total: Lighter, Robinhood-Lighter,
Vanta, Arcus, Decibel, PopDEX, Entropy, QFEX.

---

## 2. Their documented algorithm (four-arbitrage)

### 2.1 STEP-mode signed inventory (RELEASE-0.4.0)

- Each venue-pair keeps a **signed step counter** (`+N` = long-A/short-B
  bias, `−N` = opposite), bounded by configurable positive/negative caps.
- Same-direction signal → +1 step (up to cap). **Reverse-direction signal →
  removes one existing step FIFO.** Never crosses zero in one action.
- Partial fills produce **fractional steps**.
- Step counts derive from the **execution ledger**, not venue net positions —
  so one venue can hold offsetting exposure across pairs without corrupting
  per-pair state.
- STEP mode coexists with legacy cycle mode; pausing STEP preserves risk
  monitoring while halting step changes.
- Events record before/after step values + confirmation state.

### 2.2 Sliding-window directional signal

- Raw directional spread for buy-A/sell-B = `B.bid − A.ask`; both directions
  sampled on market events; duplicate samples per event suppressed.
- Trigger compares the current directional spread against the **historical
  baseline of the reverse direction**, *excluding the current sample*.
- Warm-up sample count required before opening; window size + warm-up
  configurable.
- Entry threshold reserves fees + slippage; **funding excluded** from entry
  math. Exit ("no-regret") test uses worst-case fee-adjusted baseline.
- UI exposes both directional raw spreads, historical baseline, trigger
  threshold, warm-up count, fee-adjusted absolute spread, bid/ask provenance.

### 2.3 Execution & recovery (RELEASE-0.5.x)

- Client order IDs persisted **before** submission; restart recovery reuses
  original IDs, never auto-resends entries; system stays **paused until
  reconciliation completes**.
- Exit-recovery on partial/failed exits: exit intent persisted; resubmit
  reduce-only orders for *confirmed remaining quantity* only; **≥5s between
  replacement orders; at most one order per side per recovery cycle**; never
  resubmit a completed leg.
- Recovery → PAUSED only after: all original orders terminal **and** no
  resting orders **and** **two consecutive fresh position reads agree with
  current STEP** **and** risk data healthy. Never auto-resumes entries.
- ERROR state: continuous order+position verification; an error alone
  neither forces liquidation nor bypasses an existing opening lock.

### 2.4 Reconciliation detail (arcus-lighter RELEASE-0.1.1)

- Arcus fills can carry timestamps *several seconds before* the order's own
  timestamp → fill queries overlap the order window by ~60s.
- Fill attribution validates order ID + client order ID + market + side +
  quantity.
- Reconciliation backfills fill prices **and fees**, not just step state;
  repeated runs are idempotent; WS/REST duplicate fills must not
  double-count; one leg's timeout must not overwrite the other leg's
  confirmed result.
- Operator can manually associate a fill by time/direction/qty/avg price.

### 2.5 Freshness & ops (RELEASE-0.5.11 and others)

- **Independent clocks:** account-snapshot receive time, liq-price-confirm
  time, mark-price update time tracked separately — never conflated. Mark
  stale >20s blocks entries; background risk recheck ~30s.
- Wait-state **log dedup**: identical consecutive wait reasons log only
  start/recover, not per-tick.
- Entry does **not** require local balance ≥ full notional; relies on account
  risk + slippage + reconciliation checks.
- Top-of-book insufficiency alone does not block if deeper levels satisfy the
  slippage limit (depth walk).
- Strict serial ordering across strategies: next strategy's order waits for
  the previous order's position reconciliation.
- Misc ops: Telegram push, per-venue account+API-key validation, multi-lang
  UI, equity baseline rebase, header resource monitor.

---

## 3. Parity check against Arbitrageur

Features PandaZhai documents that Arbitrageur **already implements** (verified
against current tree):

| PandaZhai feature | Arbitrageur equivalent | Evidence |
|---|---|---|
| Client order ID persisted before dispatch | Signal persisted `in_flight` before dispatch | `frontend/src-tauri/src/execution.rs:1457-1459` |
| ERROR-state continuous verification | Periodic reconciliation supervisor; vault-lock-aware; never converts poll failure to resolved | `frontend/src-tauri/src/reconciliation.rs:1196-1234` (`spawn_supervisor`, `POLL_INTERVAL_SECS` loop) |
| Fill/fee monotonicity, identity checks | `fetch_order` truth; legs only advance (`filled_qty`/`fee_usd` cumulative maxima); order+clientID+symbol identity match | `reconciliation.rs:317-378` |
| Depth-aware entry | Aggregate book depth vs `min_depth_ratio` setting + IOC VWAP limit checks | `execution.rs:1077-1175` |
| Warm-up/baseline gating | Session medians, persistence floor (observed live: 0.09 < 0.20 hold), seasonality, coverage gates | `monitor.rs` eval path; soak-verified |
| Reduce-only exits / partial-fill handling / restart rehydration | `unresolved`/`imbalanced` states, durable per-ticker gate, `ticker_reduce_only`, unwind ledger, `reconcile_signal_legs` | `execution.rs:134,894-910,915-919`; `monitor.rs:3886-3889`; `auto_exec.rs` rehydrate-on-ledger tests |
| Grid accumulation cap | `max_accumulated_units` per ticker config; open units derived from `open_balanced_signal_count` (durable ledger, not cache) | `execution.rs:812-851` |
| Exit cooldown spacing | Cooldown recorded at exit *decision*, including failed attempts | `auto_exec.rs:991-994` |
| Per-venue fee defaults | `VENUE_TAKER_FEE_BPS` table (ours more conservative: e.g. Lighter 1.5 vs their 0, PopDEX 10 vs their 3) | `commands.rs:422-501` |
| Env isolation | `testnet|live|unknown` scoping on all persisted venue/monitor state | `state_store/*`, schema v38+ |

---

## 4. Genuine gaps worth borrowing

Ranked by value-to-effort. Each entry states what to verify before adopting.

### 4.1 Bounded exit-recovery retries — HIGH value, SMALL change

**Theirs:** on a partial/failed exit, resubmit reduce-only for the *confirmed*
remainder; ≥5s between replacements; max one order per side per cycle;
recovery completes only after two consecutive fresh position reads agree.

**Ours (verified):** exit failures keep the position tracked and retry via
the bot loop with a cooldown timestamp (`auto_exec.rs:991-1019`); a partial-
close retry test exists (`commands.rs:15496`
`test_emergency_flatten_retries_partial_close`). But:

- No explicit **per-side-per-cycle replacement bound** — a flapping venue
  could see repeated submissions limited only by the cooldown duration.
- No **two-consecutive-fresh-position-reads** agreement gate before the exit
  is declared complete / before new entries reopen.
- No explicit persisted **exit intent** object separate from the signal/
  tracker rows — [PARTIALLY VERIFIED]: trackers + signal rows persist, but a
  dedicated "exit in progress for residual qty X" record that survives
  restart with its own replacement history was not found. Search terms used:
  `exit_intent`, `exit_in_progress`, `residual`, `replacement`. A
  cross-checker should confirm.

### 4.2 STEP-inventory strategy mode — MEDIUM value, LARGE change

**Theirs:** signed ±N inventory per pair; reverse signals unwind FIFO one
step at a time; never cross zero.

**Ours (verified):** open→close cycles per signal. A balanced open pair
counts toward the grid cap (`open_balanced_signal_count` at
`execution.rs:843-851`) but does **not** block the ticker — only
`in_flight`/`imbalanced`/`unresolved` signals do (`execution.rs:894-910`).
Exits fire on **convergence / stop / timeout** (`auto_exec.rs` test
`exit_on_convergence_stop_and_timeout`), **not on reverse-direction
signals**. A reverse-direction opportunity on a symbol with open pairs would
be evaluated independently — [PARTIALLY VERIFIED]: whether the monitor emits
reverse-direction candidates for symbols with open positions, and whether
such a dispatch would open an offsetting pair (margin-doubling) rather than
close existing units, was not fully traced. A cross-checker should trace
`evaluate_pair` candidate emission + dispatch for this case before designing.

**Why it matters:** their model harvests oscillating spreads (exit on reverse
extreme) rather than waiting for full convergence — plausibly higher turnover
capture in choppy books. This is a strategy-behavior decision, not a bug fix.

### 4.3 Independent freshness clocks in risk surface — MEDIUM value, SMALL change

**Theirs:** account-snapshot / liq-price-confirm / mark-price ages tracked
and displayed separately; wait-reason dedup; explicit "which clock is stale
and by how much" in status.

**Ours (verified):** `stale_ms` freshness gates exist on book paths
(`evaluate_pair` freshness check verified during soak); risk overview reports
`reads paused (vault locked)` honestly. Whether the UI/risk surface exposes
**per-source data ages** (account vs mark vs liq price) was not fully
audited — [UNVERIFIED]. Cross-checker should inspect the risk-overview DTO
fields and the frontend Risk panel for per-source timestamps.

### 4.4 Venue×symbol eligibility ledger — already spec'd

PandaZhai's explicit common-market intersection model is exactly what our
volume-fallback violates today: EURUSD passed the futures auto-filter on
**Lighter's** volume report even though Lighter doesn't list it (in-scope
venues robinhood/vanta had no qualifying figure). Fallback found in
`ticker_passes_strategy_filters` (commands.rs) — falls back to any venue's
volume map when in-scope evidence is missing.

This is already spec'd and labeled `ready-for-agent`:
**https://github.com/ChillingWombat/Arbitrageur/issues/68** —
`symbol_eligibility(venue, symbol, env) -> Unknown | Supported(source) |
Unsupported(until, source)`. The PandaZhai review reinforces the spec; no new
work item needed. A cross-checker may want to extend #68's scope to include
the volume-evidence fix explicitly if not already covered.

### 4.5 Per-direction asymmetric thresholds — LOW-MEDIUM value

**Theirs:** A→B and B→A carry independent target bps / qty / caps per pair.

**Ours [UNVERIFIED]:** whether current strategy params are symmetric per
symbol or already support per-direction overrides was not traced. Relevant
for fee-asymmetric venue pairs (e.g. Lighter 0-fee vs a 5bps venue → optimal
entry threshold differs by direction). Cross-checker should inspect strategy
config structs for direction-keyed fields.

### 4.6 Fill-timestamp lookback — mostly N/A

Their 60s look-back exists because they query fills by **time range**. Ours
is **order-centric** — `fetch_order(order_id, client_order_id)` is the sole
fill truth (`reconciliation.rs:317`), so a fill timestamped before its order
cannot be missed. **Residual audit worth doing:** grep each venue adapter for
any `myTrades`/`userFills`/fills-by-time-range endpoint usage that could
reintroduce the bug class (`crates/arbitrageur-exchanges/src/*.rs`). Quick
scan showed none, but a full sweep wasn't done.

### 4.7 Operator-facing waiting reasons — SMALL value

Their UI shows explicit per-item data ages + missing-field names while
blocked. Our monitor rows already carry named block reasons (verified live:
"spread exceeds 3x session median", "signal not ready", "persistence 0.09 <
0.20"). Marginal improvement only.

---

## 5. What NOT to copy (and why)

| Their design | Why not |
|---|---|
| **Funding excluded from entry threshold** | Weaker honesty than ours — we model carry/funding realism; their entries can lock in negative-carry pairs. Keep ours. |
| **8-venue all-pairs fan-out (28 combos × 2 directions)** | Breadth-first noise; our per-strategy scoped scans with auto-filters are more targeted. |
| **Strict global serialization across strategies** | Their next-strategy order waits on the previous order's full reconciliation. Our durable per-ticker gate is finer-grained and allows concurrent work on unrelated symbols — strictly better. |
| **Telegram push notifications** | We have `ARBITRAGEUR_ALERT_WEBHOOK`; marginal. |
| **No local-balance requirement for entry** | They dropped the balance check relying on account-risk+slippage gates. Ours keeping a balance sanity check is safer on thin venues. Don't weaken. |
| **Binary distribution / trust model** | N/A — distribution choice, not a feature. |
| **agent-wallet-source package** | Generates an agent wallet file w/ 0600 perms + `AUTHORIZE` gate before broadcast. Our credential vault + env-scoped `submissionReady` gating already covers this more comprehensively. |

---

## 6. Recommended action items

In priority order:

1. **Bounded exit-recovery** (§4.1): add per-side-per-cycle replacement
   bound + ≥Ns spacing + two-consecutive-fresh-position agreement before
   recovery completes + persisted exit intent if absent. Tests first:
   `partial exit → one replacement per cycle`, `recovery blocked until two
   agreeing reads`, `restart mid-exit resumes intent not submission`.
2. **Eligibility ledger** (§4.4): execute issue #68 — closes the EURUSD
   volume-fallback hole found in today's session.
3. **Per-direction thresholds audit** (§4.5): cheap check first; if configs
   are symmetric, add direction-keyed overrides per strategy.
4. **Fill-query audit** (§4.6): one grep sweep across adapters for
   time-range fill endpoints.
5. **STEP-inventory mode** (§4.2): strategy-design discussion, not a patch —
   decide whether reverse-signal FIFO unwind belongs in `futures`/radar or a
   new strategy id.
6. **Freshness clocks in UI** (§4.3): after verifying what the risk DTO
   already exposes.

---

## 7. Claims a cross-checker should prioritize

Highest-risk claims in this document (most likely to be wrong):

1. **§4.2** — that a reverse-direction signal on a symbol with open balanced
   pairs does not unwind existing inventory. I verified the entry gate
   doesn't block it and exits are convergence-driven, but did not fully
   trace the dispatch path for this case.
2. **§4.1** — that no persisted exit-intent object exists. Vocabulary in
   this codebase is idiosyncratic (per AGENTS.md audit discipline); the
   concept may exist under a different name (search `unwind`, `tracker`,
   `scheduled_exit`).
3. **§4.5** — symmetry of strategy thresholds (not traced).
4. **§4.3** — granularity of freshness data already in the risk DTO
   (not fully audited).
5. **§3 fee table** — "ours more conservative" holds for the rows compared;
   venue-specific real fee tiers (e.g. Lighter zero-fee retail tier) may make
   some of our defaults overly conservative on live, which is safe but loses
   marginal edge.
