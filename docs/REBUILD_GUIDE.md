# Code Walkthrough — Rebuild the System Yourself (Practice Guide)

> Practice path — rebuild layers in the order given; new code follows the tutorial-comment pattern.


> Purpose: the owner's request — "detailed comments so I can understand and
> recreate the system to practice". This guide narrates the whole codebase
> layer-by-layer: what each part does, which file to open first, and the
> smallest rebuild order that yields a working skeleton. New code (rebalance
> estimator, gadget) is commented line-by-line in-source as the pattern to copy.

## 0. Big picture (30 seconds)

```
Tauri desktop app (Rust backend + React frontend in `frontend/`)
  ├── Rust crates (`crates/`)  — math, venues, strategy, execution, risk
  ├── React UI (`frontend/src/`) — pages per strategy + Portfolio Monitor
  └── SQLite (`~/.arbitrageur/state.sqlite3`) — configs, samples, logs
```

Data flows one way: venue adapters → `market_data_hub` snapshot → strategy
evaluators (spread / funding / triangle) → paper executor + SQLite → React
polls Tauri commands every ~3s. Nothing touches real funds: native execution
is disabled backend-wide and every adapter fails closed when unconfigured.

## 1. Rust backend — open in this order

| Order | Crate / file | What it does | Key symbol to read first |
|---|---|---|---|
| 1 | `crates/arbitrageur-math/src/` | Pure quant: funding APR/APY, capacity tiers, `book_gradient` (marginal bps/$1k), win probability | `book_gradient.rs::entry_gradient` |
| 2 | `crates/arbitrageur-exchanges/src/` | One adapter per venue + `wallet.rs` (EVM signer) + `variational.rs` / `popdex.rs` (no-API venues) | `lib.rs` (adapter trait), `wallet.rs::EvmWalletSigner` |
| 3 | `crates/arbitrageur-strategy/src/` | Spread / funding-carry / basis evaluators incl. capacity + win-prob wiring | `evaluate_funding_carry` |
| 4 | `crates/arbitrageur-execution/src/` | Paper executor only (native fn returns "unavailable" refusal) | paper fill path |
| 5 | `crates/arbitrageur-risk/src/` | Circuit breaker, margin health, DeltaGuard neutrality | breaker + margin fns |
| 6 | `crates/arbitrageur-app/` + `frontend/src-tauri/src/` | Tauri commands (`get_venues`, `get_open_positions`, `funding_rate_series` …), vault, retention | `commands.rs`, `state.rs` |

Rebuild drill: implement math crate first with `cargo test` green, then one
simulated venue adapter, then a single Tauri command returning its quotes.

## 2. Frontend — open in this order

| Order | File | What it does |
|---|---|---|
| 1 | `frontend/src/services/api.ts` | All Tauri IPC in one place (`invokeDesktop`) — every gadget reads through here |
| 2 | `frontend/src/types/index.ts` + `types/tickers.ts` | Shared DTOs (`VenueBalanceInfo`, `OpenPositionDto` …) — the backend/frontend contract |
| 3 | `frontend/src/services/rebalanceFees.ts` | **Heavily commented example**: static research table + pure estimator + confidence labels |
| 4 | `frontend/src/components/RebalanceEstimatorGadget.tsx` | **Heavily commented example**: form → useMemo(estimate) → receipt UI |
| 5 | `frontend/src/components/PortfolioMonitor.tsx` | Health → Capital → Rebalance → Attention → DeltaGuard → venues → attribution |
| 6 | `frontend/src/components/ExchangeVenueGadget.tsx`, `FundingRadar.tsx`, `LiveMatrix.tsx` | Per-area monitors (same card/header system) |
| 7 | `frontend/src/services/exchangeStats.ts`, `matrixQuant.ts`, `contractSpecs.ts` | Quant + aggregation helpers with unit tests (node:test) |

Rebuild drill: `api.ts` stub returning canned JSON → render `PortfolioMonitor`
sections one at a time → add `rebalanceFees.ts` + gadget (no backend needed).

## 3. Conventions to copy (keeps the UI consistent)

- Font: Inter for every character (frontend token `font-mono` maps to Inter).
- Color: light-blue `bg-accent-dim` = selected/recommended; blue `bg-accent` = buttons only.
- Fail-closed: unconfigured/unknown → "—" or null + warning, never fabricated 0.
- Tests: `node:test` + `assert/strict`, colocated `*.test.ts`, run via `node --test`.
- Typecheck: `npx tsc --noEmit` from `frontend/` must be clean before commit.

## 4. What is NOT yet commented line-by-line (honest scope note)

The repo predates this request (hundreds of files). Full line-by-line comments
across every legacy file is ongoing work, not finished here. What IS done now:
every new file in this session carries tutorial-grade comments, and this guide
maps the rest. Suggested next practice task: pick one legacy file per session
(e.g. `matrixQuant.ts`) and add the same header + section-comment style.
