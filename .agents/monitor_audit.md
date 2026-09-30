# Audit Report: Spread Monitor, Radar Bot (`OpportunityExecutor`), Passive Unwind & Circuit Breaker

Below are the exact root causes, file paths, and line numbers across all four investigation areas (`A`, `B`, `C`, `D`), backed by direct inspection of the Rust codebase and `~/.arbitrageur/state.sqlite3`.

---

## Area A: `frontend/src-tauri/src/monitor.rs` — Ready vs. Non-Executable & `"win probability below 1% floor"`

### 1. Formatting bug in `execution_gate` (`frontend/src-tauri/src/monitor.rs:4204-4211`)
```rust
// frontend/src-tauri/src/monitor.rs:4204-4211
match p_win {
    Some(p) if p >= p_min => {}
    Some(_) => {
        return (
            false,
            Some(format!("win probability below {p_min:.0}% floor")),
        )
    }
```
* **Root cause:** `p_min` (`cfg.min_win_probability`, passed at line `3974`) is a unit-interval probability in `[0.0, 1.0]` (default `0.50` in `RadarBotConfig::default()` at `crates/arbitrageur-strategy/src/convergence_auto.rs:146`, or `0.93` in `MonitorConfig`), **not** a percentage in `[0, 100]`.
* Formatting `0.5` with `{p_min:.0}%` rounds `0.5` to `1` and renders `"win probability below 1% floor"` (and for `0.3` renders `"0% floor"`). Compare with `crates/arbitrageur-strategy/src/convergence_auto.rs:359`, which correctly formats `cfg.min_p_win * 100.0`:
  ```rust
  format!("win probability {:.0}% below {:.0}% floor", p * 100.0, cfg.min_p_win * 100.0)
  ```

### 2. Why `trade_win_probability` (`monitor.rs:1092-1136`) + `gate_p` (`monitor.rs:3968`) masks `funding_unknown`, `exit_uncostable`, and `optimal_size_usd == None`
* In `trade_win_probability` (`frontend/src-tauri/src/monitor.rs:1092-1136`):
  * Lines `1117-1119`: returns `Some(0.0)` if `dev < entry_thresh_bps`
  * Lines `1123-1125`: returns `Some(0.0)` if `funding_unknown || exit_uncostable`
  * Lines `1126-1131`: returns `Some(0.0)` if `optimal_size_usd.map(|s| !(s.is_finite() && s > 0.0)).unwrap_or(true)`
  * Lines `1132-1134`: returns `Some(0.0)` if `!variation_ok || !ratio_ok || !coverage_ok || !session_conf_ok || session_blocked`
* Then at `frontend/src-tauri/src/monitor.rs:3967-3978`:
  ```rust
  // Trade decision probability gates execution; statistical p_win is fallback.
  let gate_p = win_probability.or(p_win);
  let (executable, block_reason) = Self::execution_gate(
      state,
      &symbol,
      status,
      signal_active,
      gate_p,
      cfg.min_win_probability,
      funding_unknown,
      exit_uncostable,
      optimal,
      session_block,
  );
  ```
* Because `win_probability` is `Some(0.0)` (not `None`), `win_probability.or(p_win)` evaluates to `Some(0.0)`.
* Inside `execution_gate` (`frontend/src-tauri/src/monitor.rs:4177-4259`), `match p_win` at lines `4204-4218` runs **before**:
  * `if funding_unknown` (`line 4231`: `"funding adjustment unknown — hold cost unverified"`)
  * `if exit_uncostable` (`line 4234`: `"exit depth insufficient for two-way close"`)
  * `if optimal.map(|s| s <= 0.0).unwrap_or(true)` (`line 4240`: `"no profitable capacity at current book depth"`)
* Whenever `optimal` is `None` (no profitable capacity tier), `funding_unknown` is `true`, or `exit_uncostable` is `true`, `trade_win_probability` returns `Some(0.0)`, which trips `Some(p) if p < p_min` at line `4206` and returns `"win probability below 1% floor"`. **Lines `4231-4242` in `execution_gate` are unreachable dead code whenever `win_probability` is `Some(0.0)`.**

### 3. Why a pair gets `PairStatus::Ready` (`status: "ready"`) when `optimal_size_usd` is `None` or `p_win < cfg.min_win_probability` (`monitor.rs:3781-3835`)
* Look at the status transition logic in `frontend/src-tauri/src/monitor.rs:3817-3835`:
  ```rust
  let status = if !signal_active {
      if dev > 0.0 {
          PairStatus::Watching
      } else {
          PairStatus::Idle
      }
  } else if exit_uncostable
      || !variation_ok
      || p_win.is_none()
      || funding_unknown
      || depth_curve.is_empty()
      || !session_conf_ok
      || !ratio_ok
      || !coverage_ok
  {
      PairStatus::Triggered
  } else {
      PairStatus::Ready
  };
  ```
* Three gates that block `trade_win_probability` / `execution_gate` are **missing** from the `PairStatus::Triggered` branch (and from `gate_reasons` at lines `3781-3816`):
  1. **No profitable capacity (`optimal.is_none()` / `optimal <= 0.0`)**: `depth_curve.is_empty()` is only `true` when the raw orderbooks have zero depth (`capacity_ladder` at lines `1162-1215` pushes tiers even when `net_bps <= 0.0`, setting `optimal_size_usd = None` when no tier has both `net_bps > 0.0` and `marginal_p_shrunk >= min_win_probability`). Thus `depth_curve` is non-empty while `optimal` is `None`.
  2. **`p_win` below floor (`p_win.map(|p| p < cfg.min_win_probability).unwrap_or(true)`) or `p_win_lower` below floor (`p_win_lower.map(|l| l < cfg.min_win_lower_bound).unwrap_or(true)`)**: line `3825` only checks `p_win.is_none()`, not whether `p_win < cfg.min_win_probability` or whether `win_probability` (`trade_win_probability`) is `<= 0.0`.
  3. **`session_block.is_some()`**: not checked in `PairStatus::Triggered`.
* As a result, a pair with `signal_active == true` and `depth_curve` non-empty but `optimal == None` (or `p_win = Some(0.25) < 0.50`) is assigned `PairStatus::Ready` (`status: "ready"`), while `trade_win_probability` returns `Some(0.0)` and `execution_gate` marks it `executable: false` with `block_reason: Some("win probability below 1% floor")`.

### 4. Secondary bug in `execution_gate`: wrong `strategy_id` in `ticker_reduce_only` (`monitor.rs:4251-4257`)
* At `frontend/src-tauri/src/monitor.rs:4251-4257`:
  ```rust
  if state
      .state_store
      .ticker_reduce_only(symbol, "futures", state.env().as_str())
      .unwrap_or(true)
  {
      return (false, Some("ticker is reduce-only — exits only".into()));
  }
  ```
* Monitor opportunities execute under strategy `"radar"` (`monitor_ipc.rs:298`, `auto_exec.rs:55` `BOT_STRATEGY_ID = "radar"`, `auto_exec.rs:744`, and `execution.rs:1118`), not `"futures"`. Checking `"futures"` causes `execution_gate` to ignore `"radar"` reduce-only flags and falsely block on `"futures"` reduce-only flags.

---

## Area B: `state.sqlite3` (`monitor_signal_events`) & `auto_exec.rs` / `convergence_auto.rs` / `execution.rs`

### 1. Audit of `monitor_signal_events` in `~/.arbitrageur/state.sqlite3`
Querying `monitor_signal_events` for today (`ts_ms >= 1790640000000`, `environment = 'testnet'`) vs. all-time shows:
* **Today (`ts_ms >= 1790640000000`)**:
  * `bot_enter`: **30** rows (representing **15 actual entries** — every single entry is recorded twice; see Bug B1 below)
  * `bot_exit`: **15** rows (`spread_converged`, `hold_timeout`, `adverse_excursion`)
  * `bot_skip`: **178** rows
  * `ready_edge`: **44** rows
  * `triggered`: **6** rows

* **Exact breakdown of the 178 `bot_skip` rows today (343 all-time) by `skipReason`**:
  1. **53 today (128 all-time)** — `entry rejected: Expected net spread <N> BPS after route friction is below the 2 BPS target`
     * Affected pairs today: `SOL hyperliquid->vanta` (45), `ADA vanta->hyperliquid` (4), `ETH vanta->robinhood` (3), `BTC robinhood->hyperliquid` (1).
  2. **47 today (71 all-time)** — `entry rejected: cash-equity leg outside US market hours — vanta/alpaca would reject IOC outright` (45 on `ADA vanta->alpaca`) and `alpaca/hyperliquid` (2 on `WIF alpaca->hyperliquid`).
  3. **24 today (24 all-time)** — `entry rejected: Insufficient free margin on vanta: required 99.831900000000 > available 99.029302` (24 on `ADA vanta->hyperliquid`).
  4. **16 today (48 all-time)** — `expected profit below 0.5 bps hurdle` (14 on `BTC robinhood->hyperliquid`, 2 on `SOL vanta->robinhood`).
  5. **9 today (13 all-time)** — `position already open` (`ETH hyperliquid->vanta`, `SOL hyperliquid->vanta`, `ETH deribit->hyperliquid`).
  6. **9 today (9 all-time)** — `circuit breaker: Circuit breaker TRIPPED: EMERGENCY KILLSWITCH ENGAGED BY OPERATOR`.
  7. **8 today (13 all-time)** — Pre-trade orderbook slippage/depth rejections on Vanta (`Exit slippage check failed on vanta: 58.68 bps > max 25 bps`, `Buy-side slippage check failed on vanta: 25.03 bps > max 25 bps`, `Insufficient order book depth ratio on hyperliquid`).
  8. **4 today (6 all-time)** — `pair cooling down (<N>s left)`.
  9. **3 today (7 all-time)** — `entry rejected: signal not ready` (race between `ready_opportunities()` snapshot and `execute_monitor_opportunity_impl` re-reading `engine.snapshot()`).
  10. **5 today (24 all-time)** — Earlier session errors (`Alpaca HTTP 404 Not Found: {"message":"no quote found for ADA"}`, `order book symbol 'PERP_ADA_USDC' does not match requested symbol 'ADA'`, `Failed to fetch orderbooks for pre-trade validation`).

### 2. Root Causes of the `bot_enter` Duplication and `bot_skip` Failures

#### Bug B1: Every bot entry is recorded TWICE in `monitor_signal_events` and `self.open` (`monitor_ipc.rs:306-324` + `auto_exec.rs:244, 465-505, 954-981`)
* When `OpportunityExecutor::tick_once` dispatches an entry via `dispatch.enter(...)` (`frontend/src-tauri/src/auto_exec.rs:940`), `VenueDispatch::enter` (`auto_exec.rs:244`) calls `crate::commands::execute_monitor_opportunity_impl(symbol, buy_venue, sell_venue, size_usd, state).await`.
* At the end of `execute_monitor_opportunity_impl` (`frontend/src-tauri/src/monitor_ipc.rs:306-324`), it **unconditionally** calls `bot.adopt_manual_position(...)`:
  ```rust
  // frontend/src-tauri/src/monitor_ipc.rs:308-323
  if res.execution_success {
      let guard = state.radar_bot.lock().clone();
      if let Some(bot) = guard {
          bot.adopt_manual_position(
              state,
              &opp.symbol,
              &opp.buy_venue,
              &opp.sell_venue,
              ...
  ```
* `adopt_manual_position` (`frontend/src-tauri/src/auto_exec.rs:465-505`) inserts the position into `self.open`, upserts `radar_bot_positions`, and records a `"bot_enter"` event in `monitor_signal_events` with `"manual": true` in `economics_json`!
* Control then returns to `OpportunityExecutor::tick_once` (`frontend/src-tauri/src/auto_exec.rs:954-981`), which **overwrites** `self.open` and `radar_bot_positions` (using the pre-await `now_ms` from line `685` instead of post-fill `Utc::now()`) and records a **second** `"bot_enter"` event!
* That is why all 15 real entries today produced 30 `bot_enter` rows in `monitor_signal_events` (15 with `"manual": true` and 15 without, millisecond-apart on the exact same `signal_id`).

#### Bug B2: One-way vs. Two-way Friction Mismatch between `monitor.rs` and `execute_spatial_arbitrage_inner` (`monitor.rs:1229-1235, 3604` vs. `execution.rs:1392-1438` & `commands.rs:585-637`) — 53 skips today
* In `frontend/src-tauri/src/monitor.rs:1229-1235`, `exit_estimate_for_mode` returns `Some(0.0)` when `execution_mode != "two_way"` (the default in `MonitorConfig` is `"one_way"` at `monitor.rs:195`). Thus the monitor's `friction_bps` (`monitor.rs:3604`) only charges **entry** taker fees + funding carry (`exit_cost_bps = 0.0`), and `expected_profit_bps = effective_spread_bps - entry_fee_bps - funding_Cost`.
* For example, on `SOL hyperliquid->vanta`, the monitor computed `spread_bps = 15.09`, `friction_bps = 5.5` (entry fees only), `expected_profit_bps = +9.59 bps` (`status = Ready`), and `evaluate_entry_gates` (`convergence_auto.rs:364`) passed `9.59 >= 0.5 bps`.
* However, `execute_monitor_opportunity_impl` (`monitor_ipc.rs:293`) calls `execute_spatial_arbitrage_inner` (`execution.rs:1392-1438`), which **unconditionally** calls `compute_route_economics` (`commands.rs:585-637`).
* `compute_route_economics` (`commands.rs:598-624`) **always charges full two-way round-trip friction**:
  ```rust
  // frontend/src-tauri/src/commands.rs:619-623
  let total_friction_bps = entry_fee_bps
      + exit_fee_bps
      + entry_slippage_bps
      + exit_slippage_bps
      + closing_spread_bps;
  ```
  where `closing_spread_bps = buy_spread_bps + sell_spread_bps` (full bid-ask spread on both venues to close!). On Vanta testnet, the book spread + exit slippage + exit fees add ~35–50 bps, turning `+9.59 bps` one-way edge into `-26` to `-41 bps` two-way `expected_net_bps`.
* Furthermore, `execute_spatial_arbitrage_inner` (`execution.rs:1392-1401`) gates `effective_net_bps` against the `"futures"` strategy's `target_profit_bps` (`2.0 bps` default) instead of `"radar"`'s `min_expected_profit_bps` (`0.5 bps`), producing `"Expected net spread -26.55 BPS after route friction is below the 2 BPS target"`.

#### Bug B3: Bare Symbol Passed to `cash_session_bound` and `validate_adapter_book` (`monitor_ipc.rs:257-283`, `execution.rs:977-978, 1146-1147, 1166-1201`, `alpaca.rs:127-129, 154-174, 851-855`) — 47 skips today (+ 5 earlier book rejections)
* In `crates/arbitrageur-exchanges/src/alpaca.rs`:
  * `cash_session_bound(&self, symbol: &str)` (`alpaca.rs:851-855`) calls `Self::to_venue_symbol(symbol)` and returns `!Self::is_crypto_symbol(&v)`.
  * `is_crypto_symbol(symbol: &str)` (`alpaca.rs:127-129`) is simply `symbol.contains('/')`.
  * `to_venue_symbol(symbol: &str)` (`alpaca.rs:154-174`) converts `"ADA-USD"` -> `"ADA/USD"` (contains `'/'` -> crypto -> `cash_session_bound == false`), **but for a bare symbol `"ADA"` (or `"WIF"`) with no `'-'` and no `'/'`, line `170` returns `Ok("ADA")`**!
  * `is_crypto_symbol("ADA")` then returns `false`, so `AlpacaAdapter::cash_session_bound("ADA")` classifies bare `"ADA"` and `"WIF"` as **US cash equities** (`true`)!
* Why did `monitor.rs` allow `ADA vanta->alpaca` and `WIF alpaca->hyperliquid` to become `Ready`?
  * Because `monitor.rs:2577-2589` (`leg_cash_session_bound`) resolves the per-venue `fetch_symbol` (`"ADA-USD"`), for which `AlpacaAdapter::cash_session_bound("ADA-USD")` returns `false`!
* Why did `execute_spatial_arbitrage_inner` reject them?
  * In `frontend/src-tauri/src/monitor_ipc.rs:257-283`, `pair_legs` (`SpatialLegSymbols`) is only constructed when `opp.group.is_some()`. For standard (non-group) pairs like `ADA` and `WIF`, `pair_legs` is `None`!
  * Consequently, `execute_spatial_arbitrage_inner` (`execution.rs:977-978`) sets `buy_order_sym = symbol` (`"ADA"`) and `sell_order_sym = symbol` (`"ADA"`).
  * At `execution.rs:1146-1147`, `sell_adapter.cash_session_bound(sell_order_sym)` is called with bare `"ADA"`, returning `true` and rejecting the trade outside US equity market hours (`"cash-equity leg outside US market hours — vanta/alpaca would reject IOC outright"`)!
  * And at `execution.rs:1166-1201`, `buy_adapter.fetch_orderbook(buy_order_sym)` and `validate_adapter_book` are called with bare `"ADA"` instead of the monitor's `venue_fetch_symbol`, causing both `Alpaca HTTP 404 Not Found: {"message":"no quote found for ADA"}` and Vanta's `order book symbol 'PERP_ADA_USDC' does not match requested symbol 'ADA'` when `validate_adapter_book` checked exact symbol equality before `canonical_base`!

#### Bug B4: `VantaAdapter` Missing `set_leverage` Implementation Forces 1x Leverage Fallback (`crates/arbitrageur-exchanges/src/vanta.rs` + `execution.rs:1525-1540, 1599-1610`) — 24 skips today
* `VantaAdapter` (`crates/arbitrageur-exchanges/src/vanta.rs`) does not implement `ExchangeAdapter::set_leverage`, falling back to the default trait method in `crates/arbitrageur-core/src/exchange.rs:251` which bails with `"Leverage updates are not implemented for venue vanta"`.
* In `execute_spatial_arbitrage_inner` (`frontend/src-tauri/src/execution.rs:1525-1540`), when `adapter.set_leverage(...)` returns `Err(e)`, it logs a warning and **degrades `leg_leverage[slot]` to `1u32` (1x leverage)**:
  ```rust
  // frontend/src-tauri/src/execution.rs:1536-1539
  // Unconfirmed venue leverage: size initial margin at 1x
  // (worst-case collateral requirement) so the free-margin
  // gate cannot under-reserve.
  leg_leverage[slot] = 1;
  ```
* At `execution.rs:1599-1600`, `im_buy = (notional_buy / Decimal::from(lev_buy)) + est_fee_buy`. For a `$100` order (`optimal_size_usd = 100.0`), `im_buy` at `1x` is `$99.83`. Because the Vanta testnet account has `$99.03` free margin (after fees from prior trades), `im_buy > acc_buy.free_margin` (`99.8319 > 99.029302`) fails on every `$100` entry even though the Vanta/Orderly account actually runs at 10x perp margin!

#### Bug B5: `monitor.rs` vs `convergence_auto.rs` Hurdle Gate Discrepancy — 16 skips today
* In `monitor.rs`, `PairStatus::Ready` does **not** check `expected_profit_bps >= min_expected_profit_bps` (it only checks whether a tier in `capacity_ladder` had `net_bps > 0.0`).
* In `OpportunityExecutor::tick_once` (`auto_exec.rs:924`), `evaluate_entry_gates` (`crates/arbitrageur-strategy/src/convergence_auto.rs:364-373`) enforces `signal.expected_profit_bps < cfg.min_expected_profit_bps` (`0.5 bps`), causing pairs with `0.0 < expected_profit_bps < 0.5` (e.g. `BTC robinhood->hyperliquid` at `0.13–0.49 bps`) to stay `Ready` in the monitor and emit a `bot_skip` (`"expected profit below 0.5 bps hurdle"`) on every 500ms tick.

---

## Area C: `crates/arbitrageur-execution/src/passive_unwind.rs` & `crates/arbitrageur-exchanges/src/deribit.rs` — Sub-Lot (`<$10`) Remainder after 100% Limit Fill

### 1. Exact Mechanism of the Deribit Sub-Lot Failure
1. **How Deribit sizes and reports orders (`crates/arbitrageur-exchanges/src/deribit.rs:733-753, 1187-1204`)**:
   * Deribit linear/inverse futures (`BTC-PERPETUAL`, `ETH-PERPETUAL`) are USD-notional contracts with `min_amount = $10` (`lot = $10`).
   * When `DeribitAdapter::place_order` (`deribit.rs:1187-1204`) receives `order.quantity` in base units (e.g. `qty = 0.0120075 BTC`, which came from `fetch_account` dividing `size_usd / mark_prev` at `deribit.rs:1114` or from the ledger's `entry_qty`), it converts base to USD using the **current** `mark_price` (`raw = qty * mark_price`) and floors to the `$10` lot grid:
     ```rust
     // crates/arbitrageur-exchanges/src/deribit.rs:1189-1200
     let raw = self.to_venue_amount(&meta, order.quantity, Some(mark_price))?;
     let lot = meta.min_amount;
     let amount = if lot > dec!(0) {
         (raw / lot).floor() * lot
     } else {
         raw
     };
     if lot > dec!(0) && amount < lot {
         anyhow::bail!(
             "Deribit order amount {} below min {} for {}",
             raw, lot, meta.instrument_name
         );
     }
     ```
   * When that limit order fills 100% of `amount` USD on Deribit at `avg_px`, `DeribitAdapter::order_response` (`deribit.rs:739-743`) converts `filled_amount` and `amount` back to base units using `avg_px.or(ref_price_usd)`:
     ```rust
     let filled = Self::from_venue_amount(meta, filled_v, avg_px.or(ref_price_usd));
     let original = Self::from_venue_amount(meta, amount, avg_px.or(ref_price_usd));
     let remaining = (original - filled).max(dec!(0));
     ```
   * Thus `OrderResponse` returns:
     * `status: OrderStatus::Filled`
     * `original_qty`: `amount / avg_px`
     * `filled_qty`: `amount / avg_px`
     * `remaining_qty`: `0`
2. **Why `execute_passive_reduce_live` (`passive_unwind.rs:583-743`) and `finish_live` (`passive_unwind.rs:761-809`) fail anyway**:
   * Because `amount` was floored to a `$10` multiple AND/OR `avg_px` moved slightly relative to the mark price that originally produced the caller's `qty`, `cur.filled_qty` (`amount / avg_px`) is slightly smaller than the caller's `qty` by a sub-lot fraction (e.g., `$0.63` worth of BTC/ETH = `0.0000075 BTC` or `0.00023 ETH`).
   * In `execute_passive_reduce_live` (`crates/arbitrageur-execution/src/passive_unwind.rs:583-743`):
     * At line `720`: `filled_total += cur.filled_qty;`
     * The loop condition at line `583` is `while tries < cfg.max_tries && filled_total < qty`. Even though `cur.status == OrderStatus::Filled` and `cur.remaining_qty == 0`, `filled_total < qty` is `true` (off by `$0.63` in base units)!
     * On the next iteration (`tries = 2`), line `603` submits a new limit order with `quantity: qty - filled_total` (`$0.63` notional).
     * `DeribitAdapter::place_order` (`deribit.rs:1194-1199`) rejects it with:
       `Err("Deribit order amount 0.63 below min 10 for BTC-PERPETUAL")`.
     * Line `618` catches the error and calls `finish_live(adapter, symbol, side, qty, filled_total, ...)`.
   * In `finish_live` (`crates/arbitrageur-execution/src/passive_unwind.rs:761-809`):
     * Line `761`: `let remaining = (requested - filled_so_far).max(Decimal::ZERO);` (`$0.63` in base units, `> 0`).
     * Because `remaining > Decimal::ZERO`, it does **not** take the `if remaining <= Decimal::ZERO` early return at line `762`!
     * Instead, at line `775`, it dispatches a **market fallback order** (`OrderType::Market`) for `remaining` (`$0.63`)!
     * `DeribitAdapter::place_order` rejects that market order too with `"Deribit order amount 0.63 below min 10"`!
     * Lines `798-808` catch `Err(e)` and overwrite `outcome` with:
       ```rust
       PassiveOutcome::MarketFallback {
           reason: format!("{reason}; market fallback errored: {e}"),
       }
       ```
       and `combine_live_attempts` (`passive_unwind.rs:841`) marks the response `OrderStatus::PartiallyFilled` (`filled < requested`), causing the caller (`execution.rs`) to treat a 100%-filled Deribit unwind as a failed/partial unwind!

### 2. How `execute_passive_reduce_live` and `finish_live` Should Handle Sub-Lot Remainders
1. **`ExchangeAdapter::wire_base_qty(symbol, desired_base)` already exists** (`crates/arbitrageur-core/src/exchange.rs:365-373`, implemented on `DeribitAdapter` at `deribit.rs:941-958` and forwarded by `AccountScopedAdapter` at `accounts.rs:1766-1771`) specifically to snap a base quantity to the venue's wire lot grid (returning `Ok(Decimal::ZERO)` when `desired_base` is below 1 Deribit `$10` lot!).
2. **In `execute_passive_reduce_live` (`passive_unwind.rs:583-743`)**:
   * Before each limit attempt (or after `filled_total += cur.filled_qty`), check the remaining base quantity `rem = (qty - filled_total).max(Decimal::ZERO)`:
     * If `cur.status == OrderStatus::Filled && cur.remaining_qty <= Decimal::ZERO` on an attempt that submitted the full remaining `rem`, **or** if `adapter.wire_base_qty(symbol, rem).await` returns `Ok(wire_rem) if wire_rem <= Decimal::ZERO`, the remainder is un-orderable sub-lot dust (`< $10` on Deribit). Break out of the loop and treat the limit unwind as complete (`PassiveOutcome::LimitFilled`).
3. **In `finish_live` (`passive_unwind.rs:761-809`) and `combine_live_attempts` (`passive_unwind.rs:815-850`)**:
   * At line `761`, after computing `remaining = (requested - filled_so_far).max(Decimal::ZERO)`, check whether `remaining` snaps to zero on the venue wire (`matches!(adapter.wire_base_qty(symbol, remaining).await, Ok(w) if w <= Decimal::ZERO)`):
     * If `remaining <= Decimal::ZERO` **or** (`had_limit_fill && wire_rem <= Decimal::ZERO`), return `PassiveOutcome::LimitFilled` (with `OrderStatus::Filled` in `combine_live_attempts` by capping `requested` to `filled_so_far`) instead of dispatching a doomed sub-lot market order.

---

## Area D: `execution.rs` (`feed_breaker_equity_usd` / `breaker_frozen_equity_usd`) & `main.rs` (lines 182–203) — Deribit $11.17M BTC/ETH Testnet Equity vs. Circuit Breaker

### 1. Evidence from `~/.arbitrageur/state.sqlite3`
* `kv` `risk.cb_max_dd_usd`: `25000` (`$25,000` USD; `max_drawdown_pct` default is `5.0%` in `CircuitBreakerConfig::default()` at `crates/arbitrageur-risk/src/circuit_breaker.rs:42`).
* `kv` `cb.fx_anchor`: `{"day":"2026-09-29","fx":{"BTC":83483.7,"ETH":2687.8015}}`.
* `kv` `cb.snapshot`:
  ```json
  {
    "tripped": true,
    "trip_reason": "Unresolved exposure after failed unwind",
    "tripped_at": "2026-09-29T18:43:52.854010728Z",
    "day_start_equity": 999.576893,
    "current_equity": 11170522.809738595,
    "peak_equity": 11173894.074202275,
    "day_key": "2026-09-29",
    "consecutive_errors": 1
  }
  ```
* `margin_snapshots` / `account_equity_events` for `testnet-deribit`: raw equity fluctuates between `$10,982,854` and `$11,191,401` (~130 BTC + ~100 ETH), with `$5,000–$9,300` swings between consecutive polls and `$230,818` in `used_margin` from open BTC/ETH options and perpetuals.
* Notice that on a `$11.17M` Deribit testnet account, the `$25,000` `max_drawdown_usd` threshold is only **0.224% (22.4 bps)** of total equity!

### 2. Four Concrete Bugs in the Circuit Breaker Equity Pipeline

#### Bug D1: Incremental Per-Venue Loop on Startup / Mode Switch Corrupts `day_start_equity` (`main.rs:188-200`, `commands.rs:5418-5442`, `execution.rs:3131-3160`)
* On process start, `state.breaker_venue_equity` is **empty** (`Mutex::new(HashMap::new())` in `state.rs:1866`) and `state.last_risk_live_universe` is **`None`** (`RwLock::new(None)` in `state.rs:1864`), even though `state.circuit_breaker` restored `cb.snapshot` from SQLite (`state.rs:2203-2206`).
* Two seconds after startup, `main.rs:188-200` iterates sequentially over `bg_state.get_venues()` and calls `execution::feed_breaker_equity_usd` **inside the loop after each individual venue** (as does `maybe_record_margin_snapshots` in `commands.rs:5418-5442`):
  ```rust
  // frontend/src-tauri/src/main.rs:188-200
  for (vid, _name, adapter) in bg_state.get_venues() {
      if let Ok((mut acc, _stale)) =
          execution::live_account_or_cached(&bg_state, vid, &adapter).await
      {
          bg_state.normalize_account_usd(vid, &mut acc);
          execution::feed_breaker_equity_usd(
              &bg_state,
              vid,
              acc.equity,
              &acc.balances,
          );
      }
  }
  ```
* Look at what `feed_breaker_equity_usd` (`frontend/src-tauri/src/execution.rs:3153-3159`) does on the **very first venue** in the loop (e.g. `vanta` or `hyperliquid`, with `$999.576893` equity):
  ```rust
  let mut universe = state.last_risk_live_universe.write();
  if *universe != Some(is_live) {
      state.circuit_breaker.reanchor_baseline(total);
      *universe = Some(is_live);
  } else {
      state.circuit_breaker.update_equity(total);
  }
  ```
  1. On the first venue (`$999.58`), `*universe` is `None != Some(false)`, so it calls **`reanchor_baseline(999.576893)`** — wiping out the persisted `day_start_equity` and anchoring `day_start_equity = $999.58`, `peak_equity = $999.58`, and setting `*universe = Some(false)`!
  2. When `deribit` (`$11,169,523`) is fetched later in the **same startup loop**, `*universe` is now already `Some(false)`, so `feed_breaker_equity_usd` calls **`update_equity(11,170,522.81)`** instead of `reanchor_baseline`! That raises `peak_equity` to `$11,173,894.07` while leaving `day_start_equity` permanently stuck at `$999.58` (the exact state seen in `cb.snapshot` above!).
  3. **And in the reverse order**: if `deribit` (`$11.17M`) was anchored into `peak_equity` (either via `cb.snapshot` or because `deribit` was read first during a pre-trade check), and any subsequent poll runs when `deribit` is not yet in `breaker_venue_equity`, `update_equity` is called with `total = $999.58` against `peak_equity = $11.17M` — an instant `$11.17M` drawdown trip!
* Note that `execution.rs:3165-3194` already defines a batch function `feed_breaker_equity(state, &entries)` that collects all venues before updating the breaker once, which `get_risk_overview_impl` (`execution.rs:3303, 3352`) uses, but `main.rs:188-200` and `commands.rs:5418-5442` do not.

#### Bug D2: UTC Day-Roll Split-Brain Between `breaker_fx_anchor`, `breaker_venue_equity`, and `CircuitBreaker::roll_day_if_needed` (`execution.rs:315-319, 3143-3159` + `circuit_breaker.rs:250-282`)
* Trace what happens at `00:00 UTC` when `main.rs:188` polls venues sequentially (`hyperliquid`/`vanta` first, `deribit` later):
  1. When the first USD-margined venue (`hyperliquid` or `vanta`) feeds `feed_breaker_equity_usd` after `00:00 UTC`, `breaker_frozen_equity_usd` (`execution.rs:315-319`) sees `anchor.day != Some(today)`, sets `anchor.day = Some(today)`, and calls `anchor.fx.clear()`. Because this first venue has only USD-family balances, `anchor.fx` remains empty.
  2. `feed_breaker_equity_usd` (`execution.rs:3143-3151`) sums `state.breaker_venue_equity`. At this moment, `breaker_venue_equity["testnet-deribit"]` **still holds yesterday's frozen equity** (valued at yesterday's `fx_day`, e.g. `$11,170,000`).
  3. `feed_breaker_equity_usd` calls `state.circuit_breaker.update_equity(total)` (`$11,171,000`). Inside `update_equity` (`crates/arbitrageur-risk/src/circuit_breaker.rs:250-282`), `roll_day_if_needed` sees the new UTC date and anchors **`day_start_equity = peak_equity = $11,171,000`** (using yesterday's Deribit `fx_day`!) and sets `state.day_key = today`.
  4. A few seconds later in the same loop, `deribit` is polled. `breaker_frozen_equity_usd` sees `anchor.fx` has no entry for `"BTC"` or `"ETH"`, so it seeds `fx_day = fx_now` (today's spot price) and computes `adj = 0`.
  5. `breaker_venue_equity["testnet-deribit"]` now updates from yesterday's frozen value to today's unadjusted USD value. If BTC/ETH ended the prior UTC day down by even **0.23%** (`-$25,000` on `$11.17M`), `update_equity` runs on the already-rolled `day_key` and sees `day_start_equity - new_equity > $25,000`, **immediately tripping the circuit breaker at 00:00 UTC**!

#### Bug D3: `DeribitAdapter::fetch_account` Mutates `account_index_used` Mid-Loop Before All 8 HTTP Requests Complete (`deribit.rs:1054-1083` + `execution.rs:328-333, 3064-3089`)
* In `DeribitAdapter::fetch_account` (`crates/arbitrageur-exchanges/src/deribit.rs:1054-1083`), the loop over `SUPPORTED_CURRENCIES` (`["BTC", "ETH"]`) makes **8 sequential HTTP requests** (for each currency: `index_price_usd`, `get_account_summary`, `get_positions(future)`, `get_positions(option)`).
* At line `1056` (the very first step of each currency iteration), it immediately mutates `self.account_index_used`:
  ```rust
  // crates/arbitrageur-exchanges/src/deribit.rs:1054-1061
  for ccy in SUPPORTED_CURRENCIES {
      let idx = self.index_price_usd(ccy).await?;
      self.account_index_used.write().insert(ccy.to_string(), idx);
      let summary = self
          .private_get(&format!(
              "/private/get_account_summary?currency={ccy}&extended=true"
          ))
          .await?;
  ```
* If `index_price_usd("BTC")` succeeds and writes `idx_new` into `self.account_index_used`, and **any** of the remaining 7 HTTP calls in `fetch_account()` fails with `?` (timeout / rate limit), `live_account_or_cached` (`execution.rs:3075-3085`) falls back to the **cached** `AccountSnapshot` from `last_good_accounts` (whose `acc.equity` was computed using `idx_old`).
* Then `breaker_frozen_equity_usd` (`execution.rs:328-344`) pairs that cached `equity_usd` (`bal * idx_old`) with `a.last_account_index_usd("BTC")` (`idx_new`):
  $$\text{frozen} = \text{ bal } \times \text{idx}_{\text{old}} + \text{bal} \times (\text{fx}_{\text{day}} - \text{idx}_{\text{new}}) = \text{bal} \times \text{fx}_{\text{day}} + \text{bal} \times (\text{idx}_{\text{old}} - \text{idx}_{\text{new}})$$
* Whenever `idx_new < idx_old` (e.g. BTC fell by `$200` since the cached snapshot), `bal * (idx_old - idx_new)` injects a phantom `+$26,000` spike into `frozen`, raising `peak_equity` by `+$26,000`. On the very next poll when `fetch_account()` succeeds (`idx_old == idx_new`), `frozen` returns to `bal * fx_day` — dropping `$26,000` below the inflated `peak_equity` and tripping the `$25,000` drawdown breaker!

#### Bug D4: `breaker_frozen_equity_usd` Freezes Current Cash `balance` (`bal.total`) Instead of `equity` (`eq`) When Open Options/Perps Exist (`deribit.rs:1062-1077` + `execution.rs:344`)
* In `DeribitAdapter::fetch_account` (`deribit.rs:1062-1077`):
  * `eq = summary["equity"]` (in BTC/ETH), which on Deribit is `balance + options_market_value + futures_session_upl`.
  * `bal = summary["balance"]` (in BTC/ETH), which is cash balance **excluding** open option market value (when you buy a Deribit option, the premium is immediately subtracted from `balance` and sits in `options_market_value` inside `equity`).
* In `breaker_frozen_equity_usd` (`execution.rs:344`):
  ```rust
  adj += bal.total * (fx_day - fx_now);
  ```
  Since `equity_usd = eq * fx_now`, the resulting frozen equity is:
  $$\text{frozen} = \text{eq} \times \text{fx}_{\text{now}} + \text{bal} \times (\text{fx}_{\text{day}} - \text{fx}_{\text{now}}) = \text{bal} \times \text{fx}_{\text{day}} + (\text{eq} - \text{bal}) \times \text{fx}_{\text{now}}$$
* Because this Deribit testnet account holds open BTC/ETH options and perpetuals (`$230,818` in `used_margin`, plus option fills in `signal_executions`), `eq != bal` and `bal` shifts whenever option premium is paid or received. This caused the `$3,371.26` drift between `peak_equity` (`$11,173,894.07`) and `current_equity` (`$11,170,522.81`) in `cb.snapshot`, and any larger open option/perp position or coin-settled mark move on the `$11.17M` balance easily exceeds the static `$25,000` (`0.22%`) drawdown cap.