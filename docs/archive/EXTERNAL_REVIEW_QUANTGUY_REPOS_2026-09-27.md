# External Review — your-quantguy repos vs Arbitrageur

**Date:** 2026-09-27
**Subject repos:**
- https://github.com/your-quantguy/cross-exchange-arbitrage
- https://github.com/your-quantguy/perp-dex-tools
- https://github.com/your-quantguy/entropy-arb

**Local clones:** `/tmp/cross-exchange-arbitrage`, `/tmp/perp-dex-tools`,
`/tmp/entropy-arb` (shallow; ephemeral — re-clone if gone).

**Companion doc:** `EXTERNAL_REVIEW_PANDAZHAI_2026-09-27.md` (same template).

**Key difference vs PandaZhai:** these are **real Python source** — every
claim below cites inspectable code in the clone. Confidence is higher;
still, read the cited lines when cross-checking rather than trusting my
paraphrase.

Every "do we have it" claim cites Arbitrageur files/lines verified against
the working tree. `[UNVERIFIED]` marks claims needing deeper tracing.

---

## 1. entropy-arb — highest-value repo

Single-pair arb bot: Entropy (Hyperliquid dex namespace) vs a hedge venue
(Lighter mainnet / Lighter-Robinhood / tradexyz). ~3k LOC. Config-as-strategy
(`config.example.yaml`), no paper mode — live or `--record-only` only.

### 1.1 Marginal-depth slicing (`book.py:86-113`, `154-192`)

`crossable_base` walks both books level-by-level and returns the qty where
**every marginal slice** clears `fees + threshold`:

```
b_px*(1-sell_fee) < a_px*(1+buy_fee)*(1+threshold)  → stop walking
```

Slice = `min(q_max * take_fraction, cap_notional / top_ask)`. Buy/sell
limits are set at the **deepest level consumed** (`walk_depth`), and the
plan reports `marginal_premium_bps` — the premium at the worst unit, not the
average.

**Ours [VERIFIED DIFFERENT]:** `execution.rs:1077-1175` checks aggregate
depth vs `min_depth_ratio` plus IOC/VWAP bounds — an *average-price* check.
Theirs guarantees the *marginal unit* is profitable net of fees. Strictly
stricter. **Adopt:** walk-to-marginal pricing in our planner, or at minimum
report marginal premium alongside VWAP in the opportunity DTO.

### 1.2 Inventory ladder (`engine.py:226-247`)

Once a venue's position passes `floor_frac` of its cap **in the adding
direction**, the required premium gets a linear surcharge ramping to
`scale_bps` at 100% of cap. Soft-brake on one-sided accumulation.

**Ours:** hard grid cap (`max_accumulated_units`, `execution.rs:812-851`).
**Adopt as option:** graduated hurdle is smoother than a cliff — a position
at 90% of cap still accepts exceptional signals instead of hard-refusing.

### 1.3 Per-direction persistence arming (`engine.py:385-394`)

Edge must persist continuously for `premium_persist_sec`; the armed
timestamp resets whenever the edge disappears. **Ours:** persistence-fraction
over a window (soak-observed: `persistence 0.09 < 0.20`). Functionally
equivalent; theirs is simpler to reason about. **No change needed.**

### 1.4 Post-trade book suppression (`engine.py:376-379`)

```
if book.last_update_ts <= venue.last_traded_ts: skip
```

Never re-evaluates a book snapshot that predates our own fill — the stale
book still shows liquidity we just consumed → phantom re-fire.

**Ours [UNVERIFIED]:** books are freshness-gated (`stale_ms`) but not gated
against our own trade timestamps — grepping `monitor.rs`/`execution.rs` for
`last_traded`/`predate` found nothing. Execution re-fetches books before
dispatch (`validate_adapter_book`) so a stale signal can't directly execute —
the residual is wasted evals + misleading monitor rows, not phantom fills.
Verify before treating as a gap.

### 1.5 Order-send budget + reactive exclusion (`engine.py:97-111`, `470-478`)

Sliding-60s deque of order sends per venue (`max_orders_per_min`,
e.g. Lighter budget 30 under a ~40 ceiling); on `RATE_LIMITED` error or
**margin rejection** → venue excluded for `rate_limit_pause_sec`.

**Ours [VERIFIED PARITY+]:** `VenueLane` rolling-60s request budget +
exponential backoff for **read** traffic (`monitor.rs:1230-1289`), and
`AccountRequestCoordinator` has a **separate signed/order throttle bucket**
(`crates/arbitrageur-exchanges/src/rate_limiter.rs:18-50`). Whether order
*sends* carry an explicit per-venue/min cap vs just pacing: `[UNVERIFIED]` —
the coordinator throttles interval, not count. Worth adding an explicit
orders/min budget on venues with hard send ceilings (Lighter ~40/min).

Margin rejection → venue pause: **gap candidate** — check what our execution
does on a margin/insufficient-collateral rejection (probably just fails the
dispatch; theirs pauses the venue proactively).

### 1.6 Venue outage state machine (`engine.py:611-634`)

≥3 consecutive `fetch_position` failures → `venue_down` → trading paused;
probe every `venue_probe_sec`; on recovery → log + auto-resume entries.

**Ours:** related to issue #68 (eligibility ledger) but at **venue scope** —
their per-venue down/probe/recover loop is the venue-level analog. Our
backoff skips reads but `[UNVERIFIED]` whether a "venue down → pause all
trading on pairs touching it → probe → resume" state machine exists as such.

### 1.7 Reconcile grace window (`engine.py:578-606`)

`RECONCILE_GRACE_SEC = 5.0`: a venue that traded <5s ago is **skipped** by
chain position reads — REST position lags WS settlement, so a read could
"restore" a stale position and trigger phantom hedges. Reconcile also holds
the venue lock so it can't race an in-flight order.

**Ours [VERIFIED DIFFERENT MODEL]:** position truth is **ledger-derived**;
venue position reads inform risk display, and the only adoption path is
operator-initiated `adopt_manual_position` (`auto_exec.rs:411-486`). We never
auto-adopt a venue position read into trading state → their bug class can't
hit the ledger. Residual: a stale venue position read could still feed the
risk overview — bounded impact. **Low priority; document, don't build.**

### 1.8 Shared-account equity dedup (`engine.py:159-162`)

If the hedge venue rides the same account address (e.g. another HL dex
namespace), set `include_core_equity = False` — don't double-count shared
equity.

**Ours [VERIFIED NOT APPLICABLE TODAY]:** no two adapters share one account —
`dex_perps.rs` venues are independent venue identities; `hyperliquid.rs` is
the only HL adapter. **Precondition warning:** the day we add an HL-dex-
namespace venue (Entropy, tradexyz) or a second Lighter-family venue sharing
credentials, equity/position reads must dedupe by underlying account. Add
this to the adapter-onboarding checklist.

### 1.9 Order-path TLS keepalive (`engine.py:682-693`)

`http_keepalive_sec` pings each venue's order endpoint to keep the
connection warm — removes TCP+TLS handshake latency from the critical send
path.

**Ours [UNVERIFIED]:** reqwest connection pooling keeps idle connections
alive opportunistically, but there's no deliberate order-path warmer. Cheap
micro-latency win for live entries; also useful as a venue liveness probe.

### 1.10 Per-venue locks (`engine.py:56-58`, `325-326`)

Execution holds **both venue locks**; reconcile holds **one** — a chain read
can never race an in-flight order on that venue, but pairs on disjoint
venues execute concurrently.

**Ours [VERIFIED DIFFERENT]:** `state.execution_lock` is a **single global
mutex** (`state.rs:1453`; held at `execution.rs:854`, `4062`, `5025`). All
executions serialize. Simpler/safer; costs concurrency on disjoint pairs.
**Decision point:** keep global serialization (safer, probably intentional)
or shard per symbol-pair for throughput. Flag for explicit decision, not a
bug.

### 1.11 Other details worth noting

- `asyncio.shield` on in-flight executions — shutdown never cancels
  mid-execution; both legs settle (`engine.py:329-332`, `210-214`). Verify
  our shutdown path waits on in-flight signals similarly.
- `--record-only` + `tools/analyze.py`: collect 1-min book bars, derive
  `midline/upper/lower` from percentiles. **Parity:** our `autotune` module
  does percentile-based threshold derivation in-app (`commands.rs:3958+`).
- CSV trade journal with `exp_edge_usd` vs `fill_edge_usd` per execution —
  expected-vs-realized edge accounting. Do we log expected edge at dispatch
  to compare against realized? `[UNVERIFIED]` — cheap honesty metric.

---

## 2. cross-exchange-arbitrage — maker-first leg execution

edgeX post-only + Lighter taker hedge (`strategy/edgex_arb.py` 727 LOC,
`order_manager.py` 284 LOC). Educational-grade code; two good ideas, two
antipatterns.

### 2.1 Maker-first leg pattern (the borrowable idea)

- Place **post-only** order on the maker venue priced one tick *inside* the
  spread (`best_ask − tick` for buy — improves the bid when spread > 1 tick,
  `order_manager.py:101-120`).
- If unfilled in `fill_timeout` (5s) → cancel and re-place (chase loop,
  `order_manager.py:131-162`).
- On fill → immediately fire taker hedge on the other venue with a slippage-
  capped marketable limit (`best_bid*0.998`/`best_ask*1.002`,
  `order_manager.py:194-201`).

**Value:** avoids taker fee on the maker leg and may earn rebates — material
on fee-asymmetric pairs (e.g. 0bps-maker venues). Cost: leg risk between
maker fill and taker hedge (ms-scale) + adverse selection on the maker leg.

**Ours:** all entries are simultaneous taker IOCs. A **maker-first execution
mode** would slot in as an alternative leg policy: place post-only on the
designated maker venue → on fill, taker-hedge the other leg → on hedge
failure, our existing unwind machinery handles the naked leg. This is a real
strategy-mode candidate for pairs where one venue is structurally
maker-fee-free. [UNVERIFIED: whether our venue adapters expose `post_only`
order flags — check `OrderRequest` fields in `arbitrageur-core/order.rs`.]

### 2.2 ANTIPATTERNS — do not copy

- `order_manager.py:240-249`: on 30s hedge-fill timeout → `"marking order as
  filled to continue trading"` — **fabricates a fill** on a guess. We are
  explicitly fail-closed here (unknown order → blocked, never assumed
  filled). Keep it that way; this is the cautionary tale.
- `edgex_arb.py:599-602`: `abs(net_position) > 2*qty` → `sys.exit(1)` —
  hard-kill on imbalance. Our circuit-breaker + blocked-ticker semantics are
  strictly better (halt, don't crash).

### 2.3 Minor observations

- Positions refreshed via REST before each entry (`edgex_arb.py:569-590`) —
  we do account-freshness gates instead; equivalent posture.
- Position diff watched continuously (`get_net_position`) — parity with our
  ledger delta-neutrality checks.

---

## 3. perp-dex-tools — venue hedge-cyclers (points farming)

Eight per-venue `hedge/hedge_mode_*.py` bots (backpack/extended/apex/grvt/
edgex/nado/standx + Lighter hedge leg, ~10k LOC) plus a single-venue
`trading_bot.py` open→close-for-profit grid loop. Primary purpose is
**volume/points farming** at minimal cost, not pure arb — but it carries a
few production details worth borrowing.

### 3.1 In-place order modification (`hedge_mode_grvt_v2.py:857-891`)

Lighter `modify_order` by `client_order_index` — amend a resting order's
price in place instead of cancel+replace. Preserves queue position where the
venue price-times orders, and costs one request instead of two.

**Ours [GAP CANDIDATE]:** `grep -iE "amend|modify_order" lighter.rs` → none.
If any future strategy rests maker orders (see §2.1), amend-in-place support
in the Lighter adapter is the efficient primitive. No current strategy needs
it — mark as prerequisite for maker-mode work, not standalone value.

### 3.2 Lighter WS book integrity (`hedge_mode_grvt_v2.py:420-436`)

Offset-sequence validation on deltas; on gap → request fresh snapshot.

**Ours [VERIFIED PARITY]:** `ws.rs:320-324` `lighter_delta_continuous` —
`begin_nonce == last_nonce`, gap → resubscribe, never interpolate.

### 3.3 Dust-level BBO filtering (`hedge_mode_grvt_v2.py:438-453`)

BBO for hedge decisions ignores top levels with `size*price < $4000` —
filters sub-economic top-of-book dust before computing the hedgeable price.

**Ours:** `min_depth_ratio` guards *execution* depth; a per-level notional
floor for the *signal-side* BBO is a different (cheaper) sanity filter.
**Adopt as option:** `min_level_notional` when computing executable BBO —
blocks "edge" that lives only in a $50 top level.

### 3.4 Position-vs-open-orders mismatch check (`trading_bot.py:~395-402`)

Periodic check: `abs(position − active_close_amount) > 2*qty` → alert
"manually rebalance." Parity with our reconciliation, but the shape is nice:
**compare open-order quantity against position quantity** for reduce-only
coverage — detects an orphaned position with no covering exit order. Verify
our scheduled-exit machinery already catches "position with no exit order."

### 3.5 Telegram + Lark notifier helpers (`helpers/`)

Parity with `ARBITRAGEUR_ALERT_WEBHOOK`; not worth porting.

---

## 4. Consolidated action list (merged with PandaZhai doc §6)

New items from this review, priority-ordered against the PandaZhai list:

1. **[NEW, HIGH] Marginal-depth pricing** (§1.1): guarantee the worst unit
   clears fees+threshold; report `marginal_premium_bps` in opportunities.
   Test: multi-level book where VWAP passes but marginal level fails.
2. **[PandaZhai §4.1] Bounded exit-recovery** — unchanged, still top
   safety item.
3. **[NEW, MEDIUM] Inventory ladder** (§1.2): optional linear hurdle
   surcharge past `floor_frac` of cap — softens the cliff of
   `max_accumulated_units`.
4. **[NEW, MEDIUM] Post-trade book suppression** (§1.4): don't evaluate
   books older than our last fill on that venue — verify first whether our
   model needs it (fresh re-fetch on dispatch may already cover).
5. **[NEW, MEDIUM] Order-send budget + margin-rejection venue pause**
   (§1.5): explicit sends/min cap per venue; treat margin rejection as
   pause-worthy.
6. **[NEW, LOW] Maker-first execution mode** (§2.1): strategy-mode decision;
   prerequisite = `post_only` flags + adapter `amend` (§3.1). Real fee-edge
   value on maker-fee-free venues; adds leg-risk machinery.
7. **[NEW, LOW] Order-path keepalive** (§1.9): warm TLS on order endpoints.
8. **[NEW, LOW] `exp_edge` vs `fill_edge` accounting** (§1.11): log expected
   edge at dispatch to diff against realized — one field, honest P&L audit.
9. **[Precondition] Shared-account equity dedup** (§1.8): no action today;
   required when adding account-sharing adapters (HL dex namespaces).
10. **[NEW, LOW] Dust-level BBO floor** (§3.3): `min_level_notional` in
    executable-BBO computation.

---

## 5. Cross-checker's risky-claims list

1. **§1.4** — "no post-trade book suppression": searched `last_traded`,
   `predate`, `own.*trade.*book` in monitor.rs/execution.rs only; the concept
   may live under another name (e.g. inside feed-lane bookkeeping).
2. **§1.5** — "order sends lack an explicit count budget": verified read
   budget + signed-bucket pacing exist; did not trace whether dispatch paths
   enforce a sends/min cap. Margin-rejection → pause similarly untraced.
3. **§1.6** — "no venue-down state machine": verified VenueLane backoff is
   per-symbol-fetch; a venue-level pause/probe may exist elsewhere
   (`feed.rs`, `state.rs` health tracking) — check `venues_health` paths.
4. **§2.1** — `post_only` flag availability across adapters not verified.
5. **§3.1** — "no amend support": grep'd lighter.rs only; other adapters or
   a generic amend trait may exist in `arbitrageur-core/order.rs`.
6. **§1.10** — confirmed `execution_lock` is one global `Mutex<()>`; whether
   a second, finer-grained lock structure also exists alongside it was not
   traced (e.g. per-symbol locks inside dispatch).
7. All PandaZhai-doc §7 caveats apply — this doc is additive, not a
   replacement.
