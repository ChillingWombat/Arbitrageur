# Comprehensive External Audit & Architectural Proposals

**Date:** 2026-09-27  
**Scope:** Systematic code-level audit of four prominent open-source / decompiled arbitrage frameworks:
1. [`lihanyu81/PandaZhai-Arbitrage-Tool`](https://github.com/lihanyu81/PandaZhai-Arbitrage-Tool) (`panda-four` v0.5.24 PyInstaller reverse-engineered bytecode)
2. [`your-quantguy/entropy-arb`](https://github.com/your-quantguy/entropy-arb) (Hyperliquid Entropy ↔ Lighter/TradeXYZ statistical basis arbitrage)
3. [`your-quantguy/cross-exchange-arbitrage`](https://github.com/your-quantguy/cross-exchange-arbitrage) (EdgeX ↔ Lighter Maker-Taker arbitrage framework)
4. [`your-quantguy/perp-dex-tools`](https://github.com/your-quantguy/perp-dex-tools) (Multi-DEX suite: Backpack, Paradex, GRVT, Extended, ApeX, Nado, StandX, Ethereal)

**Target Codebase:** [`Arbitrageur`](file:///mnt/Data/Projects/Arbitrageur)  
**Authors:** Antigravity (AI Assistant) & David (Project Lead)  
**Status:** Approved Architectural Specification

---

## 1. Executive Summary & Comparative Matrix

Across the four audited repositories, we examined real-world production trading infrastructure operating on live DEX perps. While `Arbitrageur` maintains a decisive structural lead in **execution speed, language safety (Rust vs Python), concurrency, and breadth across CeFi and DeFi**, these repositories contain **hard-won operational heuristics, mathematical models, and venue-specific fixes** derived from millions of dollars of live order flow.

### Global Capability Comparison

| Dimension / Mechanism | Arbitrageur (Current) | PandaZhai (`four-arbitrage`) | your-quantguy (`entropy-arb`) | your-quantguy (`perp-dex-tools` & `cross-ex`) | Recommended Action for Arbitrageur |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Language & Runtime** | **Rust (Tokio, Decimals, zero-cost)** | Python 3.12 (PyInstaller binary) | Python 3.10+ (asyncio) | Python 3.8–3.12 (asyncio) | **Retain Rust:** Superior latency, deterministic memory, and safety. |
| **Strategy Paradigm** | Pairwise spatial & stat-arb | $N$-venue combinatorial mesh | Two-venue midline basis arb | Maker-Taker post-only + hedge | **Adopt:** Mesh generator, Midline basis, and Maker-Taker mode. |
| **Spread Hurdle Model** | Static target BPS + fees + slippage | Rolling mean excess BPS ($\Delta$) | Empirical `midline_bps` + asymmetric upper/lower | Fixed price threshold | **Adopt:** Midline offset ($\mu$) + dynamic inventory ladder surcharge. |
| **Phantom Tick Filter** | Min spread persistence in monitor | N/A | **300ms Persistence Gate (`_armed`)** | Timeout checks | **Adopt:** Microsecond persistence gate before order dispatch. |
| **Book Staleness Filter** | `stale_ms` heartbeat check | 15s quiet book timeout | **Post-trade book staleness barrier** | REST fallback | **Adopt:** Invalidate books timestamped $\le \text{last\_trade\_ts}$. |
| **Depth Walk / Sizing** | Top-of-book / VWAP slippage | Top-of-book depth walk | **Simultaneous multi-level crossing (`crossable_base`)** | Fixed clip | **Adopt:** Simultaneous level-by-level depth walking with `take_fraction`. |
| **Inventory Skew Control**| Position caps per ticker | Signed STEP inventory ($\pm \text{Steps}$) | **Inventory Ladder Surcharge (`_inv_add_bps`)** | Grid step spacing | **Adopt:** Dynamic linear inventory surcharge on one-sided builds. |
| **Manual Recovery** | Automated unwind / passive loop | **Interactive Candidate Fill Adoption** | Net-delta auto-hedge | Auto-hedge on fill callback | **Adopt:** Frontend UI modal for manual fill discovery & adoption. |
| **Feed Resilience** | WS ping/pong heartbeat | **120s periodic snapshot resync** | Connection frame liveness (`alive_ts`) | WS auto-reconnect | **Adopt:** 120s forced snapshot re-subscription. |
| **Venue Coverage** | 18+ CeFi/DeFi venues | 8 DEX Perps | 3 DEX Perps | 11 DEX Perps (Backpack, Ethereal, etc.) | **Add:** Native Backpack (Ed25519) and Ethereal (EIP-712). |

---

## 2. Critical Bug Fix Discovered: Arcus Dynamic Tick Tiers

### 2.1 The Issue in Arbitrageur
In [`crates/arbitrageur-exchanges/src/arcus.rs`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/arcus.rs#L49-L57):
```rust
struct ArcusMarketSpec {
    market_id: u64,
    tick_size: Decimal,
    step_size: Decimal,
}
```
Currently, `ArcusAdapter::fetch_markets` extracts only the static top-level `tickSize` from `/v1/markets`. Our price quantization logic (`snap_to_tick` and `exact_units`) enforces that all order prices must be an exact integer multiple of this single `tick_size`.

### 2.2 PandaZhai's Production Fix (`arcus.py: limit_price`)
Arcus markets feature **dynamic tiered tick sizes (`tickTiers`)**. A single market can have different price tick increments depending on the price level:
```json
"tickTiers": [
    { "upToPrice": "100.0", "tick": "0.01" },
    { "upToPrice": "1000.0", "tick": "0.10" },
    { "tick": "1.00" }
]
```
If an order is placed on an Arcus market whose current price falls into a tier with a different tick increment, Arcus's matching engine rejects the order immediately with:
> *"Arcus price/quantity is not an exact multiple of its step"*

### 2.3 Proposed Fix in Arbitrageur
```rust
#[derive(Clone, Debug, PartialEq)]
pub struct ArcusTickTier {
    pub up_to_price: Option<Decimal>,
    pub tick: Decimal,
}

// In ArcusMarketSpec:
pub struct ArcusMarketSpec {
    pub market_id: u64,
    pub tick_size: Decimal, // fallback
    pub tick_tiers: Vec<ArcusTickTier>,
    pub step_size: Decimal,
}

// In snap_to_tick:
pub fn resolve_arcus_tick(spec: &ArcusMarketSpec, price: Decimal) -> Decimal {
    if spec.tick_tiers.is_empty() {
        return spec.tick_size;
    }
    for tier in &spec.tick_tiers {
        match tier.up_to_price {
            Some(upper) if price < upper => return tier.tick,
            None => return tier.tick,
            _ => continue,
        }
    }
    spec.tick_size
}
```

---

## 3. Top Algorithmic Innovations to Borrow

### Proposal 1: The Midline Offset Formulation (`entropy-arb`)

#### The Problem
Cross-exchange price premiums between DEX perps (e.g. Hyperliquid vs Lighter, or Lighter vs Robinhood-Lighter) are **rarely centered at zero**. Differences in underlying collateral (USDC vs USDG), differing index oracle price feeds, and venue structural demand cause the equilibrium spread to sit persistently at $+5\text{ bps}$ or $-12\text{ bps}$.

A conventional arbitrage bot that triggers only when $\text{Spread} > 0$ will:
1. Fire one direction constantly until its capital/position limit is hit.
2. Never unwind because the spread never crosses $0$ to the negative side.
3. Lock up capital in dead inventory with zero turnover.

#### The Solution: Empirical Midline & Asymmetric Entry Bands
From `entropy-arb`:
```
premium_bps = (Price_A / Price_B - 1) * 10,000

                          ┌──────────────  SELL A + BUY B
midline + upper  ───────────────────────────────────────────
                                        ▲
midline          ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┼ ─ ─  Equilibrium Center (e.g. +5.0 bps)
                                        ▼
midline − lower  ───────────────────────────────────────────
                          └──────────────  BUY A + SELL B
```
- **Hurdle for Selling A / Buying B:**
  $$\text{Executable Premium}_{A/B} \ge \text{midline\_bps} + \text{upper\_bps} + \text{Fees}$$
- **Hurdle for Buying A / Selling B:**
  $$\text{Executable Premium}_{A/B} \le \text{midline\_bps} - \text{lower\_bps} - \text{Fees}$$

**Key Insight:** If the midline is $+5\text{ bps}$ and `lower_bps` is $3\text{ bps}$, the buy hurdle is $+2\text{ bps}$. Buying A at $+2\text{ bps}$ is $3\text{ bps}$ cheaper than its equilibrium, successfully unwinding an earlier short opened at $+8\text{ bps}$! A full round-trip captures:
$$\text{Net Profit} \ge \text{upper\_bps} + \text{lower\_bps} \quad \text{(net of all fees)}$$

---

### Proposal 2: The Persistence Gate (Phantom Tick Suppressor)

#### The Problem
In high-frequency WebSocket environments, packet arrival jitter across independent network connections creates artificial, transient price spikes (e.g. 10–50ms) where Exchange A updates before Exchange B. Firing immediately into these one-tick blips results in:
- High execution failure rates (the liquidity was already gone or was an artifact of network timing).
- Unbalanced fills and unnecessary taker fee burn.

#### The Solution (`entropy_arb/engine.py: _scan`)
```python
# Disassembled from entropy_arb/engine.py
armed = self._armed.get(dkey)
if armed is None:
    # First time the edge is detected: arm the trigger timestamp
    self._armed[dkey] = now
    self._schedule_poke(cfg.premium_persist_sec)
    continue

if now - armed < cfg.premium_persist_sec:
    # Edge is present but has not persisted long enough
    self._schedule_poke(cfg.premium_persist_sec - (now - armed))
    continue

# Edge survived >= premium_persist_sec (e.g. 300ms) -> execute!
```

**Proposed Implementation in Arbitrageur:**  
Add an atomic persistence tracker in [`crates/arbitrageur-strategy/src/spatial_price_arb.rs`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-strategy/src/spatial_price_arb.rs). A signal is only dispatched if it remains continuous for $\ge \text{persist\_duration}$ (e.g. 200–300ms).

---

### Proposal 3: Dynamic Inventory Ladder Surcharge

#### The Problem
Hard position caps are binary: below the cap you trade normally, at the cap you abruptly stop. This causes inventory to slam into limits and stay stuck there.

#### The Solution (`entropy_arb/engine.py: _inv_add_bps`)
Introduce a dynamic surcharge that linearly ramps up the required profit hurdle as position utilization exceeds a floor fraction:
$$u = \frac{|\text{Position}| \times \text{Price}}{\text{Cap}_{\text{USD}}}$$
For a trade that **adds** to the position:
$$\text{Surcharge (bps)} = \begin{cases} 
0, & u \le \text{floor\_frac} \\
\text{scale\_bps} \times \frac{u - \text{floor\_frac}}{1.0 - \text{floor\_frac}}, & u > \text{floor\_frac}
\end{cases}$$
**Asymmetric Property:** If the trade **reduces** inventory (e.g. buying when short or selling when long), $\text{Surcharge} = 0$.

**Impact:** As a venue's inventory fills up, the bot demands progressively fatter spreads to add more risk, while simultaneously making it much easier to execute unwinding trades.

---

### Proposal 4: Post-Trade Book Staleness Barrier

#### The Problem
When a taker order is placed and filled, your own transaction consumes liquidity on the exchange orderbook. Because WebSocket trade and orderbook delta messages have network latency (e.g. 20–100ms), the strategy loop might evaluate the orderbook again *before* the exchange pushes the updated book. This causes the bot to attempt to fill against liquidity that **it just consumed itself**, resulting in immediate rejections or unexpected slippage.

#### The Solution (`entropy_arb/engine.py: _scan`)
```python
# Never refire into books that predate the venue's own last trade
if (buy.book.last_update_ts <= buy.last_traded_ts or 
    sell.book.last_update_ts <= sell.last_traded_ts):
    continue
```
In `Arbitrageur`, record `last_traded_ts` on the venue adapter. Reject opportunity evaluation if `book.timestamp <= last_traded_ts`.

---

### Proposal 5: Simultaneous Multi-Level Depth Crossing (`crossable_base`)

#### The Problem
Naive arbitrage algorithms check only top-of-book (Level 0: bid 0 / ask 0). If the top level has tiny liquidity (e.g. $0.001$ BTC), the bot either trades an insignificantly small size or sizes up and incurs massive multi-level slippage.

#### The Solution (`entropy_arb/book.py: crossable_base` & `plan_arb`)
The algorithm walks both order books (asks on Venue A, bids on Venue B) simultaneously level by level:
1. Calculates available quantity at each level pair: $\text{take} = \min(a_{\text{rem}}, b_{\text{rem}})$.
2. Verifies whether the marginal slice at that depth is still profitable:
   $$P_{\text{bid}} \times (1 - \text{fee}_B) \ge P_{\text{ask}} \times (1 + \text{fee}_A) \times (1 + \text{threshold})$$
3. Continues accumulating depth until the marginal profit drops below the threshold, returning $Q_{\text{max}}$.
4. Sizes the order conservatively:
   $$\text{Target Qty} = \min(Q_{\text{max}} \times \text{take\_fraction}, \frac{\text{Cap}_{\text{notional}}}{P_{\text{ask}}})$$
   *(where $\text{take\_fraction} = 0.5$ leaves a safety margin against racing market participants)*.
5. Computes exact depth-walked price limits: `buy_bound` and `sell_bound`.

---

### Proposal 6: Asymmetric Maker-Taker Arbitrage (`cross-exchange-arbitrage`)

#### The Problem
Pure double-taker arbitrage pays two taker fees (often 2 to 10 bps total). In competitive markets, the gross spread rarely exceeds two taker fees + slippage.

#### The Solution (`edgex_arb.py`)
1. **Maker Leg (Venue A):**
   - Places a **Post-Only limit order** inside the spread on Venue A (e.g. $\text{Best Ask} - 1\text{ tick}$ for Buy, or $\text{Best Bid} + 1\text{ tick}$ for Sell).
   - Pays $0$ fee (or earns a maker rebate).
2. **Execution & Event Detection:**
   - Listens to the Venue A WebSocket order update feed.
   - If the order times out without fill (e.g. after 5 seconds), it cancels the order.
3. **Instant Taker Hedge (Venue B):**
   - As soon as Venue A reports `FILLED` (or `CANCELED` with `cumMatchSize > 0`), the bot immediately fires an IOC/Market order on Venue B for the exact filled size.
4. **Net Economics:**
   - Pays only **one** taker fee instead of two.
   - Captures half the bid-ask spread on Venue A.

---

### Proposal 7: Interactive Manual Fill Adoption / Candidate Picker UI (`PandaZhai`)

#### The Problem
In cross-venue trading, one leg can occasionally fail (e.g. margin spike, nonce race, exchange engine maintenance). Operators often step in and execute the hedge manually on the exchange web interface. However, the bot's internal state machine remains stuck in `unresolved` or `imbalanced`, because it has no record of the manual fill.

#### The Solution (`panda_four/manual_recovery.py`)
1. **Candidate Discovery API (`GET /manual_candidates`):**
   - Scans the failed venue's recent trade history in a window $[T_{\text{order}} - 60\text{s}, T_{\text{order}} + 60\text{s}]$.
   - Identifies candidate fills matching: target symbol, required hedge direction, and approximate size.
   - Filters out any fills already bound to existing cycles.
2. **Preview & Confirm Modal in Desktop UI:**
   - Operator selects the matching fill from a dropdown.
   - UI previews the reconstructed cycle PnL, execution price, and fees.
   - Operator clicks "Adopt Fill": the bot binds the fill ID to the open signal, writes it to the ledger with tag `status: manual_recovery`, clears the error lock, and returns to a healthy paused/running state.

---

### Proposal 8: New Venue Expansion Opportunities

From `perp-dex-tools`, two rapidly growing DEX ecosystems stand out as high-priority additions for `Arbitrageur`:

1. **Backpack Exchange (`backpack.py`)**:
   - High-liquidity perps and spot built on Solana/Rust infrastructure.
   - Authentication uses standard Ed25519 signing (`instruction={action}&timestamp={ts}&window=5000`).
   - Native WebSocket feed: `wss://ws.backpack.exchange` (`account.orderUpdate.{symbol}`).
   - Fast REST execution with keep-alive HTTP connections.

2. **Ethereal DEX (`ethereal.py`)**:
   - EVM appchain (Chain ID: 5064014) designed for high-throughput perps.
   - Uses EIP-712 typed data signatures for subaccount linked-signer order routing.
   - Endpoints: `https://api.ethereal.trade`, `wss://ws.ethereal.trade`.

---

## 4. Architectural Roadmap for Arbitrageur

### Phase 1: Immediate Reliability & Bug Fixes (Week 1)
- [ ] **Arcus Dynamic Tick Tiers**:
  - Update [`crates/arbitrageur-exchanges/src/arcus.rs`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/arcus.rs) to parse `tickTiers` from Arcus `/v1/markets`.
  - Implement tiered tick resolution in `snap_to_tick`.
  - Add comprehensive unit tests verifying price quantization across multiple tiers.
- [ ] **Post-Trade Book Staleness Barrier**:
  - Add `last_traded_ts` tracking to `ExchangeAdapter`.
  - Suppress signal evaluation if `book.timestamp <= last_traded_ts`.

### Phase 2: Signal & Microstructure Hardening (Week 2)
- [ ] **Persistence Gate in Strategy Evaluator**:
  - Integrate a microsecond persistence gate (200–300ms) in [`SpatialPriceArbStrategy`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-strategy/src/spatial_price_arb.rs) to filter phantom tick crosses.
- [ ] **Midline Offset & Asymmetric Hurdles**:
  - Add `midline_bps`, `upper_bps`, and `lower_bps` parameters to spatial and basis arbitrage strategies.
- [ ] **Simultaneous Multi-Level Depth Crossing**:
  - Implement `crossable_base` level-by-level book walking in `arbitrageur-math`, supporting fractional depth taking (`take_fraction = 0.5`).

### Phase 3: Risk & Inventory Management (Week 3)
- [ ] **Dynamic Inventory Ladder Surcharge**:
  - Implement `_inv_add_bps` ramp logic based on position capacity utilization to automatically disincentivize one-sided inventory accumulation.
- [ ] **120-Second WebSocket Snapshot Resynchronization**:
  - Add periodic re-subscription timers to WebSocket feed lanes in `arbitrageur-exchanges` to guard against silent book drift.

### Phase 4: Operational Tooling & Advanced Modes (Week 4+)
- [ ] **Interactive Manual Fill Adoption**:
  - Implement Tauri command `get_manual_fill_candidates` and `adopt_manual_fill`.
  - Build UI dialog in the desktop frontend to allow operators to bind external manual exchange fills into stuck position cycles.
- [ ] **Maker-Taker Arbitrage Strategy Module**:
  - Create `MakerTakerArbStrategy` in `arbitrageur-strategy` for post-only placement on primary legs with instant taker hedges on secondary legs.
- [ ] **Backpack & Ethereal Exchange Adapters**:
  - Implement native Rust adapters for Backpack (Ed25519) and Ethereal (EIP-712).
