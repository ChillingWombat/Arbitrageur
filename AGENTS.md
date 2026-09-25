# Arbitrageur — Agent Rules

## Live mode is the product

- **Every feature request means live mode first.** Real venues, real orders,
  real money. Live mode is the primary product path.
- Never ship a capability that works only in testnet while claiming live
  support. If a live path is unsafe or blocked, say so and gate it — don't
  silently deliver testnet-only.
- The environment model is `TradingEnvironment::{Testnet, Live}`
  (`state.env()`), persisted as `trading_mode = testnet|live` (legacy
  `paper`→testnet, `real`→live). Every environment-sensitive read routes
  through `get_venues()`/`get_adapter()` — both are env-scoped. Testnet
  adapters (`tn_*`) use venue demo endpoints; Live adapters (`live_*`) use
  mainnet. `rest_base_url()` exposes an adapter's configured endpoint —
  feed lanes derive WS URLs from it so env never crosses.
- Persisted venue/monitor data is env-scoped (schema v38): every write
  stamps `environment`, every read filters it, and pre-v38 rows stay
  `'unknown'` — never guessed into either env. `'unknown'` open-position
  trackers list under both envs (fail-closed rewatch); `'unknown'`
  scheduled exits never dispatch. State-store accessors take an
  `environment: &str` (or a row struct carrying it) — reject anything
  outside `testnet|live` via `normalize_environment`.
- Testnet availability per venue is uneven (Binance/Hyperliquid/Lighter/
  Robinhood-Lighter/OKX demo-mode/Vanta-Orderly/Decibel/PopDEX/Deribit/
  Arcus/Alpaca-paper have public testnets; Coinbase, Aster, the DexPerp
  long-tail, EVM AMMs, and IBKR do not — IBKR paper is account-level on the
  same gateway, not a separate endpoint). Venue gaps are acceptable —
  silent local simulation in place of testnet is not. Simulated exchanges
  exist only under `#[cfg(test)]` (`test_sims`).

## Audit discipline

- Before answering "does X exist / how does X work," **enumerate the module
  contents first** (`ls` the crate/dir, read `lib.rs` mod lists), then grep.
  Feature names in this codebase are idiosyncratic ("passive unwind" not
  "limit exit") — vocabulary-based searches produce false negatives.
- When reporting "no X exists," state what was searched and what would have
  been found if it existed.

## Venue quirks

- **OKX regional entities.** OKX runs separate API-key namespaces per entity
  domain: `www.okx.com` (global), `app.okx.com` / `us.okx.com` (US & AU),
  `eea.okx.com` / `my.okx.com` (EEA), `tr.okx.com` (TR). A key issued on one
  returns `50119 "API key doesn't exist"` on every other domain — check which
  host the user's browser shows before debugging credentials. Per-account
  `baseUrl` (Region / API Domain dropdown) overrides the adapter's host;
  stored on `ExchangeCredentials.base_url`, applied to public and signed
  calls alike. OKX demo keys additionally need `x-simulated-trading: 1` —
  implemented via `OkxAdapter::for_testnet` (the `tn_okx` adapter); a demo
  key only works on a demo-minted credential pair, never a mainnet key.

## Execution safety (standing)

- Fail-closed on live paths: unverifiable order state → stop, don't stack
  resting orders; never claim fills from submit acknowledgements.
- Venue truth via `fetch_order` polling; order identity is durable state.
- Emergency exits stay market. Everything else follows the configured policy.

## Verification

- Frontend tests: `cd frontend && npm test` (node --test + scripts/test-components.mjs). NOT vitest.
- Backend: `cargo test -p arbitrageur-desktop`, `cargo clippy`, `cargo fmt --check`.
- Release binary: `cargo build --release --bin arbitrageur-desktop --features custom-protocol`; restart via `setsid nohup`.
  Without `custom-protocol`, Tauri compiles a dev-mode binary whose webview loads
  `devUrl` (localhost:1420) → "Could not connect to localhost" unless vite is running.
