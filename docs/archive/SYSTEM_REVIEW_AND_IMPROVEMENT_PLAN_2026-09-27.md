# Arbitrageur System Review and Improvement Plan — 2026-09-27

**Status:** Cross-checked recommendation; pending project-owner approval  
**Repository state reviewed:** `be8c270419fcda924ef445c5baefacfb7c93d1c4` plus the three untracked 2026-09-27 review documents  
**Primary product assumption:** live mode is the product; testnet-only capability is not considered complete live support

## 1. Scope and method

This review cross-checked:

- the current Rust workspace and Tauri production runtime;
- the live entry, fill, reconciliation, unwind, risk, market-data, and persistence paths;
- the three documents created earlier on 2026-09-27:
  - `EXTERNAL_REVIEW_PANDAZHAI_2026-09-27.md`;
  - `EXTERNAL_REVIEW_QUANTGUY_REPOS_2026-09-27.md`;
  - `PANDAZHAI_CODE_AUDIT_AND_ACTIONABLE_PROPOSALS_2026-09-27.md`;
- the referenced external repositories at pinned commits:
  - PandaZhai: `4428963894c18aa0d676e32df36b616594f9547b`;
  - entropy-arb: `aa0391471f6bf72f78c45801fb8117b7bf7e8c89`;
  - cross-exchange-arbitrage: `3dd42c06c20458e77eb87c17b4c014df83260c23`;
  - perp-dex-tools: `4679a339b8cdc9998707feeda3c5d8b84fb8681f`;
- Arcus's official API documentation and current public `/v1/markets` and `/v1/feetiers` responses;
- GitHub issue #68, the venue×symbol eligibility-ledger specification.

No live order was sent and no credentialed venue call was made. Public market metadata was read from Arcus. The repository verification baseline was run without modifying tracked files:

- `cargo fmt --check` — passed;
- `cargo test -p arbitrageur-desktop` — 563 passed, 0 failed;
- `cargo clippy --workspace --all-targets -- -D warnings` — passed with zero warnings;
- `cd frontend && npm test` — 172 unit tests plus approximately 80 component tests passed.

## 2. Executive conclusion

Arbitrageur is materially stronger than the external projects in several areas: environment isolation, durable signal state, venue-identity validation, monotonic fill accounting, residual-exposure handling, uncertainty-aware statistics, credential custody, and automated tests. The external repositories should be treated as sources of operational ideas, not as architectures to copy.

The main weakness is not a missing collection of strategies. It is that production behavior is split across large Tauri modules and several partially overlapping models:

- the monitor computes a directional empirical baseline and a marginal depth-capacity curve;
- execution independently recomputes average-price route economics;
- signal rows persist before submission, but the exact client order identities do not;
- account/risk state is aggregated per adapter even where several adapters can represent one economic account;
- strategy configuration exposes multi-unit accumulation while derivative entry rejects any existing venue position;
- market-data, account, liquidation, order-path, and venue-health freshness are not represented by one explicit operational model.

The highest-value work is therefore to deepen the execution and risk modules before adding maker-first modes or more venues.

### Recommended order

1. Fix concrete live correctness gaps: Arcus tick tiers, exact pre-submit order identity, shared-account risk aggregation, leverage verification, and truthful derivative accumulation limits.
2. Introduce one deep execution interface that owns coherent book capture, planning, admission, durable intent, submission, and lifecycle state.
3. Add explicit data/venue health, typed venue errors, protected order budgets, independent freshness clocks, and issue #68's eligibility ledger.
4. Only then add signed STEP inventory and graduate autonomous strategies to tightly capped live canaries.
5. Treat maker-first execution, lock sharding, and new venue integrations as later measured experiments.

## 3. What the system actually is

The production runtime is the Tauri backend, not the crate diagram alone. Startup creates `AppEngineState`, the spread monitor, autonomous executors, reconciliation, feed supervisors, scheduled exits, AutoTune, and the drawdown feed in `frontend/src-tauri/src/main.rs`.

The principal spatial/radar path is:

```text
feed.rs / REST lanes
        ↓
MarketDataHub
        ↓
monitor.rs (baseline, qualification, capacity, readiness)
        ↓
monitor_ipc.rs or auto_exec.rs
        ↓
execution.rs (all admission, fresh books, margin/delta, persistence)
        ↓
arbitrageur-execution coordinator
        ↓
venue adapters
        ↓
reconciliation.rs + flatten children + virtual ledger
```

The workspace crates contain useful pure modules and adapter primitives, but the product orchestration is concentrated in the desktop crate. Measured hotspots include:

- `commands.rs`: 17,208 lines;
- `dex_perps.rs`: 13,761 lines;
- `execution.rs`: 6,633 lines;
- `monitor.rs`: 5,847 lines;
- `funding.rs`: 4,897 lines;
- `state.rs`: 4,115 lines;
- `reconciliation.rs`: 2,579 lines.

Accordingly, `docs/ARCHITECTURE.md` overstates “strict modular separation” and “without mutex contention.” Production deliberately has a global `execution_lock`, a large Tauri orchestration layer, and a broad generic DEX adapter. The architecture document should eventually describe the actual control plane, data plane, and durability plane rather than presenting only crate dependencies.

The headless `arbitrageur-app` is a research/simulation CLI with a placeholder server, not the production daemon. It also constructs `SimulatedExchange` in a release-visible command, which conflicts with the project rule saying simulators exist only under `#[cfg(test)]`.

## 4. Evidence quality of the external reviews

| Source | What is trustworthy | Important limitation | Usage decision |
|---|---|---|---|
| `entropy-arb` | Real MIT-licensed source; marginal book walk, per-venue locks, order budgets, inventory surcharge, own-trade watermark, graceful in-flight drain | Small single-pair Python engine; local mutable position model is less durable than Arbitrageur's ledger | Best source of implementation ideas; independently adapt concepts |
| `cross-exchange-arbitrage` | Real source demonstrating maker-first then taker-hedge flow | README says educational and not for production; AGPL-3.0; fabricates a Lighter fill after timeout | Use the strategy concept only; do not copy code or failure semantics |
| `perp-dex-tools` | Real source with venue examples and some sequence/snapshot handling | Non-commercial license; several scripts also fabricate fills after timeout; volume-farming focus | Do not copy code into the product; use only independently re-derived ideas |
| PandaZhai | Public release notes contain useful operational requirements and claimed test cases | Core is a stripped proprietary binary; product license expressly prohibits decompilation and competitive use; implementation cannot be audited | Use public behavioral notes and independently verified venue facts only |

The document `PANDAZHAI_CODE_AUDIT_AND_ACTIONABLE_PROPOSALS_2026-09-27.md` should not be treated as an approved specification in its current form:

- its claim of a systematic decompiled-code audit is not reproducible from the repository;
- the PandaZhai package license prohibits the stated decompilation/use;
- “millions of dollars of live order flow” is not supported by the cited repositories;
- “decisive” latency superiority is not supported by a benchmark;
- several recommendations are already implemented or target a non-production code path;
- several parity claims are incorrect, as detailed below.

The two `EXTERNAL_REVIEW_*` documents are more appropriately cautious, but their `[UNVERIFIED]` claims still require the corrections in this report.

## 5. Current strengths to preserve

1. **Environment isolation.** Adapters, reads, writes, signals, lots, telemetry, and exits are scoped to `testnet|live`; ambiguous historical rows fail closed.
2. **Fail-closed live admission.** Live entries require an unlocked vault, implemented submission, authoritative order polling, strategy-entry admission, and credentials.
3. **Durable blocking semantics.** `in_flight`, `imbalanced`, and `unresolved` signals block the ticker across restarts.
4. **Authoritative polling.** Reconciliation validates order/venue/symbol identity, rejects quantity regressions, and does not convert poll failure into success.
5. **Bounded residual compensation.** Flatten-child intents are durable, duplicate working children suppress re-dispatch, submitted residual attempts are capped, and retries are spaced.
6. **Strong quantitative monitor.** The radar already has directional baselines, OU/empirical fallback, uncertainty bounds, censored outcomes, coverage and persistence gates, depth curves, session controls, and probability-weighted marginal sizing.
7. **Security posture.** Vault encryption, Stronghold-backed EVM signing, file permissions, signer hashing, field-scoped credential handling, and explicit live capability gates are substantial strengths.
8. **Test discipline.** The current full baseline is green and materially larger than the external repositories' visible test surfaces.

## 6. Cross-check of today's proposed ideas

| Earlier proposal or claim | Verdict | Correct action |
|---|---|---|
| Arcus dynamic tick tiers | **Confirmed live defect** | Fix immediately; official schema says `upToPrice` is exclusive |
| Empirical midline | **Already implemented** | Keep the monitor's directional `mu`; do not add a duplicate strategy model |
| Asymmetric direction bands | **Partly absent** | Consider route/direction policy only after one shared planner; carry is already directional |
| 200–300 ms persistence gate | **Reframe** | Current 5 s evaluation plus persistence ratio exists; first fix book coherence/skew and own-trade watermarks |
| Marginal multi-level depth walk | **Already in monitor, absent at final admission** | Make one planner serve monitor, manual execution, and auto execution |
| Dynamic inventory surcharge | **Valid later** | First resolve the contradiction between grid accumulation and the one-position entry gate |
| Post-trade book suppression | **Confirmed missing** | Invalidate pre-trade book generations until a newer snapshot arrives |
| Bounded exit recovery absent | **Mostly false** | Existing flatten children, 30 s spacing, and three submitted attempts already provide it; add stable completion confirmation rather than a second exit table |
| Persisted exit intent absent | **False in substance** | Flatten/trim child signals are durable exit intents; deepen them into exact order intents |
| Client order ID persisted before dispatch | **False** | Exact client IDs are currently generated before dispatch but not inserted with the signal leg |
| Venue×symbol eligibility ledger | **Confirmed, already specified** | Implement GitHub issue #68; do not create a duplicate design |
| Independent account/mark/liquidation clocks | **Confirmed gap** | Add source-specific freshness to backend gates and UI |
| Hard sends-per-minute budget | **Confirmed gap** | Current signed lane spaces requests by interval but has no explicit rolling count/capacity or reserved exit quota |
| Venue outage state machine | **Partial** | Feed lanes back off, but there is no unified market/account/order health state used by entry admission |
| Shared-account dedup “not applicable” | **False** | Multiple EVM AMM adapters can query the same chain+wallet and are summed per adapter today |
| Strict global serialization is not used | **False** | Spatial, flatten, options, and other paths share one global execution mutex |
| Per-venue locks should replace it immediately | **Premature** | Keep the safe global lock until exact order lifecycle and resource identities are centralized |
| Maker-first execution | **Valid experiment, not near-term default** | Existing `OrderRequest` supports post-only, but partial-fill hedging, cancel confirmation, and amend capability need a new lifecycle policy |
| Manual fill adoption | **Valid operational tool** | Add only after exact durable order identity; candidate binding must be unique, previewed, atomic, and leave the engine paused |
| 120 s forced WS resubscribe | **Protocol-specific, not universal** | Use transport/subscription/book-integrity health; resubscribe only where the protocol cannot otherwise prove validity |
| Expected edge vs fill edge | **Partial parity** | Predictions and realized outcomes exist; unify attribution around the exact executable plan |
| Dust BBO floor | **Low-value standalone knob** | A coherent marginal planner and minimum executable notional should subsume most of it |
| New Backpack/Ethereal adapters | **Defer** | Reduce adapter breadth debt and add conformance gates before adding venues |

## 7. Confirmed high-priority findings

### P0-1. Exact client order identities are not durable before submission

`execution.rs` builds both `OrderRequest`s, including generated client IDs, before inserting the signal. However, `NewSignalLeg` has no client-ID field, and `record_signal_execution` inserts legs without one. The ID is attached only after a venue response carries an order ID.

This leaves a crash window:

1. durable signal says `in_flight`;
2. venue accepts one or both orders;
3. process exits before the acknowledgement/order ID is persisted;
4. restart has no locator for the submitted order.

`list_pending_order_legs` polls an `in_flight` leg only when it has a venue order ID; no-ID legs are included only after a later `imbalanced|unresolved` status. The symbol therefore remains blocked, but the supervisor cannot prove venue truth. This is safer than re-submitting, but operationally incomplete and contradicts the earlier parity claim.

**Required change:** persist the exact order intent, including client order ID, before any network submission. Reconciliation must support an `OrderLocator` with optional venue ID plus mandatory client ID where the venue supports client-ID lookup. Restart must never auto-resend an entry whose submission outcome is unknown.

### P0-2. Arcus ignores dynamic tick tiers

`ArcusMarketSpec` stores only top-level `tickSize`; `fetch_markets` ignores `tickTiers`; order snapping and signed integer conversion always use the top-level tick.

Arcus's official schema states that each order price must be a multiple of the tick for the price band and that `upToPrice` is an **exclusive** upper bound. The current public response contains `tickTiers`. On 2026-09-27, 2 of 59 online markets were already trading outside their base tier:

- `WLFI-USD`: base tick `0.000001`, active tick `0.00001`;
- `USELESS-USD`: base tick `0.00001`, active tick `0.0001`.

These routes can be rejected despite otherwise valid planning.

**Required change:** parse and validate ordered tiers, resolve the tick from the final order price, use it consistently for inward/loose snapping and the signed `p` value, and retain top-level `tickSize` only as a fail-closed legacy fallback.

Official sources:

- <https://docs.arcus.xyz/api-reference/public/get-markets.md>
- <https://api.arcus.xyz/v1/markets>

### P0-3. Portfolio equity can double-count one shared EVM economic account

Every enabled EVM AMM adapter reads the same wallet's native/ERC-20 balances for its chain and emits an `AccountSnapshot` under its venue ID. Risk aggregation then sums account equity by adapter/venue. The credential system explicitly supports a shared Stronghold wallet across EVM AMMs.

If two same-chain AMM adapters use the same wallet, overlapping balances can be counted twice. This can inflate portfolio equity, understate margin ratios, and distort the drawdown baseline. The earlier external review's “not applicable” conclusion is therefore incorrect.

The breaker map also retains every key in the active environment, not every currently admitted economic account. Disabling a venue can leave old equity in the total, while a real universe change does not re-anchor unless the environment itself changes.

**Required change:** introduce `EconomicAccountId` (for EVM: chain ID + address; for centralized venues: venue/entity + account/subaccount). Aggregate equity once per economic account, keep venue-specific execution capabilities separately, and re-anchor the breaker when the admitted economic-account set changes. A temporary outage may retain last-known equity for an admitted account, but freshness must remain visible and entries must fail closed when required data ages out.

### P0-4. Leverage application is best-effort after margin was calculated as if it succeeded

The spatial path calculates initial-margin requirements using the target leverage, then calls `set_leverage`. A rejected leverage change logs a warning and the order still submits at the venue's existing leverage.

If existing leverage is lower than the assumed target, the pre-check understates required margin. One leg may reject while the other fills, forcing an avoidable real-money unwind.

**Required change:** live entry must either prove the effective leverage before sizing or fail closed. Set leverage before the final account/margin snapshot, then verify the applied value through an authoritative venue read where supported. A venue with unverifiable leverage must be sized from a conservative known bound, not the desired value.

### P0-5. Derivative grid accumulation is exposed but blocked by the entry path

The UI/config and ledger support `max_accumulated_units`, with a default greater than one. Execution counts open balanced signals against that cap. Later in the same path, however, any existing derivative position on either venue is rejected with “closing lifecycle is not implemented.”

Therefore, live derivative accumulation cannot reach its advertised cap. Reverse signals also do not currently perform STEP/FIFO unwind; they reach the existing-position refusal instead.

**Immediate truth fix:** for derivative strategies, clamp live accumulation to one and reject/hide configurations above one until a real inventory model exists. Do not claim grid/STEP accumulation from the durable count alone.

**Later capability:** add explicit cycle-vs-STEP inventory modes as described in Phase 3.

## 8. Target module design

### 8.1 One external execution interface

Callers should cross one seam:

```rust
pub trait ExecutionEngine {
    async fn execute(&self, intent: ExecutionIntent) -> Result<ExecutionReceipt, ExecutionReject>;
}
```

`ExecutionIntent` expresses strategy intent, not venue wire details. It should contain the environment, strategy, route/legs, desired size policy, and execution policy (`AtomicTaker` initially). It must not let an IPC caller override a safety floor.

Behind this interface, the implementation owns the required ordering:

1. canonicalize route and environment;
2. acquire execution resources;
3. capture a coherent market snapshot;
4. resolve venue capabilities, fees, economic accounts, and fresh risk state;
5. build one exact executable plan;
6. materialize client IDs once and persist the exact leg/order intents;
7. submit according to policy;
8. poll authoritative terminal state;
9. compensate residual exposure;
10. persist the final execution and attribution outcome.

This module should replace—not layer on top of—the duplicated gate ordering in spatial, funding, triangle, options, and flatten paths.

### 8.2 Internal pure planner

Use an internal seam with a compact interface:

```rust
fn plan(snapshot: CoherentPairSnapshot, policy: RoutePolicy) -> Result<ExecutablePlan, PlanReject>;
```

`ExecutablePlan` should carry:

- exact common-grid and venue-native quantities;
- exact IOC bounds and client-ID-free order templates;
- cumulative and marginal entry economics;
- modeled exit friction;
- fees and their evidence/source;
- book receive generations, age, and cross-venue skew;
- expected edge and maximum adverse slippage;
- every invariant needed to audit why submission was allowed.

The monitor can call the same pure planner for candidate capacity; final execution calls it again on a new coherent snapshot. The lifecycle layer then allocates client IDs exactly once and persists the resulting requests before submission. This removes the current split between marginal monitor sizing and average-price dispatch admission.

### 8.3 Durable order lifecycle

Represent order state explicitly rather than inferring it from optional fields:

```text
Prepared → Dispatching → Working → Filled/Empty/Partial/Unknown
                         ↓
                 Compensating → Flat/Unresolved
```

Persist the exact request before `Dispatching`. Store every submit/cancel/replacement attempt idempotently. Flatten children remain the durable compensation mechanism; they should become consumers of the same order-lifecycle module rather than a separate implementation.

### 8.4 Portfolio risk and health modules

A deep `PortfolioRisk` module should produce one `PortfolioRiskSnapshot` from deduplicated economic accounts. Each source must carry independent freshness:

- account snapshot received;
- position set confirmed;
- mark prices received;
- liquidation fields confirmed;
- order status last verified.

A separate `VenueRuntimeState` should track facets instead of one vague online/offline flag:

- market-data transport and subscription integrity;
- symbol eligibility;
- account-read health;
- order-submit health;
- order-poll health;
- public/private/entry/exit request budgets;
- last typed error and retry deadline.

Risk-reducing cancels/exits must have reserved capacity and must not be blocked merely because the entry budget is exhausted.

## 9. Phased implementation plan

### Phase 0 — live correctness and truth

#### 0.1 Arcus market-rule fix

- Parse `tickTiers` into validated ascending bands.
- Treat `upToPrice` as exclusive.
- Resolve from the final snapped price for limit and market protective bounds.
- Refresh the stale Arcus fee assumption: the current public base taker fee is 225 ppm (2.25 bps), not the hard-coded 450 ppm. Prefer a timestamped fee profile with a conservative fallback.

**Acceptance criteria**

- boundary-minus, exact-boundary, boundary-plus, and unbounded-tier tests;
- malformed/unsorted/missing tiers fail closed or use only a proven base fallback;
- fixtures for currently non-base markets produce an aligned signed price;
- no order body or signature uses a different tick from validation.

#### 0.2 Persist exact order intent before submission

- Add client order identity and exact request fields to the pre-submit persistence interface.
- Replace mandatory-string `fetch_order` identity with a typed locator that supports client-only recovery where the venue supports it.
- Cover every entry/exit strategy, not just spatial.

**Acceptance criteria**

Crash/restart tests at these boundaries:

1. after persistence, before either submit;
2. after leg A acceptance, before acknowledgement persistence;
3. after both submissions, before either acknowledgement persistence;
4. while one or both orders are working.

In every case, restart reuses identity, does not auto-resubmit entry, and cannot mark a fill from an acknowledgement alone.

#### 0.3 Deduplicate economic accounts and fix breaker membership

- Add adapter-provided economic account identity.
- Deduplicate same-chain/same-wallet EVM snapshots.
- Remove disabled economic accounts from the breaker total and re-anchor on membership changes.
- Preserve last-known equity only for still-admitted accounts and surface its age.

**Acceptance criteria**

- two AMMs on the same chain+wallet count equity once;
- the same address on two chains remains two economic accounts;
- disabling a venue cannot leave ghost equity;
- a temporary read outage retains last-known value without reporting it as fresh;
- universe changes re-anchor rather than appearing as profit/loss.

#### 0.4 Make leverage fail closed in live mode

- Establish/verify leverage before final margin admission.
- Re-read account/risk state after the change.
- Do not submit either leg if one venue's effective leverage is unknown or below the sizing assumption.

#### 0.5 Make accumulation claims truthful

- Clamp derivative live mode to one open unit until Phase 3.
- Keep spot behavior separate because inventory semantics differ.
- Update UI/docs so a configured cap cannot imply unreachable behavior.

### Phase 1 — deepen execution planning and lifecycle

#### 1.1 Build `ExecutionEngine` and migrate spatial first

Move canonicalization, live capability checks, coherent book capture, risk admission, exact planning, intent persistence, coordinator invocation, and outcome persistence behind one interface. Keep emergency exits market-based.

#### 1.2 Use a coherent pair snapshot

The current monitor may accept books 30–300 seconds old, while final dispatch uses a two-second cache window. Neither path requires a maximum cross-venue receive skew.

Add per-slot receive generation and monotonic timestamp to the hub. Capture both books concurrently, then enforce:

- source-appropriate maximum age;
- maximum pair skew;
- valid subscription/snapshot epoch;
- sequence integrity for delta feeds;
- a generation newer than each venue/symbol's last own trade.

#### 1.3 Enforce marginal profitability at final admission

Use the existing level-by-level capacity approach at dispatch. Test a book where cumulative VWAP clears the floor but the final marginal slice does not. The executable quantity must stop before that slice.

Report both cumulative and worst-unit net bps; persist both with the plan.

#### 1.4 Graceful shutdown

- stop admitting new entries;
- keep exits/reconciliation available;
- wait a bounded interval for in-flight lifecycle tasks;
- persist shutdown state and unresolved work;
- rely on restart recovery rather than cancelling mid-dispatch.

Crash recovery remains mandatory; graceful shutdown is defense in depth.

#### 1.5 Operator-assisted fill adoption

After exact order identity exists, add a read-only candidate query and explicit adoption flow:

- exact account, venue, symbol, side, time, and quantity checks;
- only complete, unbound fills;
- unique constraint on venue trade ID/account;
- preview reconstructed delta, price, fees, and P&L;
- atomic bind plus audit backup;
- engine remains paused after adoption until a fresh reconcile pass succeeds.

### Phase 2 — operational health, evidence, and observability

#### 2.1 Implement issue #68

Use the existing specification for the environment-scoped venue×symbol eligibility ledger. Include the volume-filter fix: evidence from unrelated venues must not satisfy an in-scope route's filter.

#### 2.2 Introduce typed venue errors

Replace string interpretation at orchestration boundaries with categories such as:

```text
RateLimited(retry_after)
InsufficientMargin
UnsupportedSymbol
Authentication
Transport
Rejected(reason)
UnverifiableOrder
```

Typed errors should drive eligibility, entry pauses, alerts, and retry policy. They must never turn unknown order state into failure/success.

#### 2.3 Protect order-path budgets

Upgrade `AccountRequestCoordinator` from interval-only pacing to configured rolling/token budgets for:

- public market reads;
- private account/order reads;
- new entries;
- cancels and reduce-only exits.

Reserve cancel/exit capacity. A margin rejection pauses new entries touching that venue until a fresh account read proves recovery; it must not pause risk-reducing exits.

#### 2.4 Surface independent freshness clocks

Extend risk DTOs and UI with ages/status for account, positions, marks, liquidation confirmation, and order polling. An unchanged liquidation value can still be freshly confirmed; do not use value-change time as freshness.

Entry gates consume these clocks. Display-only stale data may remain visible but must be labeled.

#### 2.5 Protocol-specific feed validity

- Lighter: keep sequence-gap resnapshot behavior.
- Full-snapshot feeds: track transport/subscription epoch and complete-book receipt.
- Quiet markets: do not confuse “price unchanged” with “connection dead.”
- Resubscribe periodically only for protocols where heartbeat cannot prove subscription validity.

#### 2.6 Dynamic fee and capability evidence

Replace hand-maintained fee/capability assumptions with timestamped venue/account profiles where APIs expose them. Keep a conservative configured ceiling when live truth is unavailable. Show the evidence source and age in route diagnostics.

### Phase 3 — inventory semantics and live automation

#### 3.1 Explicitly choose inventory mode

Do not silently change current cycle behavior. Add an explicit mode:

- `Cycle`: one independently opened/closed pair;
- `Step`: signed route inventory with confirmed fractional units.

For STEP mode:

- route key is `(environment, strategy, symbol, buy venue, sell venue)`;
- units derive only from confirmed ledger fills;
- same-direction signal adds one lot up to cap;
- reverse signal closes one FIFO lot and never crosses zero in the same action;
- venue net positions are reconciliation invariants, not the source of lot ownership;
- restart reconstructs exactly from durable lots;
- shared venues may net economically while route lots remain distinct.

Only after this exists should the optional inventory-ladder surcharge be added for risk-adding orders. Reducing orders receive no surcharge.

#### 3.2 Graduate autonomous radar/options to live canaries

Today autonomous radar and options entries are intentionally hard-blocked in live mode. That is safe, but it is not finished live-mode capability.

Graduation requirements:

- all Phase 0 and relevant Phase 1 gates complete;
- exit bands/stop bands persisted from the same model used at entry (remove the documented testnet approximation);
- startup begins paused and reconciliation-clean;
- shadow/record-only decisions first;
- explicit operator arm per environment;
- tiny per-route and per-account notional caps;
- one open derivative cycle per symbol until STEP mode is proven;
- automatic fallback to reduce-only on health degradation;
- no automatic resume after unresolved order state.

### Phase 4 — measured advanced execution

#### 4.1 Maker-first then taker-hedge

Treat this as a distinct execution policy behind `ExecutionEngine`, not a new strategy bypassing it.

Prerequisites:

- adapter capability contract for post-only, order stream/polling, cancel confirmation, and amend;
- incremental hedging of partial maker fills;
- no hedge based on an assumed/fabricated fill;
- confirmed cancel before any fallback that could stack orders;
- bounded maker lifetime and adverse-selection metrics;
- automatic residual compensation through the same lifecycle.

Evaluate it in record/shadow mode using realized maker fill rate, adverse selection, fee savings, and hedge slippage before live enablement.

#### 4.2 Replace the global lock only after resource identity is explicit

Do not jump directly to per-symbol locks. Symbols sharing a venue/economic account also share margin, rate budgets, and reconciliation state.

When the lifecycle module is stable, acquire deterministic sorted resource locks for the involved economic accounts/venues. Disjoint resources may execute concurrently; overlapping venues/accounts serialize. Reconciliation must use the same locks. Keep the global lock until equivalence tests prove no duplicate submit or account-race regression.

#### 4.3 Defer venue expansion

`dex_perps.rs` already models 26 long-tail venues in one 13k-line file, including doc-shaped integrations. Backpack/Ethereal should not be added merely because another repository includes them.

Require first:

- adapter contract and wire-conformance harness;
- live submission plus authoritative order polling;
- credentialed read-only evidence and recorded payload fixtures;
- typed capabilities and errors;
- venue profitability/liquidity evidence;
- licensing-clean, independently implemented code.

## 10. Test strategy

The safest sequence is test-first at module interfaces.

### Execution contract tests

- exact order request persisted before network call;
- acknowledgement is never treated as fill;
- wrong order/client/symbol/venue response rejected;
- partial and late fills remain monotonic;
- process restart at every dispatch boundary;
- no duplicate entry on restart;
- cancel unconfirmed means no market fallback stacking;
- residual attempts are bounded and spaced;
- emergency exits remain available when entry budget/venue entry circuit is closed.

### Planner tests

- VWAP passes but marginal unit fails;
- asymmetric level depth;
- common-grid quantity snapping;
- stale, skewed, wrong-source-epoch, and pre-own-trade books;
- fees change between scan and execution;
- exact route intent remains auditable after serialization.

### Portfolio-risk tests

- economic-account deduplication;
- account-universe re-anchor;
- independent source clocks;
- stale last-known value retained but labeled;
- stale required risk source blocks entry;
- same wallet on different chains is not deduplicated.

### Adapter conformance tests

For every live-capable adapter, generate all supported combinations of order type, TIF, post-only, reduce-only, and price bound; assert the exact wire request and explicit refusal of unsupported combinations. No adapter may silently downgrade policy.

## 11. What not to copy

- Any fabricated-fill timeout from `cross-exchange-arbitrage` or `perp-dex-tools`.
- Hard process exit on imbalance.
- PandaZhai's exclusion of fees/funding from admission economics.
- Global serialization as an end state, or per-symbol locking without account/venue resources.
- A blanket 120-second resubscribe independent of protocol semantics.
- A fixed `$4,000` BBO dust threshold across assets/strategies.
- New venue code copied from AGPL, non-commercial, or proprietary/decompiled sources without a deliberate licensing decision.
- A separate second strategy model in `arbitrageur-strategy::spatial_price_arb` when the production live path is the Tauri monitor/execution pipeline.

## 12. Backlog ordering

### Immediate

1. Arcus dynamic tick tiers and fee-source correction.
2. Exact pre-submit client-order persistence and client-ID recovery.
3. Economic-account deduplication plus breaker-membership correction.
4. Verified leverage admission.
5. Truthful one-unit derivative limit until an inventory mode exists.

### Next

6. Shared coherent executable-plan module.
7. Own-trade book watermark and pair-skew gate.
8. Graceful execution drain.
9. Typed venue errors and protected order budgets.
10. Independent risk freshness clocks.
11. Execute GitHub issue #68.
12. Manual fill adoption.

### After the foundation

13. Explicit cycle/STEP inventory with FIFO reverse unwind.
14. Inventory surcharge.
15. Live autonomous canary graduation.
16. Account/venue-scoped concurrency.
17. Maker-first execution experiment.
18. Evidence-driven new venues only.

## 13. Final assessment

The right response to the external-project review is not to add all proposed features. Arbitrageur already has the more sophisticated statistical monitor and the stronger durable reconciliation posture. Its highest leverage improvement is to make the production interface smaller and deeper: one coherent plan, one durable order lifecycle, one economic-account risk view, and one explicit venue-health model.

Once those seams are in place, STEP inventory and maker-first execution become contained policies rather than new parallel paths. That sequencing improves live safety now and makes later strategy work cheaper to verify.

---

# Part II — second-source review

A second batch of sources was reviewed after Part I: two GitHub repositories (inspected at pinned commits) and six FMZ (发明者量化) pages. None of it changes the Part I ordering; it adds one new gated work item (model-health governance), one small observability increment, and several entries to the anti-pattern catalog.

## 14. Provenance and evidence classes

Evidence classes used below: **A** = inspectable source at a pinned commit; **B** = platform-rendered description only (no source); **C** = self-reported results without reproducible artifacts.

| Source | Artifact | Pin / identifier | License | Code visible | Class |
|---|---|---|---|---|---|
| `vincentkoc/dexscraper` | GitHub repo | `a6c6126` (2026-04-30) | GPL-3.0 | Yes | A |
| `your-quantguy/binance-scalping` | GitHub repo | `e8612d3` (2025-08-05) | MIT | Yes | A |
| EWY 三腿统计套利 43天实盘 | FMZ article | fmz.com/bbs-topic/11007 | n/a | Snippets only | C |
| makebit 套利王 | FMZ strategy | fmz.com/strategy/371264 | Proprietary | No | B |
| Pure-Alpha 中性对冲 | FMZ strategy | fmz.com/strategy/429863 | Proprietary | No | B |
| Binance funding 经典账户版 | FMZ strategy | fmz.com/strategy/520274 | Proprietary | No | B |
| Binance funding 统一账户版 | FMZ strategy | fmz.com/strategy/520275 | Proprietary | No | B |
| OKX funding hedge | FMZ strategy | fmz.com/strategy/520276 | Proprietary | No | B |

All five FMZ strategy pages are marketplace listings: a "Buy" button, vendor contact, marketing copy, and risk notes — no source. The "live robot" links (e.g. `robot/564099`) are vendor-operated dashboards, not independent audits. They are usable only as strategy-design input, never as verified evidence or code.

## 15. Per-source findings

### 15.1 dexscraper — connection engineering is the useful half; the decoder is a cautionary example

What it is: a Python SDK/CLI that scrapes DexScreener's undocumented WebSocket (`wss://io.dexscreener.com/dex/screener/...`) for token discovery, with configurable chain/rank/liquidity/volume/age/FDV filters and OHLC export.

Genuinely good patterns (adopt as concepts — never the code, GPL-3.0):

- **Connection-attempt accounting surfaced as state**: retry counters, backoff stage, and status changes are first-class in `scraper.py`/`config.py` (bounded `max_retries`, capped exponential backoff with ±25% jitter, `ping_interval`/`ping_timeout`, per-connection epoch tracking). Arbitrageur's `feed.rs` already owns the supervisor/backoff/keepalive machinery and does more (instant-kill detection, env-bound sockets, identity-validated ingest); what is missing is *surfacing* those counters on the health panel. Small Phase-2 increment, see §16.
- **Parser fuzz coverage**: `test_edge_cases.py` and `test_scraper.py` exercise malformed binary frames, truncated chunks, empty payloads, and network error paths — a discipline worth matching for our venue adapters' WS parsers.

Negative findings:

- The "binary protocol decoder" is heuristic guessing, not a spec: `protocol.py` tries candidate chunk sizes until one parses, and `enhanced_protocol.py` classifies float64 clusters by plausible value ranges (price by magnitude band, liquidity by magnitude band). Fields it cannot classify become `protocol: "unknown"`, `chain` defaults to `"solana"`, and `_parse_pair_from_cluster` invents `created_at = now()` — fabricated plausible-looking timestamps flowing downstream. This is precisely the anti-pattern our `validate_feed_book` drop-not-fabricate policy guards against; keep it.
- All tests are synthetic — no recorded real frames — so an upstream schema change silently shifts semantics rather than failing loudly.
- The Cloudflare bypass / `Origin`-spoofing header kit is anti-bot evasion aimed at a third-party data site; it is a ToS and fragility surface with no place in authorized venue connectivity.
- GPL-3.0: no code from this repo can enter Arbitrageur's tree under any configuration.

Decisions: **adopt** connection-health telemetry (concept); **reject** the decoder approach and anti-bot evasion; **defer** any DexScreener-derived discovery feed — if ever added it is quarantined to radar watchlist input, never execution-grade data. Scraped aggregate prices are not venue truth.

### 15.2 binance-scalping — clean skeleton, unsafe fill semantics

What it is: a ~730-line one-sided Binance USDⓈ-M scalping loop (not arbitrage, not hedged). It places a post-only `priceMatch='QUEUE'` entry, waits ≤10s on a user-data WS fill event, cancels if unfilled, and on any fill places a reduce-only take-profit limit order. Single process, in-memory `OrderMonitor`, CSV logging, Telegram alerts.

Correct-in-shape patterns (Arbitrageur already implements them more safely): user-data WS events as the primary fill trigger inside a bounded wait; reading `executedQty` from the cancel response; a periodic activity/liveness report.

Unsafe patterns — cataloged as anti-patterns:

- `runbot.py:365–370`: when cancel fails with "already filled", the code sets `filled_qty = config.quantity` and `filled_price = float(order["price"])` — fabricating a fill from *request* parameters rather than venue truth. On `priceMatch` orders `"price"` may be absent entirely, so the except path can raise again.
- `runbot.py:374`: close-leg size reads `order["origQty"]` (requested) instead of `executedQty` (actual).
- Startup reconciliation is partial: `run()` re-reads resting close orders via `get_active_close_orders` at boot, but never reconciles *positions* or missed fills — an entry filled while the bot was down leaves a naked position with no close order, and nothing looks for it (no `fetch_positions`, no fill catch-up).
- `cumulative_pnl += take_profit * quantity` assumes fixed profit and ignores fees.
- Broad `except Exception` around order operations; no typed venue errors; no durable client-order identity; no tests.

Decision: **reject** as an implementation reference. Its only transferable idea — a fill-event-driven enter→confirm→exit loop — is already the shape of our residual-compensation path with durable identity. One cheap idea worth filing: a "no fills in N minutes" liveness alert for the operator channel (folds into the existing alerting issue, low priority).

### 15.3 EWY article — the only deep operational source in the batch

`fmz.com/bbs-topic/11007`: a post-mortem of a three-leg statistical-arbitrage factor — short EWY (Korea ETF) vs long β-weighted Samsung + SK Hynix perp legs, `ln(EWY) = c + β₁·ln(Samsung) + β₂·ln(Hynix) + ε`, harvesting residual ε mean reversion on a z-grid (entries at |z| bands, 10x leverage, 43-day self-reported run). The ROI claim is unverifiable, but the operational narrative is unusually specific and consistent with real failure modes. Four lessons transfer:

1. **Frozen entry model vs. live monitor.** While a position is open, all decisions use the model fitted at entry (β, intercept, residual σ); a *separate* live regression runs purely as a drift monitor. This resolves a real ambiguity we would otherwise hit: an updating β makes z drift for computational, not market, reasons — the author reports a feature deleted because β updates alone crossed exit thresholds and manufactured three-leg trades that paid fees on nothing.
2. **Tiered automated drift response.** Monitor tiers: (i) warn; (ii) drift >50% or β-leg ratio materially changed → forced flatten and re-fit; (iii) **β sign flip → immediate flatten + stop new entries + human**. Their β₂ went negative (a 23%-weight component with negative coefficient is economically nonsensical): the "hedge" leg became a same-direction short — a directional bet on Korean semis, not arb. The factor then died; position caps slowed the loss but could not detect structural invalidation — *"仓位上限拦不住结构失效"*.
3. **Half-life as the ex-ante health metric.** Fit mean reversion to the residual: half-life ≪ max hold = healthy; lengthening half-life = early factor decay; half-life > max hold = systematically forced-out before convergence → auto-delever or stop. We already own the machinery: `arbitrageur-math` has `ou_process::fit` (θ, half-life) and `engle_granger_test` (ADF + half-life). Second-pass correction on usage: OU half-life *is* consumed — `compute_matrix_baselines` fits it per venue pair (with an R² ≥ 0.6 quality gate) to auto-size monitor windows (`commands.rs:3895–3919`), and it backs ad-hoc research commands. What is missing is specifically a *per-position, model-health* use: nothing re-fits the live model against the entry snapshot while a position is open. `engle_granger_test` itself has no production caller.
4. **Idempotent recovery loop with symmetric exit discipline.** Each cycle: enumerate positions → query venue truth (never local cache) → cancel resting orders first → submit only the delta → re-query → confirm flat twice before marking `FLAT_CLEAN`. Exit-side single-leg risk is worse than entry-side. Our bounded flatten children implement most of this; the "cancel-first, submit-only-diff, double-confirm" ordering is the part to keep explicit in the unified lifecycle.

Mapping to Arbitrageur: `stat_arb` is currently a research lab with no order dispatch (`execution.rs:2623`), and `stat_arb_pairs.rs` runs a live (non-frozen) Kalman β with rolling-z thresholds, no persisted model snapshot, no drift monitor, no half-life gate. The math substrate exists; the governance does not.

Decisions: **adopt** model-health governance as a prerequisite for any model-dependent strategy going live (new Phase-1.6 item, §16). **Reject**: the self-reported 43-day ROI as evidence; alert-only drift monitoring (the author names it as the direct amplifier of the terminal loss); reusing mean-reversion z for divergence-direction trades. **Already have**: residual-recovery loop, durable flatten children, feed staleness/reconnect guards (`feed.rs` already treats >N-second staleness, reconnect, REST fallback, private-API backoff).

### 15.4 FMZ paid strategies — descriptions only; a few design confirmations

All five are closed marketplace products. Read as design documentation:

- **Funding trio** (520274 Binance-classic, 520275 Binance-unified, 520276 OKX — near-identical bodies, account-model variants): enter long-spot/short-perp when funding exceeds a threshold; refuse entry on deeply negative basis; iceberg orders since carry entry is latency-insensitive; exit when rate < ~-0.2% rather than bleeding; auto-close on runaway price; diversify ≥30 hedged positions because one -2% tail erases ~200 periods of +0.01%; keep margin headroom on the short leg.
- **makebit 套利王** (371264): multi-venue spot/perp + funding + auto spot-balancing; opens gated on profitable funding direction; strictest margin-mode opening restriction; unbalanced-leg stop-loss; give-up-on-high-latency. Its checklist confirms items already in the Part I plan; nothing new.
- **Pure-Alpha** (429863): basket stat-arb selection via ADF/Engle-Granger/Johansen/Hurst, equal-notional long/short baskets, claims "no risk under black swans". The selection-time statistical-validity gate is adoptable as a research concept; the zero-risk claim is marketing — mean-reversion baskets are exactly what breaks under regime shifts (see EWY).

What Arbitrageur already does better than the funding trio: 8h-normalized cross-venue gaps, empirical gap persistence with Wilson bounds, episode-counted backtest win rates, decay-modeled expected hold hours, depth-walked executable sizing, measured entry-spread cost, Boros YU fixed-yield locking, persisted minute-bucketed rate history (`funding.rs`).

Gaps worth adopting — all deferred behind the unified lifecycle:

- a **rate-flip exit rule** for open carry positions evaluated every scan. Second-pass precision: `scheduled_exits` today serves imported external positions only (`external_positions.rs` is its sole writer); the convergence path has its own auto-exit evaluator (`evaluate_spatial_exit` → converged/stopped/timed-out at `execution.rs:5350+`) which is edge-based, not rate-based. Sharper finding: `funding_auto_exit_spread_pct`, `funding_rotation_threshold_bps`, and `funding_liq_trim_pct` are already plumbed `StrategyParams` fields — validated, persisted, exposed to the frontend — with **zero consumers** (`funding.rs` reads only `funding_min_basis_bps` and `funding_max_entry_spread_bps` for entry gating). The rate-flip exit is therefore a defined-but-unwired knob, which is worse than absent: the UI implies protection that does not exist. Wiring it (or removing it until wired) belongs with the Phase-3 item;
- a **diversification floor + per-venue funding exposure cap** inside `PortfolioRiskSnapshot`;
- a **short-leg margin-headroom check** beyond entry margin;
- **sliced/iceberg carry entry** — deferred to the maker-first phase.

Decisions: **defer** all four funding-policy items to Phase 3 (they are policies on top of the risk snapshot and exit queue, both of which Phase 1/2 must deliver first); **reject** the "low-risk / no-risk" framing as evidence; **defer** basket stat-arb until single-pair stat_arb is live-proven.

## 16. Priority changes from this round

Phase 0 is untouched. Additions:

- **Phase 1.6 (new) — model-health governance.** For any strategy whose admission depends on a fitted model (stat_arb first; funding carry and any future cross-ticker second): persist the entry-model snapshot (β/intercept/residual σ + version) at open; run an independent live drift monitor; wire tiered automated response (warn → block adds → reduce-only → flatten → halt); admission requires *both* a z-sanity bound and a stationarity/half-life condition. Drift must change state, not only log — EWY's post-mortem identifies alert-only monitoring as the terminal-loss amplifier. Gate stat_arb live-enable on this.
- **Phase 2.7 (new) — feed connection telemetry.** Surface per-lane attempt counts, backoff-until, last-connect epoch, and keepalive status on the health surface. `feed.rs` already owns the machinery; this is display + persistence only.
- **Phase 3 policy additions** (folded, not new work items): funding rate-flip exit via `scheduled_exits`; carry diversification floor and per-venue funding cap in `PortfolioRiskSnapshot`; short-leg margin headroom.
- **No change**: venue expansion stays deferred — dexscraper contributes no venue; its data is discovery-grade at most.

## 17. What not to copy (additions)

- Heuristic protocol decoding that fabricates fields (dexscraper's value-range classification, `created_at = now()`). Drop-not-fabricate ingest stays the rule.
- Cancel-failure treated as "already filled" with request-parameter qty/price (binance-scalping `runbot.py:365–374`); `origQty` as fill quantity; profit accounting that ignores fees. Venue truth via `fetch_order` only.
- Alert-only model monitoring with no automated response.
- Anti-bot evasion (Cloudflare bypass, Origin/UA spoofing) for any dependency.
- Marketing performance claims ("low risk", "no risk under black swans", vendor robot dashboards) as input to sizing or configuration.
- Scraped third-party aggregator data anywhere near the execution path.
- GPL-3.0 code inclusion.

## 18. Acceptance criteria (additions)

- stat_arb (or any model-dependent strategy) live-enable requires: a persisted entry-model snapshot; an independent drift monitor with tiered automated response ending in flatten+halt; admission gated on both z-sanity and stationarity/half-life. Drift changes state, never just logs.
- Open carry positions get a rate-flip exit evaluated each scan through `scheduled_exits`, not only entry gating.
- Feed supervisor exposes per-lane connection health (attempts, backoff-until, last-connect epoch, keepalive) on the risk/health surface.
- Portfolio risk enforces a carry diversification floor and per-venue funding exposure cap.
- No code from any Part II source enters the tree; all adoptions are design-level and land behind the unified lifecycle.

## 19. Sources (Part II)

- `github.com/vincentkoc/dexscraper` @ `a6c6126`, GPL-3.0 (inspected clone: `/tmp/dexscraper` — ephemeral; pin by hash).
- `github.com/your-quantguy/binance-scalping` @ `e8612d3`, MIT (inspected clone: `/tmp/binance-scalping` — ephemeral; pin by hash).
- fmz.com/bbs-topic/11007 — 一个因子的生与死：EWY 三腿统计套利 43 天实盘全记录 (article; code snippets only).
- fmz.com/strategy/{371264, 429863, 520274, 520275, 520276} — marketplace listings; description + risk notes only; no source.
- Internal cross-checks: `frontend/src-tauri/src/feed.rs`, `funding.rs`, `execution.rs:2623`, `state_store/exits.rs`; `crates/arbitrageur-strategy/src/stat_arb_pairs.rs`; `crates/arbitrageur-math/src/{cointegration,ou_process,kalman_filter}.rs`.

---

## 20. Implementation status — 2026-09-29 live testnet session

Ran the release build under sustained testnet operation (systemd-managed,
journald + dev-RPC observability). Issues published: **#83** funding
accrual attribution, **#84** radar-vs-execution edge divergence, **#85**
position-integrity watchdog — all `ready-for-agent`.

**Landed (validated against real testnet venues, not simulation):**

- Canonical-base symbol matching across the exposure surface —
  `account_position_for` (exact → canonical-base → ambiguity-error, never
  guesses flat), `system_qty_for`, shared-symbol venue-excess sweep, plus
  `_`-separated spellings in `canonical_base`/`symbol_quote`
  (`BTC_USDT`, `PERP_ADA_USDC`; previously all underscore forms reduced
  wrong). Stranded two radar-leg orphans and a crashed-entry ADA pair were
  found and closed; the next radar entry flattened cleanly end-to-end.
- Orderly/Vanta client-order-id ≤36 bound (readable ledger id, bounded
  wire id).
- `lighter.fetch_orderbook` re-keys numeric market-id books to canonical
  symbols via cached details lookup.
- `acknowledge_signal` (unresolved→failed|closed operator resolution)
  exposed on the dev RPC socket in addition to the Tauri/UI path.
- Ops: desktop runs as `arbitrageur-testnet.service` (cgroup memory caps,
  Restart=on-failure); dev-RPC + sqlite runbook in `docs/RUNBOOK.md §3`.

**Meured evidence for the specced work:**

- #84: production `bot_skip` — `expectedProfitBps +1.61` vs priced
  `−22.66 BPS after route friction` on the same row; phantom `ready`
  routes on un-fetchable venues dominate the qualification set
  (alpaca ×4 routes).
- #85: Lighter testnet reaped account index 423 mid-position → the
  ledger-`balanced` carry was actually a naked HL `−0.00477` short for
  ~7h; resolved only after the operator re-provisioned credentials
  (index 39) and venue truth confirmed the leg vanished.
- #83: the carry crossed a settlement window; no accrual attribution
  exists — realized P&L excludes settled funding entirely.

**Open watch items:** desktop RSS 1.7–4.0G (feed/subscription retention
suspected); `external_positions` RPC exceeds 3min with all venues enabled
(serial account fetches + cooldowns); rotation-exit logs per-cycle
"residual" spam while a venue is unreadable (#85 covers the fix);
`opar-1790583464516` shows ~$112k notional vs ~$3k siblings — mark-sanity
check pending.
