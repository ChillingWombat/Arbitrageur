# Audit Report: System Status, Vault Auto-Lock, Funding Carry, Spot Arbitrage & Triangular Arbitrage

Below are the exact root causes, file paths, and line numbers for Questions A–D.

---

## Question A: Why `get_system_status_impl` (`system_status` RPC) reports `status: "READ_ONLY"` for all active testnet/live venues

### Exact Location
- `frontend/src-tauri/src/commands.rs:1024-1039` (`get_system_status_impl`, closure `venue_status`)
- `frontend/src-tauri/src/commands.rs:1043-1404` (`venues` vector construction & post-loop capability enrichment)

### Root Cause
1. **Hardcoded 3-state closure with no `"ONLINE"` / `"CONNECTED"` / `"ACTIVE"` branch (`commands.rs:1024-1039`):**
   ```rust
   let venue_status = |vid: &str, enabled: bool| -> &str {
       if !crate::state::venue_has_adapter(vid) {
           "UNSUPPORTED"
       } else if !enabled {
           "OFFLINE"
       } else if admitted_map.contains_key(vid) {
           "READ_ONLY"
       } else {
           "UNSUPPORTED"
       }
   };
   ```
   Every venue entry in `venues = vec![...]` (`commands.rs:1043-1393`) sets `status: venue_status("<vid>", s.<vid>_enabled).into()`. Whenever a venue is enabled and present in `state.get_venues()` (`admitted_map`), `venue_status` unconditionally returns `"READ_ONLY"`.
2. **Post-construction loop sets `can_submit` / `can_enter_strategy` without updating `v.status` (`commands.rs:1396-1404`):**
   ```rust
   for v in &mut venues {
       if let Some(a) = state.get_adapter(&v.id) {
           v.can_submit = a.live_order_submission_implemented();
           v.can_enter_strategy = v.can_submit && a.supports_strategy_entry();
       }
       v.feed = feed_lanes.get(&v.id).cloned();
   }
   ```
   Even when `a.live_order_submission_implemented() && a.has_live_credentials()` is `true` (and `v.can_submit == true`, `v.can_enter_strategy == true`), `v.status` is never promoted from `"READ_ONLY"` to `"ONLINE"` / `"ACTIVE"`, nor does it differentiate between credentialed order-capable venues and uncredentialed/data-only venues.

---

## Question B: Vault 15-Minute Auto-Lock & Its Interaction Across Subsystems

### 1. How the 15-Minute Auto-Lock Works (`vault.rs` & `state.rs`)
- **Fixed 15-minute TTL from unlock/write (`frontend/src-tauri/src/vault.rs:491, 526, 634`):**
  - `initialize` (`vault.rs:491`), `admit_unlocked` (`vault.rs:526`, called by `unlock`), and `save_data` (`vault.rs:634`) set:
    `self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));`
  - Reading from the vault (`load_data`, `is_unlocked`) does **not** slide/extend `unlocked_until`. Exactly 15 minutes after the last unlock or credential save, the TTL expires.
- **Lazy auto-lock transition (`vault.rs:254-264`, `286-293`):**
  - The next call to `vault.is_unlocked()` (`vault.rs:254-264`) or `vault.load_data()` (`vault.rs:593-597`) sees `Utc::now() > until` and calls `self.apply_lock(true)` (`vault.rs:286-293`), which:
    1. Sets `self.is_unlocked = false` and `self.auto_locked = true`.
    2. Zeroizes and drops `decrypted_cache`, `encryption_key`, and `kdf_salt`.
- **Intended design in `sync_vault_credentials_to_adapters` (`frontend/src-tauri/src/state.rs:2821-2833`):**
  - Comments at `vault.rs:141-145` and `state.rs:2821-2823` explicitly state the intended architecture:
    > *"Under auto-lock the already-provisioned adapter credentials stay in place so live trading/reconciliation survives an unattended session; only an explicit operator lock (`auto_locked == false`) wipes them."*
  - Specifically, in `state.rs:2824-2832`:
    ```rust
    if !vault.is_unlocked() {
        if vault.is_auto_locked() {
            return;
        }
        drop(vault);
        self.clear_adapter_credentials();
        return;
    }
    ```
  - And in `reconciliation.rs:324-338`:
    `let manual_locked = vault.is_initialized() && !vault.is_unlocked() && !vault.is_auto_locked();` — order reconciliation explicitly continues polling and dispatching residual flattens during `is_auto_locked() == true`.

### 2. Where Other Subsystems Violate the Auto-Lock Contract (`!vault.is_unlocked()` without checking `!vault.is_auto_locked()`)

Because `vault.is_unlocked()` returns `false` once auto-locked, **every caller that checks `!state.vault.write().is_unlocked()` without checking `!vault.is_auto_locked()` treats the 15-minute idle auto-lock as a hard lock**, contradicting `state.rs:2821-2832`:

| Subsystem / Function | File : Lines | Behavior After 15m Auto-Lock (`is_unlocked() == false`, `is_auto_locked() == true`) |
|---|---|---|
| **`live_entry_gate(state)`** | `commands.rs:35-43` | Checks `if !state.is_live() { return Ok(()); } if !state.vault.write().is_unlocked() { return Err("Live strategy entries require an unlocked credential vault".into()); }`. **Blocks all Live-mode strategy entries after 15 minutes** even though adapter credentials remain loaded in memory. (Testnet mode bypasses `live_entry_gate` because `!state.is_live()` returns `Ok(())` at line 36.) |
| **Spatial Arbitrage Entry** (`execute_spatial_arbitrage_inner`) | `execution.rs:1071` | Calls `crate::commands::live_entry_gate(state)?`. In **Live mode**, manual spatial entries and **autonomous Radar Bot (`auto_exec.rs:1441`) entries fail after 15 minutes** with `"Live strategy entries require an unlocked credential vault"`. In **Testnet mode**, both manual and Radar Bot entries continue working because `tn_*` adapters kept their credentials and `live_entry_gate` is a no-op on testnet. |
| **Funding Carry Entry & Autonomous Fixed-Yield Entry** (`execute_funding_carry_impl`) | `funding.rs:1987, 1989, 2150, 2152` | Calls `live_entry_gate(state)` at line 1989 (and 2152). Worse, for `fixed_yield` (Boros YU), line 1987 enforces `if !state.is_live() { return Err(...); }` AND line 1989 calls `live_entry_gate(state)?`, plus lines 2742 & 2948 call `crate::accounts::load_evm_private_key(state, ...)` which reads the vault (`accounts.rs:640`). Thus **autonomous Fixed-Yield entry (`funding.rs:1895` in `check_fixed_yield_maturities`) and manual Live funding carry entry break completely 15 minutes after unlock**. |
| **Position Flattening — CRITICAL LIVE SAFETY BUG** (`flatten_positions_inner`) | `execution.rs:5111-5116` | Checks `if is_live && !state.vault.write().is_unlocked() { return Err("Cannot flatten while the secure credential vault is locked — unlock the vault first so close orders can authenticate".into()); }`. While `lock_vault_impl` (`accounts.rs:159-177`) refuses manual locking when live positions are open, **15 minutes after unlock the vault auto-locks and `flatten_positions_inner` refuses ALL Live flattens** — breaking: (1) operator panic flattens (`flatten_ticker_positions_impl` / `flatten_signal_positions_impl`), (2) Radar Bot autonomous exits (`auto_exec.rs:820`), (3) scheduled hold-timeout exits (`commands.rs:3844`), and (4) funding exit supervisor (`monitor.rs:587` `flatten_funding_signal`)! |
| **Fixed-Yield Maturity Exit** (`execute_fixed_yield_exit_impl`) | `funding.rs:3629-3634`, `3742` | Checks `if !state.vault.write().is_unlocked() { return Err("Fixed-yield exit requires an unlocked credential vault...".into()); }` and loads the EVM private key from the vault at line 3742. If a Boros YU position matures >15 minutes after the operator unlocked the vault, `check_fixed_yield_maturities` (`funding.rs:1767`) fails every 60s until the operator manually re-unlocks the vault. |
| **Liquidation-Distance Trim** (`maybe_liq_trim_pair`) | `execution.rs:3816-3820` | Checks `if state.is_live() && !state.vault.write().is_unlocked() { return None; }`. **Disables autonomous liquidation-protection trimming in Live mode 15 minutes after unlock!** |
| **Triangle Arbitrage Entry** (`execute_triangle_arbitrage_impl`) | `triangle.rs:693` | Calls `crate::commands::live_entry_gate(state)?`. Fails in Live mode after 15m auto-lock; works in Testnet mode. |
| **Options Parity & Package Entries + Options Bot** (`execute_options_parity_impl`, `execute_options_package_impl`, `OptionsBot::poll_substrategy`) | `options_parity.rs:565`, `options_packages.rs:997`, `options_bot.rs:496, 509` | Both execute functions call `live_entry_gate(state)?`. In Live mode, `OptionsBot` (`options_bot.rs:496, 509`) and manual option entries fail after 15m auto-lock; in Testnet mode they continue working. |
| **Cancel Venue Order & Close External Position** | `commands.rs:4814`, `dev_rpc.rs:354`, `external_positions.rs:1143` | All check `if !state.vault.write().is_unlocked()` without allowing `is_auto_locked()`, blocking live order cancellation and external position closing after 15m. |
| **Venue Execution Capabilities** (`get_venue_execution_capabilities_impl`) | `commands.rs:2800-2815` | Sets `let is_unlocked = state.vault.write().is_unlocked(); let submission_ready = is_unlocked && supports_live && has_credentials;`. After 15m auto-lock, `submission_ready`, `strategy_entry_ready`, and `live_ready` all flip to `false` in the UI (even on Testnet!) despite adapters still holding credentials. |
| **Accounts & Balances** (`accounts.rs`) | `accounts.rs:770-790`, `1690-1732`, `1992-2003` | `get_venue_balances_impl` (`accounts.rs:1690`) calls `sync_vault_credentials_to_adapters()` (which is a no-op under auto-lock) and then checks `adapter.has_live_credentials()`, so **balance polling continues to work during auto-lock**. However, `list_accounts_impl` (`accounts.rs:770-790`) reads `vault.load_data().ok()`, which returns `None` during auto-lock, causing every account row to report `credentials_configured: None` and `has_api_key: false`; and `test_account_connection_impl` (`accounts.rs:1994`) fails with `"Vault is locked"`. |

---

## Question C: `funding.rs` — Why `scan_funding` Returned `breakeven_hours: None` and `entry_spread_bps: 0.0` on `BTC-USDC` (`lighter` -> `hyperliquid`, `net_apr_pct: 183%`), and How Funding Carry Positions Are Tracked, Monitored & Unwound

### 1. Why `breakeven_hours` Was `None` (and When `break_even_hours` Is `0.0` or Understated)
There are **two distinct layers** here:
1. **DTO / JSON field-name mismatch (`frontend/src-tauri/src/funding.rs:60` vs `crates/arbitrageur-math/src/funding_carry.rs:256`):**
   - In `FundingOpportunityDto` (`funding.rs:34-60`), the field is named `pub break_even_hours: f64` (two underscores, serialized to JSON as `"break_even_hours"` because `FundingOpportunityDto` has **no** `#[serde(rename_all = "camelCase")]`).
   - By contrast, `FundingHoldEstimate` (`funding_carry.rs:256`) names its field `breakeven_hours` (one underscore). Any RPC client/script reading `row["breakeven_hours"]` or `row["breakevenHours"]` from `scan_funding` gets `None` / `undefined` because the JSON key is `"break_even_hours"`.
2. **Mathematical bug in `evaluate_funding_carry` (`crates/arbitrageur-math/src/funding_carry.rs:83-94` & `funding.rs:826-835, 949-968`):**
   - `FundingOpportunityDto.break_even_hours` comes from `eval.break_even_hours` (`funding.rs:1066`), which is computed in `evaluate_funding_carry` (`funding_carry.rs:83-88`):
     ```rust
     let fee_ratio = round_trip_fee_bps / dec!(10000);
     let break_even_hours = if net_hourly > dec!(0) {
         fee_ratio / net_hourly
     } else {
         dec!(999999)
     };
     ```
   - Notice two things:
     - `evaluate_funding_carry` **completely ignores `entry_spread_bps`** when computing `eval.break_even_hours` (it only divides `round_trip_fee_bps` by `net_hourly`).
     - Meanwhile, `funding.rs:949-954` computes a *second* struct `hold_estimate = estimate_funding_hold_hours(net_hourly_f64, round_trip_fee_bps, entry_spread_bps.max(0.0), ...)` (`funding_carry.rs:289-294`), which *does* include `entry_spread_cost_bps` in `hold_estimate.breakeven_hours`, **but `funding.rs:953` only uses `hold_estimate.breakeven_hours` as an input to `gap_persistence_probability` and discards it instead of putting it on `FundingOpportunityDto`!**
     - And at `net_apr_pct = 183%` (`net_hourly = 1.83 / 8760 = 0.0002089`, or ~2.09 bps/hr) with Lighter (`0–1.5 bps`) + Hyperliquid (`3.5 bps`), `round_trip_fee_bps` is `10.0 bps` (`fee_ratio = 0.0010`), so `eval.break_even_hours` is `0.0010 / 0.0002089 = 4.78 hours` (or `0.0` if both venues' fee overrides are `0`).

### 2. Why `entry_spread_bps` Returned `0.0` on `BTC-USDC` (`lighter` -> `hyperliquid`)
- **Exact Location:** `frontend/src-tauri/src/funding.rs:843-863` and `crates/arbitrageur-math/src/funding_carry.rs:964-1065` (`funding_entry_capacity`).
- **Root Cause — favorable cross-venue entry spread is clamped to `0.0` (`funding.rs:861`):**
  1. In `funding_entry_capacity` (`funding_carry.rs:1052-1053`):
     ```rust
     let spread = (cost_b / qty_b - cost_a / qty_a) / (cost_a / qty_a) * 10_000.0;
     ```
     Here `venue_a` (`lighter`) is the **long** leg (buying the ask at VWAP `cost_a / qty_a`) and `venue_b` (`hyperliquid`) is the **short** leg (selling the bid at VWAP `cost_b / qty_b`).
     - When `short_bid > long_ask` (i.e. Hyperliquid's perp bid is higher than Lighter's perp ask — which naturally happens when Hyperliquid has a huge positive funding rate of +183% APR because Hyperliquid perp trades at a premium to Lighter!), `spread` (`rep.entry_spread_bps`) is **positive** (`> 0.0`), meaning you **earn** a positive spatial spread upon entering the carry trade.
  2. Then in `funding.rs:860-862`:
     ```rust
     let rep = cap.tiers.iter().find(|t| (t.notional_usd - opt).abs() < 1.0)?;
     let cost = rep.entry_spread_bps.map(|e| (-e).max(0.0))?;
     Some((cost, opt))
     ```
     `(-e).max(0.0)` negates the signed spread `e` to convert it into an "entry spread **cost** in bps" and **clamps any favorable (`e > 0`) entry spread to `0.0`**!
  3. Thus:
     - `entry_spread_bps == 0.0` whenever `short_bid >= long_ask` (favorable or zero entry spread cost).
     - `entry_spread_bps > 0.0` whenever `short_bid < long_ask` (adverse entry spread cost).
     - `entry_spread_bps == -1.0` (`funding.rs:863`, sentinel for "unavailable") when either venue's orderbook is missing/stale (>120s in `market_data_hub`) OR when book depth cannot fill at least 99% of the smallest capacity tier (`0.5 * trade_size_usd`, `funding_carry.rs:1057`: `if filled >= tier * 0.99 { optimal = Some(tier); }`).

### 3. How `execute_funding_carry_impl` Tracks, Monitors, and Unwinds Funding Carry Positions
1. **Pre-Flight & Validation (`funding.rs:1920-2455`):**
   - Enforces `live_entry_gate(state)` (`1989`, `2152`), circuit breaker, pause state, and `execution_lock`.
   - Re-scans via `scan_funding_opportunities_impl(symbol, trade_size_usd, state)` (`1996`) and matches `(symbol, long_venue, short_venue, kind)`.
   - Verifies `opp.net_apr_pct >= funding_min_apr(state)` (default `8.0%`, `2015-2023`), fresh orderbook slippage via `verify_pre_execution_slippage` (`2233`), and `live_entry_spread_cost_bps <= max_entry_spread` (default `50.0 bps`, `2268-2281`).
   - Checks `state_store.blocking_signals(&root_symbol)` (`2290`), `ticker_reduce_only` (`2299`), and verifies both venues' accounts have no existing position above `triangle_dust_qty()` (`2310-2335`).
   - Sets leverage on perp legs (`adapter.set_leverage`, `2356-2380`) and checks initial margin (`long_im`, `short_im`, `2385-2434`).
2. **Durable Signal & Virtual Ledger Tracking (`funding.rs:2468-3115`):**
   - Persists a durable `SignalExecution` row in SQLite (`signal_executions` + `signal_legs`) **before** order submission (`funding.rs:2474-2504`):
     - `id`: `"fund-{timestamp_millis}-{rand16}-carry"`
     - `strategy_id`: `"funding"`
     - `symbol`: `root_symbol` (e.g. `"BTC"`)
     - `legs`: `LegSide::Long` on `long_venue` (`long_exec_symbol`, e.g. `"BTC-SPOT"` or `"BTC-USDC"`) and `LegSide::Short` on `short_venue` (`short_exec_symbol`).
   - Persists deterministic `client_order_id`s (`2527-2549`), then submits both legs concurrently via `state.coordinator.execute_two_leg(long_order, short_order, ...)` (`2579`).
   - Calls `reconcile_signal_legs` (`2691`) and updates signal status to `SignalStatus::Balanced` (if both filled) or `SignalStatus::Imbalanced` (if one-sided/partial, triggering auto-residual flatten in `reconciliation.rs:661`).
   - Opens an attributed cycle lot in `state.virtual_ledger` (`id = "funding-{cycle}"`, `strategy_id = "funding_rate"`, `funding.rs:3084-3105`) and persists it to SQLite via `state.state_store.upsert_cycle_lot(&lot)`.
   - If `kind == "fixed_yield"`, dispatches 2 Boros YU hedge legs under sibling signal `"{cycle_tag}-yu"` (`2815-3055`) and inserts a row into `fixed_yield_maturities` (`3061-3075`).
3. **Autonomous Monitoring & Unwinding (`monitor.rs:2900-3090`, `funding.rs:1733-1913`, `commands.rs:3818-3862`):**
   - **Carry / Basis Exit Supervisor (`MonitorEngine::supervise_funding_exits` in `frontend/src-tauri/src/monitor.rs:2900-3090`):**
     Called every 60s at the end of `MonitorEngine::poll_funding` (`monitor.rs:2889`). It iterates over all `SignalStatus::Balanced` root signals with `strategy_id == "funding"` (`monitor.rs:3011-3016`) and evaluates **four exit gates in order**:
     1. **Liquidation-Distance Trim (`monitor.rs:3031-3042`):** If `funding_liq_trim_pct` is configured, calls `execution::maybe_liq_trim_pair` to partially trim both legs when either leg's liquidation distance breaches `trim_pct`.
     2. **Basis-Convergence Exit (`monitor.rs:3046-3052`, `614-645`):** For spot+perp basis pairs (where `long_leg.symbol` ends with `-SPOT`), if the live `(perp_mid - spot_mid) / spot_mid * 100.0` falls below `funding_auto_exit_spread_pct`, calls `flatten_funding_signal(state, sig, "basis-convergence", ...)` (`monitor.rs:577-606`), which invokes `flatten_ticker_positions_impl(Some(&sig.symbol), "funding", CycleStatus::StoppedOut, state)`.
     3. **Yield Rotation Exit (`monitor.rs:3066-3079`):** If `funding_rotation_threshold_bps` is configured and another executable symbol/venue pair has a fresh (<5m) EMA span APR exceeding the current position's `net_apr_pct` by more than `funding_rotation_threshold_bps / 100.0`, flattens the position via `flatten_funding_signal(..., "rotation", ...)`.
     4. **Carry-Hurdle Decay / Rate-Flip Exit (`monitor.rs:3080-3088`):** Computes the position's live EMA net APR `net_apr_pct = (sr - lr) * 8760.0 * 100.0` from `self.funding_ema` (requires fresh <5m samples). If `net_apr_pct < min_apr` (`funding_min_apr(state)`, default `8.0%`), flattens the position via `flatten_funding_signal(..., "carry-hurdle", ...)`.
   - **Fixed-Yield Maturity Supervisor (`check_fixed_yield_maturities` in `funding.rs:1733-1913`):**
     Polls `fixed_yield_maturities` every 60s (`funding.rs:1707-1725`) and calls `execute_fixed_yield_exit_impl` (`funding.rs:3611`) when `now_ms >= mat.maturity_ms`.
   - **Scheduled Hold-Timeout Exits (`commands.rs:3818-3862`):**
     `spawn_scheduled_exit_supervisor` checks `due_scheduled_exits` every 5s and calls `flatten_ticker_positions_impl` if an operator scheduled a timed exit for `"funding"`.
   - **Note on Vault Auto-Lock interaction:** Because `flatten_funding_signal` calls `flatten_ticker_positions_impl` -> `flatten_positions_inner` (`execution.rs:5111`), **in Live mode `supervise_funding_exits` will fail to flatten decayed/flipped funding positions once the vault's 15-minute auto-lock fires** (see Question B above)!

---

## Question D: Why `scan_spot` Returned 0 Rows on Testnet, and How `scan_spot` / `execute_spot` and `triangle.rs` Work

### 1. Why `scan_spot_opportunities_impl` (`scan_spot`) Returned 0 Rows on Testnet (`commands.rs:2281-2530`)
There are **three compounding reasons** in `frontend/src-tauri/src/commands.rs:2281-2380`:

1. **Almost no testnet adapters emit `MarketType::Spot` in `fetch_markets()`:**
   - `scan_spot_opportunities_impl` (`commands.rs:2300-2308`) iterates over `state.get_venues()` and filters `state.cached_markets(vid, adapter)` for `m.symbol.market_type == MarketType::Spot && m.is_active`.
   - Across the entire `crates/arbitrageur-exchanges/src/` crate, only **4 adapters** ever construct `MarketType::Spot` (`Symbol::spot`):
     - `coinbase.rs:523` — **Live-only** (no `tn_coinbase` in testnet `state.rs:2080-2131`).
     - `evm_amm.rs:1831` — **Live-only** (no EVM AMMs in testnet `state.rs:2080-2131`).
     - `alpaca.rs:377, 419` (`tn_alpaca`) — Requires authenticated `/v2/assets` headers (`alpaca.rs:353`), so `fetch_markets()` fails unless an Alpaca paper account is configured; and its crypto pairs are quoted primarily in `USD` (`BTC-USD`, `ETH-USD`) or `USDC`/`USDT`.
     - `lighter.rs:434` (`tn_lighter` and `tn_robinhood`) — Emits `Symbol::spot(symbol_str, "USDC")` when Lighter's `/api/v1/orderBookDetails` has `"market_type": "spot"`.
   - None of `tn_binance`, `tn_okx`, `tn_hl`, `tn_kraken`, `tn_vanta`, `tn_decibel`, `tn_popdex`, `tn_deribit`, or `tn_arcus` emit `MarketType::Spot` in `fetch_markets()`.
2. **Exact `BASE-QUOTE` multi-venue overlap requirement (`commands.rs:2305-2327`):**
   - `scan_spot_opportunities_impl` keys `symbol_venues` by exact `canonical = format!("{}-{}", canonical_base(&m.symbol.base), m.symbol.quote.to_uppercase())` and requires `venues.len() >= 2` (`commands.rs:2323`, never merging `USD` and `USDC` quotes).
   - Because Robinhood testnet (`api.rh-testnet.lighter.xyz`) lists no spot markets and `tn_alpaca` is either uncredentialed or lists `*-USD` while `tn_lighter` lists `*-USDC`, `shared` (`venues.len() >= 2`) is empty (`0` shared spot pairs).
3. **Broken hub key & symbol resolution for Lighter spot books (`commands.rs:2371-2377`, `2591`, `2684` vs `lighter.rs:350, 500, 1523`):**
   - Even when two venues share a spot `canonical` symbol (e.g. `ETH-USDC`), `scan_spot_opportunities_impl` (`commands.rs:2371-2377`) **only** queries `state.market_data_hub.get_valid_orderbook(&venue_id, &canonical, Some(Duration::from_secs(30)))` with **no REST fallback**.
   - On Lighter, spot books in the WS feed (`ws.rs` / `feed.rs`) and REST (`lighter.rs:350, 500`) are keyed as `"{BASE}-SPOT"` (`canonical_base("ETH-SPOT") == "ETH-SPOT"`), whereas passing `&canonical` (`"ETH-USDC"`) to `get_valid_orderbook` looks up `canonical_base("ETH-USDC") == "ETH"` (the **perp** book slot!).
   - Furthermore, in `monitor.rs:1986-2016`, `by_key` deduplicates each venue's markets by `pair_key(&m.symbol.raw, merge_usd)`, so a venue that lists both `ETH-USDC` perp and `ETH-USDC` spot only polls **one** of them in `monitor.rs`! And if `monitor.rs` tries to poll Lighter's spot `venue_symbol` (numeric `market_id`, e.g. `"2048"`), `lighter.rs:1523-1551` (`resolve_market_id`) defaults any symbol without a `"-SPOT"` suffix to `market_type = "perp"` and fails with `"No Lighter perp market with id ..."`!
   - Finally, in `execute_spot_arbitrage_impl` (`commands.rs:2591, 2684, 2691`), calling `fetch_orderbook(&canonical)` and `OrderRequest::new_ioc(..., canonical.clone(), ...)` passes `"ETH-USDC"` (without `"-SPOT"`), which on Lighter resolves to the **perp** market (`lighter.rs:1523-1526`). Compare `funding.rs:1165-1190` (`funding_spot_book`), which explicitly translates Lighter spot requests to `"{BASE}-SPOT"`.

### 2. How `triangle.rs` (`scan_triangular_opportunities_impl` & `execute_triangle_arbitrage_impl`) Works and Its Testnet Limitations
- **Scanner (`frontend/src-tauri/src/triangle.rs:176-531`):**
  1. Iterates over **single-venue** adapters in `state.get_venues()` (`triangle.rs:365`) plus multi-DEX route plans (`plan_multi_venue_triangles`, `triangle.rs:413-525`).
  2. For each venue, calls `plan_venue_triangles(&markets)` (`triangle.rs:64-152`):
     - Collects active base assets from `markets` (`triangle.rs:84-97`).
     - Forms candidate pairs `(A, B)` of non-stable bases (capped at `MAX_TRIANGLE_BASES = 10` -> up to 45 triangles per venue, `triangle.rs:116-127`).
     - For each `(A, B)`, constructs a 3-leg cycle with a quote stablecoin (`USDC`/`USDT`/`USD`):
       `leg_symbols = [ "{A}-{Q}", "{A}-{B}", "{B}-{Q}" ]` (`triangle.rs:138-149`).
       **Crucial filter (`triangle.rs:101-106`, `128-136`):** If a venue reports >3 active markets, `plan_venue_triangles` requires the cross-base pair (`"{A}-{B}"` or `"{B}-{A}"`) to actually exist in `listed_pairs` (`markets`)!
       - **Why single-venue triangle scan returns 0 rows on perp-only testnet venues (`tn_hl`, `tn_lighter`, `tn_binance`, `tn_vanta`, etc.):** Perpetual DEXes/CEXes only list `BASE-USDC` / `BASE-USDT` contracts and **never** list cross-base pairs like `BTC-ETH` or `ETH-SOL`. Thus `has_cross_pair` (`triangle.rs:130-136`) is `false` for every `(A, B)` pair on perp venues!
  3. **Multi-venue DEX triangles (`plan_multi_venue_triangles`, `triangle.rs:413-525`):**
     - Only considers venues in `DEX_TRIANGLE_VENUES` (`triangle.rs:31-42`: `uniswap_v3`, `pancakeswap_v3`, `aerodrome`, `velodrome`, `camelot_v3`, `sushiswap_v3`, `curve`, `balancer_v2`, `maverick_v2`, `traderjoe_v2`).
     - None of those EVM AMM venues exist in Testnet mode (`state.rs:2080-2131`), so multi-venue DEX triangles are **Live-only**.
  4. **Orderbook lookup without REST fallback (`triangle.rs:369`, `210-250`):**
     - `scan_triangular_opportunities_impl` calls `evaluate_venue_triangle_plan(..., rest_fallback = false)` (`triangle.rs:369`), which calls `resolve_leg_orderbook(..., allow_rest = false)` (`triangle.rs:210-231`) with `TRIANGLE_HUB_BOOK_MAX_AGE = Duration::from_secs(2)` (`triangle.rs:47`).
     - Even if a venue listed a cross-base pair `BTC-ETH` (e.g., Kraken, OKX, Coinbase, Alpaca), `monitor.rs:1986-2016` keys lanes by `canonical_base` (`"BTC"`) and binds `fetch_symbol` to the venue's modal quote (`USD`/`USDC`/`USDT`), so **neither `monitor.rs` nor `feed.rs` ever polls cross-base books like `BTC-ETH` into `market_data_hub`**, and with `rest_fallback = false` and a 2-second hub TTL, `resolve_leg_orderbook` always returns `None` during scans! (By contrast, `execute_triangle_arbitrage_impl` at `triangle.rs:733` passes `rest_fallback = true`.)
- **Executor (`execute_triangle_arbitrage_impl`, `triangle.rs:645-1154`):**
  1. Checks `live_entry_gate(state)` (`693`), circuit breaker (`701`), and `execution_lock` (`706`).
  2. Re-evaluates the triangle plan with `rest_fallback = true` (`725-739`) and verifies `opp.is_executable && opp.net_profit_bps > 0.0` (`774-782`).
  3. Checks `blocking_signals`, `ticker_reduce_only`, and ensures all 3 leg symbols are flat on the venue (`822-887`), then verifies fresh top-of-book slippage (`891-912`) and sets leverage if applicable (`919-931`).
  4. Persists a durable signal execution (`id = "tri-{ts}-{rand16}"`, `strategy_id = "triangle"`, `947-990`) and sequentially dispatches Leg 1 -> Leg 2 -> Leg 3 (`1009-1039`), scaling Leg 2 and Leg 3 quantities by the actual fill ratio of the preceding leg (`Leg 1 fill_ratio` -> `Leg 2`, `Leg 2 fill_ratio` -> `Leg 3`, `1014-1034`).
  5. If Leg 2 or Leg 3 fails or partially fills after Leg 1 filled, marks the signal `SignalStatus::Imbalanced` (`1097-1114`) so `reconciliation.rs` (`dispatch_residual_flatten`) or the operator can unwind the residual legs.
- **Note on `dev_rpc.rs` (`frontend/src-tauri/src/dev_rpc.rs:148-231`):**
  - `dev_rpc.rs` exposes `scan_spatial`, `scan_spot`, `scan_funding`, `execute_spatial`, and `execute_funding_carry`, but **does not wire RPC commands for `scan_triangle`, `execute_triangle`, or `execute_spot`**.