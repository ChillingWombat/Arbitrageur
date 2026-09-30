# Audit Report: Options, Deribit & Reconciliation Subsystem

All findings below are verified against the codebase and corroborated against the live SQLite ledger (`~/.arbitrageur/state.sqlite3`) and `journald` logs.

---

## A. Why `feeUsd` on all Deribit legs in `signal_executions` (and `fees` in `execution_log`) is `0.0`

Every one of the 28 Deribit legs in `signal_legs` has `fee_usd = 0.0`, and every Deribit row in `execution_log` (`exec-options-240..245`, `exec-opkg-242..243`, `exec-flatten-244-*`) has `fees = 0.0`. There are **three compounding root causes**:

1. **`DeribitAdapter::place_order` (`crates/arbitrageur-exchanges/src/deribit.rs:1276-1281`) and `order_response` (`deribit.rs:759-767`) ignore the `trades` array on `/private/buy` & `/private/sell`, and `fetch_order` (`deribit.rs:1326-1375`) only queries `/private/get_order_state`:**
   - `/private/buy` and `/private/sell` return `{"order": {...}, "trades": [{"fee": ..., "fee_currency": "BTC"|"ETH"|"USDC", ...}]}` inside `result`.
   - `place_order` (line 1277) extracts only `let o = v.get("order").cloned()...` and passes `&o` to `order_response`.
   - `order_response` (lines 759–767) only inspects `o.get("commission")`:
     ```rust
     let fee = dec_from_json(o.get("commission")).and_then(|c| {
         if c < dec!(0) {
             return None;
         }
         match meta.settlement_currency.as_str() {
             "USD" | "USDC" | "USDT" => Some(c),
             _ => index_usd.map(|i| c * i),
         }
     });
     ```
   - It never sums `v.get("trades")[*].get("fee")` when `o.get("commission")` is `0` or absent, and `fetch_order` (lines 1338–1374) only calls `/private/get_order_state?order_id=...` (never falling back to `/private/get_user_trades_by_order` when `commission` is zero/missing). Furthermore, if an order gets a maker rebate (`commission < 0`), `if c < dec!(0) { return None; }` discards it to `None` -> `0.0`.
   - Note also: on Deribit Testnet (`test.deribit.com`), accounts with default/VIP-level equity (this testnet account has ~$11.06M equity: ~100 BTC + ~1,000 ETH) or default testnet fee schedules often report `commission: 0` on the order object unless trade-level fees or fee schedules apply.

2. **`execute_options_parity_impl` (`frontend/src-tauri/src/options_parity.rs:894-905`) and `execute_options_package_impl` (`frontend/src-tauri/src/options_packages.rs:1319-1330`) skip `apply_leg_fill` whenever `ledger_px_usd` returns `None`:**
   - After all legs fill, the executor loops over `leg_fills` and calls:
     ```rust
     let Some(px) = crate::execution::ledger_px_usd(state, venue, &leg_sym, f.price) else {
         tracing::warn!(
             "options_parity: no USD underlying mark for {leg_sym} — deferring fill record to reconciler"
         );
         continue;
     };
     let _ = state.state_store.apply_leg_fill(&f.sig_id, f.leg_side, f.qty, px, f.fee, Some(&f.order_id));
     ```
   - Because `BTC-PERPETUAL` / `ETH-PERPETUAL` is not in `state.market_data_hub` (see Section B), `ledger_px_usd` returns `None` for option legs, so `apply_leg_fill` (along with `f.fee`, `f.qty`, `f.price`) is **skipped outright**!

3. **`execute_options_*_impl` then immediately flips `in_flight` legs to `SignalStatus::Balanced` (`options_parity.rs:943-952`, `options_packages.rs:1365-1374`), preventing `reconcile_pending_orders` from ever backfilling the skipped leg fills:**
   - Despite the warning saying `"deferring fill record to reconciler"`, lines 943–952 (`options_parity.rs`) and 1365–1374 (`options_packages.rs`) immediately mark every `InFlight` leg as `SignalStatus::Balanced`.
   - Because `list_pending_order_legs` (`frontend/src-tauri/src/state_store/signals.rs:568`) filters `WHERE e.status = 'in_flight'`, the reconciler **never polls a `Balanced` signal**. Any leg that the reconciler did not happen to race-poll mid-cycle before `update_signal_status(..., Balanced)` ran is stranded in SQLite with `filled_qty = 0.0, fill_price = NULL, fee_usd = 0.0` permanently (confirmed in SQLite on `ojel-1790706351731-2ba126f4-leg4..leg7`).

---

## B. Why `unrealizedPnlUsd` in `get_open_positions` is `null` for all open `options_parity` and `options_jelly_roll` positions on Deribit

In `get_open_positions` (`frontend/src-tauri/src/execution.rs:2897-2931`), `state_store::aggregate_open_positions` (`frontend/src-tauri/src/state_store/mod.rs:703-759`) computes `unrealized_pnl_usd` by calling the `mark(venue, symbol)` closure for each filled leg (`execution.rs:2909-2926`):
```rust
let raw = engine
    .market_data_hub
    .get_valid_orderbook(&VenueId::from(venue), symbol, Some(MARK_READ_TTL))
    .and_then(|b| b.mid_price().or(b.mark_price))
    .and_then(|m| dec_to_f64(m, "mark mid").ok());
let Some(p) = raw else {
    tracing::warn!("open_positions mark miss: {venue}/{symbol} — no fresh hub book");
    return None;
};
let out = ledger_px_usd(engine, venue, symbol, p);
```
In `aggregate_open_positions` (`state_store/mod.rs:722-729`), if `mark(&leg.venue, &leg.symbol)` returns `None` for **any** leg in the group, `pnl` for the entire position is poisoned to `None`. Furthermore, for any Deribit option leg, `ledger_px_usd` (`execution.rs:248-277`) **also** requires a fresh `MarketDataHub` orderbook for `{under}-PERPETUAL` (`BTC-PERPETUAL` or `ETH-PERPETUAL`) within `MARK_READ_TTL` (120s, `execution.rs:221`).

Why `MarketDataHub` never has fresh books for Deribit options or `BTC-PERPETUAL` / `ETH-PERPETUAL` (5 root causes):

1. **No Deribit WebSocket feed lane (`frontend/src-tauri/src/feed.rs:50`):**
   - `FEED_VENUES` only includes `["hyperliquid", "lighter", "robinhood", "vanta"]`. Deribit has no WS lane feeding `MarketDataHub`.

2. **Options scanners and executors never write fetched Deribit books into `MarketDataHub`:**
   - `scan_options_parity_impl` (`options_parity.rs:318-354`), `execute_options_parity_impl` (`options_parity.rs:661-678`), `scan_options_packages_impl` (`options_packages.rs:403-439`), and `execute_options_package_impl` (`options_packages.rs:1098-1113`) call `adapter.fetch_orderbook(...)` and `deribit.fetch_book_summaries_by_currency(...)`, but **never call `state.market_data_hub.update_orderbook(...)`**.

3. **`canonical_base` does not strip `-PERPETUAL` (`crates/arbitrageur-core/src/symbol.rs:53`), breaking `monitor.rs` held-symbol matching for `BTC-PERPETUAL` / `ETH-PERPETUAL` (`frontend/src-tauri/src/monitor.rs:1938-1944, 1988, 2028-2031`):**
   - `CLASS_SUFFIXES` in `symbol.rs:53` is `&["-PERP", "-SWAP", "-SPOT", "_PERP", "_SWAP", "_SPOT"]` — it omits `"-PERPETUAL"`.
   - Consequently, `canonical_base("BTC-PERPETUAL")` and `pair_key("BTC-PERPETUAL", merge_usd)` return `"BTC-PERPETUAL"` (not `"BTC"`).
   - In `monitor.rs:1938-1944`, `held_by_venue["deribit"]` inserts `pair_key(&leg.symbol, merge_usd)` = `"BTC-PERPETUAL"`.
   - Meanwhile, in `monitor.rs:1988`, `fetch_symbol` keys markets by `pair_key(&m.symbol.base, merge_usd)`, which for `BTC-PERPETUAL` (`m.symbol.base == "BTC"`, `deribit.rs:845`) is `"BTC"`.
   - Then `monitor.rs:2028-2031` filters:
     ```rust
     let held: HashSet<String> = held_for_venue
         .into_iter()
         .filter(|k| fetch_symbol.contains_key(k))
         .collect();
     ```
     Because `fetch_symbol` has `"BTC"` (not `"BTC-PERPETUAL"`), **`"BTC-PERPETUAL"` and `"ETH-PERPETUAL"` are filtered out of `lane.held`!**

4. **Dated Deribit futures clobber `BTC-PERPETUAL` / `ETH-PERPETUAL` in `monitor.rs` `by_key` (`deribit.rs:840-857`, `monitor.rs:2006-2015`):**
   - In `DeribitAdapter::fetch_markets` (`deribit.rs:845`), **all** `kind == "future"` instruments (both `BTC-PERPETUAL` and dated futures like `BTC-27MAR26`, `BTC-26JUN26`, etc.) are constructed via `Symbol::crypto_perp(&base, &quote)` with `base = "BTC"`, `quote = "USD"`, and `raw = name`.
   - In `monitor.rs:2006-2015`, the tie-breaker for `by_key["BTC"]` checks:
     ```rust
     let dominated = match by_key.get(&key) {
         None => false,
         Some((_, cur_q)) => {
             rank_of(cur_q) <= rank_of(&quote) && !venue_symbol.ends_with("-PERP")
         }
     };
     ```
   - Because `"BTC-PERPETUAL"` ends with `"-PERPETUAL"` (not `"-PERP"`), `!venue_symbol.ends_with("-PERP")` is `true`, so `dominated` is `true`! Whichever dated future Deribit returns first (`e.g. BTC-27MAR26`) wins `by_key["BTC"]`. When the monitor polls `"BTC"`, it fetches `BTC-27MAR26` and stores it in `MarketDataHub` under `hub_key("BTC-27MAR26")` = `"BTC-27MAR26"`, **never populating `"BTC-PERPETUAL"` in `MarketDataHub`**.

5. **Deribit's ~1,500+ option instruments flood `VenueLane.symbols` (`deribit.rs:846`, `monitor.rs:1303, 1407-1434, 1845`):**
   - `DeribitAdapter::fetch_markets` returns every active BTC and ETH option (~1,500+ symbols). All of them enter `lane.symbols` on a lane whose `venue_budget("deribit")` (`monitor.rs:1303`) is `60/min` (`usable_per_min = 45/min`), taking **~33 minutes** per round-robin cycle vs `MARK_READ_TTL = 120s`.
   - Even for option symbols whose names do match `fetch_symbol`, `maybe_refresh_watchlist` (`monitor.rs:1845, 1934`) only rebuilds `lane.held` once per `watchlist_refresh_secs` (default **3600s = 1 hour**), so newly opened option legs are not added to `lane.held` for up to an hour after entry.

### How `get_open_positions` (and Deribit mark caching) should provide live marks:
1. **Write fetched books to `MarketDataHub` in options scanners & executors:**
   - In `execute_options_parity_impl` (`options_parity.rs:661-678`) and `execute_options_package_impl` (`options_packages.rs:1098-1113`), call `state.market_data_hub.update_orderbook(...)` for `perp_book`, `call_book`, `put_book`, and `fresh_books` (and in `scan_options_parity_impl` / `scan_options_packages_impl`, seed `MarketDataHub` from the fetched `perp` / option books or summaries).
2. **Fall back to `last_good_accounts` (or on-demand book/summary fetch) in `get_open_positions` (`execution.rs:2909-2926`):**
   - `DeribitAdapter::fetch_account` (`deribit.rs:1089-1143`) already fetches `/private/get_positions` for BTC and ETH and populates `account.positions` with `p.mark_price` **already in USD** (`mark * index_usd` for options, `mark` for futures, lines 1110–1117), cached in `state.last_good_accounts` under `"{env}-deribit"`.
   - If `MarketDataHub` lacks a fresh book for `(venue, symbol)` in `get_open_positions`, falling back to `state.last_good_accounts` (or fetching the missing book / Deribit book summary before calling `aggregate_open_positions`) immediately supplies live USD marks for all open option and perp legs.
3. **Fix `monitor.rs` & `symbol.rs` for Deribit:**
   - Add `"-PERPETUAL"` and `"_PERPETUAL"` to `CLASS_SUFFIXES` in `crates/arbitrageur-core/src/symbol.rs:53` (or handle `-PERPETUAL` in `monitor.rs:2012`: `!(venue_symbol.ends_with("-PERP") || venue_symbol.ends_with("-PERPETUAL"))`).
   - In `DeribitAdapter::fetch_markets` (`deribit.rs:840-857`), do not emit dated futures (`name != format!("{currency}-PERPETUAL")`) with `base = "BTC"` / `MarketType::Perpetual` so dated futures cannot clobber `BTC-PERPETUAL` / `ETH-PERPETUAL`.
   - Refresh `lane.held` on every monitor tick (or whenever open signals change) instead of only once per hour inside `maybe_refresh_watchlist`.

---

## C. Exact Mechanics of the Deribit `BTC-PERPETUAL` / `ETH-PERPETUAL` Rounding & Reconciliation Bugs

### C.1. `wire_base_qty` -> `place_order` double-flooring (`(amount / lot).floor() * lot`) loses a $10 / $1 lot
- **Step 1 (`wire_base_qty`, `crates/arbitrageur-exchanges/src/deribit.rs:1381-1395`):**
  - Called at `options_parity.rs:712` (`adapter.wire_base_qty(sym, qty_dec)`).
  - Fetches `ref_px_1 = self.mark_price(&instrument).await?` (line 1386).
  - Computes USD notional `amount_1 = qty * ref_px_1` (`to_venue_amount`, line 666).
  - Snaps down to the lot size (`$10` for `BTC-PERPETUAL`, `$1` for `ETH-PERPETUAL`):
    `let snapped = (amount_1 / lot).floor() * lot;` (line 1390).
  - Converts back to base units (`from_venue_amount`, line 679):
    `snapped_base = snapped / ref_px_1` (line 1394).
- **Step 2 (`place_order`, `crates/arbitrageur-exchanges/src/deribit.rs:1185-1204`):**
  - Called at `options_parity.rs:747` with `order.quantity = snapped_base` (`= snapped / ref_px_1`).
  - Fetches `ref_px_2 = self.mark_price(&instrument).await?` a **second** time (line 1188).
  - Computes `amount_2 = order.quantity * ref_px_2 = (snapped / ref_px_1) * ref_px_2` (`to_venue_amount`, line 1192).
  - Floors **again** at line 1203:
    `amount = (amount / lot).floor() * lot;`
- **Why it drops a full lot ($10 on BTC, $1 on ETH):**
  1. **Even if `ref_px_2 == ref_px_1`**, 28-digit `rust_decimal::Decimal` division `snapped / ref_px_1` truncates the repeating decimal, so `(snapped / ref_px_1) * ref_px_1 = snapped - 1e-24` (e.g., `(83480 / 83492) * 83492 = 83479.999999999999999999999998`). Then `(83479.999999999999999999999998 / 10).floor() * 10 = 83470` — losing a $10 lot purely from Decimal round-trip truncation!
  2. **If `ref_px_2 < ref_px_1`** by even a single tick between `wire_base_qty` and `place_order`, `(snapped / ref_px_1) * ref_px_2 < snapped`, so `.floor()` again drops a full $10 / $1 lot.
- **Confirmed in SQLite:**
  - `opar-1790706281162-26ddf9ca-leg2` (`BTC-PERPETUAL`): `wire_base_qty` snapped to `$83,480` (`intended_qty = 0.9998816623044613`), then `place_order` double-floored to `$83,470` (`filled_qty = 0.9997365017007618 = 83470 / 83492.0`).
  - `opar-1790707047137-17a5a918-leg2` (`ETH-PERPETUAL`): `wire_base_qty` snapped to `$2,691` (`intended_qty = 0.9996805183033293`), then `place_order` double-floored to `$2,690` (`filled_qty = 0.9995728321230244`).

### C.2. `place_order` on `reduce_only` orders floors `$2685.63` to `$2685` instead of rounding to `$2686`, leaving a 1-lot residual
- **Mechanics:**
  1. In `DeribitAdapter::fetch_account` (`deribit.rs:1113`), a Deribit future position with USD `size = $2686` and mark `mark_t1 = 2690.32` is reported in base units as `p.quantity = size / mark_t1 = 2686 / 2690.32 = 0.9983942430640221` ETH.
  2. In `flatten_positions_inner` (`frontend/src-tauri/src/execution.rs:5380`), `let basis = dispatchable.min(p.quantity);` selects `qty = 0.9983942430640221` (`= 2686 / mark_t1`).
  3. `flatten_leg` (`execution.rs:4239-4253`) submits a `reduce_only = true` market order with `order.quantity = 0.9983942430640221`.
  4. `DeribitAdapter::place_order` (`deribit.rs:1188-1204`) fetches a fresh mark `mark_t2 = 2689.95` (which ticked down $0.37 from `2690.32`), computes `amount = 0.9983942430640221 * 2689.95 = 2685.63059...`, and unconditionally floors at line 1203:
     `amount = (amount / lot).floor() * lot;` -> **`$2685`**!
  5. That leaves a **`$1.00` (1-lot) residual** open on `ETH-PERPETUAL` (confirmed on `flat-opar-1790706309374-666fefa1-leg2-2bfc` in SQLite: `intended_qty = 0.9983942430640221`, `filled_qty = 0.9981598170969721 = 2685 / 2689.95`).
  6. **Compounding dust-threshold mismatch (`execution.rs:5714` vs `options_parity.rs:607`):**
     - Immediately after the close, `flatten_positions_inner` (`execution.rs:5714`) checks `let is_dust = wire_qty == Decimal::ZERO || notional_usd < 10.0;`. Because `$1.00 < $10.00` (even though `ETH-PERPETUAL`'s minimum lot on Deribit is **$1.00**, not $10.00!), `flatten_positions_inner` logs `"remaining venue position ... is untradeable dust ($1.00) — closed under emergency flatten"` and marks the signal `Closed`!
     - Then in `execute_options_parity_impl` (`options_parity.rs:607`), the pre-entry check only tests `adapter.wire_base_qty(leg_sym, p.quantity) == Decimal::ZERO`. Whenever `mark_t2 >= mark_t1`, `(1.0 / mark_t1) * mark_t2 >= 1.0`, so `wire_base_qty` returns `1.0 / mark_t2 > 0` and **blocks new ETH parity entries** (`"An open position already exists for ETH-PERPETUAL (qty 0.00037167...); flatten it before cycling"` in `journald` at `04:36:44`); whenever `mark_t2 < mark_t1` (or Decimal division truncates `1.0 / mark * mark` to `0.9999999999999999999999999998`), `wire_base_qty` floors to `0` and logs `"ETH-PERPETUAL carries a sub-minimum residual position; proceeding"` (`04:37:26`)!

### C.3. `reconciliation.rs` marks a 100%-filled Deribit future order as `PartialTerminal` -> `Unresolved`
- **Mechanics (`crates/arbitrageur-exchanges/src/deribit.rs:740-743` & `frontend/src-tauri/src/reconciliation.rs:106-127, 519-538`):**
  1. In `DeribitAdapter::order_response` (`deribit.rs:740-743`):
     ```rust
     let filled = Self::from_venue_amount(meta, filled_v, avg_px.or(ref_price_usd));
     let original = Self::from_venue_amount(meta, amount, avg_px.or(ref_price_usd));
     ```
     For a 100%-filled Deribit future order (`filled_v == amount`), `resp.status == OrderStatus::Filled`, `resp.filled_qty == resp.original_qty == amount / avg_fill_price`, and `resp.remaining_qty == dec!(0)`.
  2. However, `leg.intended_qty` in SQLite was recorded before submission from `wire_base_qty` (`= snapped_1 / pre_submit_mark_price`, `options_parity.rs:712, 745`).
  3. When `reconcile_pending_orders` polls the order (`reconciliation.rs:519-520`), `classify_response(&resp, intended)` (`reconciliation.rs:115-127`) compares `resp.filled_qty` (`= snapped_2 / avg_fill_price`) against `intended` (`= snapped_1 / pre_submit_mark_price`) using `materially_short(filled, intended)` (`filled + 0.000001 < intended`, line 106).
  4. Whenever `avg_fill_price > pre_submit_mark_price` (normal when buying at the ask) **and/or** `snapped_2 = snapped_1 - $10` (due to C.1 double-flooring), `resp.filled_qty < intended` by more than `1e-6`, so `classify_response` returns **`LegPoll::PartialTerminal`** even though `resp.status == OrderStatus::Filled`, `resp.filled_qty == resp.original_qty`, and `resp.remaining_qty == 0`!
  5. Then the sub-wire dust check at `reconciliation.rs:529-538` runs:
     ```rust
     if poll == LegPoll::PartialTerminal && resp.remaining_qty <= Decimal::ZERO {
         let shortfall = intended - resp.filled_qty;
         if shortfall > Decimal::ZERO {
             if let Ok(wire) = adapter.wire_base_qty(&leg.symbol, shortfall).await {
                 if wire <= Decimal::ZERO {
                     poll = LegPoll::Filled;
                 }
             }
         }
     }
     ```
     In `opar-1790706281162-26ddf9ca-leg2` (`BTC-PERPETUAL`), `intended = 0.9998816623044613` (`$83,480 / 83489.88`) and `resp.filled_qty = 0.9997365017007618` (`$83,470 / 83492.0`). The combined shortfall is `0.0001451606` BTC = **`$12.12` >= `$10.00` (`min_amount`)**!
  6. Therefore `adapter.wire_base_qty("BTC-PERPETUAL", shortfall)` returns `10 / mark > 0`, `poll` stays `LegPoll::PartialTerminal`, and `derive_signal_status` (`reconciliation.rs:226-237`) escalates the 100%-filled order to **`SignalStatus::Unresolved`** (`"Leg(s) ended partially filled or with mixed terminal states"`), permanently blocking `BTC-PERPETUAL`!

### C.4. Race condition between `reconcile_pending_orders` and `execute_options_parity_impl` / `execute_options_package_impl`
- **Mechanics:**
  1. `execute_options_parity_impl` (`options_parity.rs:552`) and `execute_options_package_impl` (`options_packages.rs:984`) hold `state.execution_lock`, whereas `reconcile_pending_orders` (`reconciliation.rs:299`) only acquires `state.reconcile_lock` (`try_lock`) and runs every `POLL_INTERVAL_SECS = 5s` (`reconciliation.rs:43`).
  2. Inside the sequential leg loop (`options_parity.rs:691-870` for 3 legs; `options_packages.rs:1138-1298` for up to 8 legs), each leg is persisted into `signal_executions` as a **1-leg signal (`leg_count = 1`)** in `SignalStatus::InFlight` with its `client_order_id` and `venue_order_id` **before** later legs are dispatched (`options_parity.rs:738-792`, `options_packages.rs:1184-1233`).
  3. While the executor is still submitting later legs (or polling `fetch_account()` at `options_parity.rs:921` / `options_packages.rs:1343`), the 5s background `reconcile_pending_orders` task queries `list_pending_order_legs` (`signals.rs:549-581`), sees the earlier `in_flight` 1-leg signals (`leg0`, `leg1`, ...), polls Deribit, and immediately transitions those 1-leg signals to `SignalStatus::Balanced` (`"All legs reached terminal filled state via venue poll"`) or `SignalStatus::Unresolved` (`reconciliation.rs:584`)!
  4. **Confirmed in SQLite:**
     - `ojel-1790706351731-2ba126f4`: `leg0..leg3` have `note = "All legs reached terminal filled state via venue poll"` (written by `reconcile_pending_orders` mid-cycle), while `leg4..leg7` have `note = "options package cycle completed — legs remain open until expiry/flatten"` (written by `execute_options_package_impl` at cycle end).
     - `opar-1790706281162-26ddf9ca`: `leg0` and `leg1` were flipped to `Balanced` by the reconciler while `leg2` was flipped to `Unresolved` mid-cycle.
  5. **Critical safety hazard on partial-cycle failure (`options_parity.rs:934-972`, `options_packages.rs:1356-1394`):**
     - At the end of the leg loop, the executor updates each leg's signal status:
       ```rust
       let current = state
           .state_store
           .get_signal(id)
           .ok()
           .flatten()
           .map(|s| s.status);
       if current != Some(SignalStatus::InFlight) {
           continue;
       }
       ```
     - If a later leg (e.g. `leg2` in parity or `leg7` in a box/jelly-roll) **fails** (`cycle_complete == false`), the executor intends to mark all earlier filled legs (`0..filled_count`) as `SignalStatus::Imbalanced` so `dispatch_residual_flatten` (`reconciliation.rs:928`) will unwind the naked legs.
     - Because `reconcile_pending_orders` already raced in and flipped `leg0..legN-1` from `InFlight` to `Balanced` while later legs were still executing, `if current != Some(SignalStatus::InFlight) { continue; }` **skips them**! Those unhedged legs remain `SignalStatus::Balanced` forever and are **never marked `Imbalanced` and never auto-flattened**!

---

## D. Additional Logic, Math, and Safety Bugs Found in `options_parity.rs`, `options_packages.rs`, `options_bot.rs`, and `deribit.rs`

1. **Unhedged naked exposure left behind when `reconcile_pending_orders` races a failing multi-leg options cycle (`options_parity.rs:939-962`, `options_packages.rs:1361-1384`):**
   - Even if `reconcile_pending_orders` is prevented from polling mid-cycle (e.g. by holding `reconcile_lock` or gating on cycle completion), if `cycle_complete == false`, the status update loop in `options_parity.rs:939` / `options_packages.rs:1361` should **always** force filled legs of an incomplete cycle to `SignalStatus::Imbalanced` even if their status was already `Balanced` (only skipping `Closed`), AND `execute_options_parity_impl` / `execute_options_package_impl` never calls `dispatch_residual_flatten` or unwinds the filled legs inline when a later leg fails!
   - Wait: look at `reconciliation.rs:949-966` (`dispatch_residual_flatten`):
     Let's check whether `dispatch_residual_flatten` even runs on `options_parity` or `options_jelly_roll`!
     In `options_parity.rs:953-960` and `options_packages.rs:1375-1383`, failed partial cycles mark filled legs `SignalStatus::Imbalanced` with note `"options parity cycle aborted after this leg filled — manual flatten required"`. If `engine.auto_resolve_residual` is enabled, `dispatch_residual_flatten` (`reconciliation.rs:949`) will attempt to flatten `Imbalanced` signals — **unless** they were prematurely flipped to `Balanced` by the C.4 race!

2. **Permanent loss of `filled_qty` and `fill_price` on `Balanced` option legs (`options_parity.rs:894-905, 943-952` and `options_packages.rs:1319-1330, 1365-1374`):**
   - As detailed in A.2/A.3, when `ledger_px_usd` returns `None` (because `BTC-PERPETUAL` / `ETH-PERPETUAL` is not in `MarketDataHub`), `execute_options_*_impl` skips `apply_leg_fill` (leaving `filled_qty = 0.0, fill_price = NULL` in `signal_legs`) and then immediately sets the signal status to `SignalStatus::Balanced`.
   - Consequences:
     1. `reconcile_pending_orders` never polls `Balanced` signals, so `filled_qty` stays `0.0` and `fill_price` stays `NULL` forever.
     2. `get_open_positions` (`state_store/mod.rs:716`, `if !(leg.filled_qty.is_finite() && leg.filled_qty > 0.0) { continue; }`) **completely omits** those open legs from the UI Open Positions table!
     3. `flatten_positions_inner` (`execution.rs:5288-5304`, `let entry_qty = ledger_qty_to_dec(leg.filled_qty, ...)?; ... if residual <= dec!(0) { continue; }`) **skips any leg with `filled_qty == 0.0`**, so clicking "Clear / Flatten" on that trade leaves those Deribit option legs open on the venue while marking the signal `Closed`!
   - **Fix:** `execute_options_parity_impl` already has `index: Decimal` in scope (`options_parity.rs:647`), and `execute_options_package_impl` already has `index: Decimal` in scope (`options_packages.rs:1084`). Convert option fill prices directly using `f.price * index_f64` (or fallback to `ledger_px_usd`) so `apply_leg_fill` is **never** skipped on a filled leg, AND push the fetched `perp_book` / option books into `state.market_data_hub.update_orderbook(...)`.

3. **Pre-execution position check in `execute_options_package_impl` (`options_packages.rs:1022-1049`) does not deduplicate shared instruments across legs when checking or logging, and lacks the sub-minimum residual check present in `options_parity.rs:604-619`:**
   - More importantly, in `execute_options_package_impl` (`options_packages.rs:1013-1020`):
     `ensure_instrument_not_blocked(&state.state_store, &leg.instrument, state.env().as_str())` is called per leg. If an existing `options_jelly_roll` is already open in `Balanced` status at the same strikes/expiries, `ensure_instrument_not_blocked` only blocks `InFlight | Imbalanced | Unresolved` (`execution.rs:487`), so `ensure_instrument_not_blocked` passes, and then `deribit.account_position_for(&account, &leg.instrument)` (line 1036) catches the open position.
   - However, in `options_bot.rs:492-575` (`tick_package`), when `open_positions > 0` for a substrategy (e.g. `jelly_roll` with `max_positions = 2`), the bot scans `scan_options_packages_impl` and picks `candidates.first()` (`options_bot.rs:523`). If `candidates.first()` uses the **same option instruments** as the already-open position #1, `execute_options_package_impl` fails at line 1038 (`"An open position already exists for {instrument}"`), and the bot never tries `candidates[1]`! `tick_package` (`options_bot.rs:523`) and `tick_parity` (`options_bot.rs:425`) should filter out candidates whose leg instruments already have open signals/positions when `max_positions > 1`.

4. **`DeribitAdapter::fetch_funding_rate` (`crates/arbitrageur-exchanges/src/deribit.rs:977-1022`) treats Deribit's `funding_8h` / `current_funding` as a raw 8h fraction without checking units, and hardcodes `next_funding_time = now + 8h`:**
   - Line 1018 sets `next_funding_time: now + chrono::Duration::hours(8)` instead of Deribit's actual 08:00 / 16:00 / 00:00 UTC boundary (or continuous funding).

5. **`DeribitAdapter::order_response` (`crates/arbitrageur-exchanges/src/deribit.rs:742-743`) uses `avg_px` for `original_qty` (`Self::from_venue_amount(meta, amount, avg_px.or(ref_price_usd))`):**
   - When a Deribit USD-contract future order is **partially filled** (`0 < filled_v < amount`), `original_qty` is computed as `amount / avg_px`, while before any fill (`avg_px == None`) `original_qty` was `amount / ref_price_usd`.
   - Worse, when `reconcile_pending_orders` checks `resp.remaining_qty` (`(original - filled).max(0)`), that part is `> 0` for partial fills, **but** for a 100% filled order (`filled_v == amount`), `original_qty` (`amount / avg_px`) differs from `leg.intended_qty` (`amount_pre / mark_pre`), which is the exact mismatch that triggers C.3 in `reconciliation.rs` (`classify_response(&resp, intended)`).
   - In `reconciliation.rs:115-127`, `classify_response` should treat an order with `resp.status == OrderStatus::Filled && resp.remaining_qty == Decimal::ZERO && resp.filled_qty > Decimal::ZERO && resp.filled_qty == resp.original_qty` (or within sub-wire tolerance) as `LegPoll::Filled` when the venue itself reports the entire order amount filled.