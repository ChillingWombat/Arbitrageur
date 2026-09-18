# System Specification: WebSocket Price Feeds & Historical Retention Control

Status: **APPROVED — implementation in progress**

## Decisions Locked (user sign-off)

1. **Transport**: WebSocket-first, REST fallback. Every admitted venue streams
   orderbooks over WS into `MarketDataHub`; REST polling remains as the
   cold-start and stale-feed fallback. A dead WS lane marks its pairs `stale`
   — never fabricated.
2. **Rate-limit truth (verified against live venue docs, 2026-09-16)**:
   WS does **not** bypass rate limits — it **relieves** them. Lighter's docs
   state REST standard = 60 req/min/IP while WS has a separate envelope
   (255 conns/IP, 500 subs/conn, 200 client msgs/min, keepalive frame every
   2 min), and explicitly warn that REST-abuse rate-limits the WS path too
   ("vice versa"). Today's Lighter book poll costs **2 REST calls**
   (`orderBookDetails` + `orderBookOrders`), so moving books to WS frees the
   whole REST budget for execution/market traffic. Same logic holds for
   Hyperliquid (one info connection/IP, push-on-change) and Vanta/Orderly
   (~1s snapshot pushes). Nothing here evades venue policy.
3. **History**: always-on top-of-book recording to the existing SQLite WAL
   `spread_samples`, independent of which UI page is open. SQLite stays the
   store (no DuckDB — the WAL schema, migrations, and math consumers already
   target it).
4. **Retention controls**: System Settings → Execution → *Historical Price
   Data* card with **Max history days** and **Max storage GB**, both
   **default unlimited** (`None`; `0` input normalizes to `None`). Time and
   size limits compose (whichever binds first wins); each is enforced only
   when set.
5. **Durability**: retention settings persist in `state_store` (migration V7)
   and load at boot. Session-only settings were the prior gap — this closes
   it for retention without migrating the whole settings DTO.

## Problem Statement

All ingestion today is REST polling (`SpreadMonitorEngine` lanes +
`fetch_orderbooks_impl` 350 ms cache-first fallback, hub TTL 400 ms).
`ExchangeAdapter::subscribe_orderbook` is a stub on every venue and
`tokio-tungstenite` is a declared but unused dependency. Consequences:

- Lighter-family books burn 2 REST slots each against a 60/min standard IP
  budget (45 usable after the 25% tx reserve) — the watchlist cadence
  degrades to 5–300 s/symbol as symbols grow.
- Quotes go stale between polls; `spread_samples` has gaps exactly where
  the newest venues should be.
- Retention is hardcoded (30 days, 4 M rows, hourly prune in
  `fetch_orderbooks_impl`) with no user control and no GB accounting.

## Solution

### Transport — per-venue WS clients + feed supervisor

A supervisor task per admitted venue owns **one** WS connection and
multiplexes all watchlist symbols on it:

| Venue | URL | Subscribe | Messages |
|---|---|---|---|
| Hyperliquid | `wss://api.hyperliquid.xyz/ws` | `{"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}` | `{"channel":"l2Book","data":{"coin","levels":[bids,asks],"time"}}` push-on-change; server ping ~30 s (tungstenite auto-pong); max one info connection per IP |
| Lighter (+ Robinhood-Lighter) | `wss://<host>/stream` (derive from adapter `base_url` by scheme swap; RH host `api.rh.lighter.xyz`) | `{"type":"subscribe","channel":"order_book/{market_id}"}` (market_id via existing `orderBookDetails` resolution) | snapshot on subscribe, then `type:"update/order_book"` deltas ~50 ms with `begin_nonce`/`nonce` continuity; gap (`begin_nonce != last nonce`) → resubscribe; client frame at least every 2 min keepalive |
| Vanta (Orderly) | `wss://ws-evm.orderly.org/ws/stream/{account_id}` | `{"id","topic":"PERP_<BASE>_USDC@orderbook","event":"subscribe"}` | depth-100 snapshot push ~1 s: `{topic,ts,data:{symbol,asks:[[px,qty]],bids}}`; requires account_id — none configured → WS skipped, REST only |

Message handling (shared): parse → reuse each adapter's existing REST
level parsers where shapes match → `validate_adapter_book` identity check →
`MarketDataHub::update_orderbook` → sample hook (top-of-book to recorder).
Reconnect: exponential backoff ≤120 s (mirror lane backoff); while
disconnected the lane's books age out of the 400 ms TTL and pairs go
`stale` via the existing path. REST `fetch_orderbook` stays for cold start
and any symbol without a fresh hub book.

Robinhood EVM (chain/router, no CLOB) keeps its current status: no WS book
feed; the Lighter-protocol RH endpoint uses the Lighter WS row above.

### Recorder — always-on

The monitor task records every WS-fed top-of-book into `spread_samples`
under the canonical base (`canonical_base`), merging the `BASE`,
`BASE-USDC`, `BASE-USD` aliases exactly as today. Recording is best-effort:
store failures never break ingestion. No new schema for samples.

### Retention — settings, durability, enforcement

- `EngineSettingsDto` (Rust + TS mirror): `spread_samples_max_days:
  Option<u32>`, `spread_samples_max_gb: Option<f64>`; defaults `None`.
  Validation: reject NaN/negative/infinite; `0` normalizes to `None`.
- Durability: new `key_value` table (migration V7:
  `CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
  `save_engine_settings_impl` upserts `retention.max_days` /
  `retention.max_gb`; `AppEngineState` boot loads them into settings.
- Enforcement (existing hourly prune slot in `fetch_orderbooks_impl`
  extended): if days set → `DELETE WHERE ts_ms < now - days`; if GB set →
  measure via `PRAGMA page_count` × `PRAGMA page_size`, delete oldest rows
  in 100k batches until under budget (one extra batch bound for safety).
  Both unset → prune is a no-op (oldest data accumulates).
- UI: Execution tab *Historical Price Data* card — two numeric inputs with
  "No limit" hints, empty = unlimited; save path reuses
  `save_engine_settings`.

### IPC surface

- No new commands. `get_engine_settings` / `save_engine_settings` carry
  the two fields; `get_monitor_snapshot` unchanged. Retention state
  observable via existing `spread_samples` counts (SignalTrend footer).

## Failure & Honesty Constraints

- WS gap, malformed frame, unknown symbol, failed validation → drop frame,
  resubscribe/backoff; pairs go `stale`, never interpolated.
- Unknown-fee/unknown-funding rows keep current fail-closed behavior.
- Prune failures are best-effort and logged, never break ingestion.
- Live mode: feeds may run for research visibility; execution stays
  disabled per existing gates.

## Testing Decisions

- **Unit (exchanges)**: fixture JSON per venue → parsed `OrderBook`
  equality; Lighter nonce-gap → resubscribe signal; Vanta broker-suffix
  rejection; WS-URL derivation incl. RH host.
- **Backend (desktop)**: prune honors days-only / GB-only / both / neither
  (in-memory store + temp file size); settings validation (0→None,
  negative/NaN rejected); boot loads persisted retention; IPC round-trip.
- **Frontend**: retention card renders unlimited state, saves `None` on
  empty, clamps invalid input; no other page affected.
- No network in tests — fixtures only (prior art: Vanta fixture parser
  tests, monitor evaluate tests).

## Out of Scope

- Live trading, signing, execution changes.
- WS private/user streams, funding WS channels (funding stays REST/EMA).
- DuckDB or second store; backfill of history older than retention.
- Migrating the rest of `EngineSettingsDto` to durable storage.
- Robinhood-EVM on-chain pool quotes (unchanged).

## Rollout

Land transport + recorder first (REST fallback keeps behavior identical on
failure), then retention settings + enforcement, then this spec's
verification gates: `cargo test -p arbitrageur-desktop`, `cargo clippy
--workspace -- -D warnings`, `tsc --noEmit`, `npm test`.
