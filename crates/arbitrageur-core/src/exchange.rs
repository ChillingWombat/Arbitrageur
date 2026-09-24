use crate::market::MarketMetadata;
use crate::order::{CancelRequest, OrderRequest, OrderResponse};
use crate::position::{AccountSnapshot, Position};
use crate::quote::{FundingRate, OrderBook, Ticker};
use crate::types::VenueId;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;
use tokio::sync::broadcast;

/// Unified async exchange connector interface.
#[async_trait]
pub trait ExchangeAdapter: Send + Sync {
    /// Unique identifier of the venue.
    fn venue_id(&self) -> VenueId;

    /// Human readable name.
    fn name(&self) -> &str;

    /// Account identifier for venues whose public market-data WS stream is
    /// account-scoped (Orderly brokers such as Vanta). Default `None` —
    /// most venues' public streams are anonymous.
    fn ws_account_id(&self) -> Option<String> {
        None
    }

    /// The adapter's configured REST base URL — the authoritative endpoint
    /// this instance talks to. Feed lanes derive WS URLs from it so a
    /// testnet-scoped adapter never leaks mainnet books into the hub.
    /// Default `None` for adapters without a single REST base.
    fn rest_base_url(&self) -> Option<String> {
        None
    }

    /// List available markets and instrument metadata.
    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>>;

    /// Best-effort reference price per `MarketMetadata::venue_symbol`.
    ///
    /// This is used to seed the dynamic ticker universe, not for execution.
    /// Adapters with a bulk mark/last-price endpoint should override it;
    /// returning an empty map is valid and callers must never invent a price.
    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        Ok(HashMap::new())
    }

    /// Connectivity probe used for latency measurement. The request must
    /// reach the exchange — implementations must bypass local response
    /// caches and CDN edge caching so the round-trip is meaningful.
    /// Default reuses `fetch_markets()`.
    async fn ping(&self) -> anyhow::Result<()> {
        self.fetch_markets().await.map(|_| ())
    }

    /// Fetch latest L2 orderbook snapshot.
    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook>;

    /// Fetch latest ticker.
    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker>;

    /// Fetch current funding rate (for perpetual contracts).
    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate>;

    /// Fetch funding rates for all markets in one request. Adapters with a
    /// bulk endpoint (Hyperliquid metaAndAssetCtxs, Lighter funding-rates)
    /// override this; the default returns an empty list so callers can use
    /// per-symbol `fetch_funding_rate` as a fallback. Returning rates for a
    /// venue's own exchange only — third-party reference rates are excluded.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        Ok(Vec::new())
    }

    /// Fetch consolidated account balance, equity and positions.
    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot>;

    /// Normalize one venue-native account row into the engine/canonical
    /// symbol and quantity units that `OrderRequest` uses. `Ok(None)` means
    /// the row belongs to a market class this adapter intentionally does not
    /// manage (for example inverse futures). Mapping failures error instead
    /// of pretending the instrument is flat or guessing a default market.
    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        Ok(Some(position.clone()))
    }

    /// Return `account`'s position for an engine/canonical symbol. Multiple
    /// rows resolving to the same instrument are ambiguous and fail closed.
    fn account_position_for(
        &self,
        account: &AccountSnapshot,
        symbol: &str,
    ) -> anyhow::Result<Option<Position>> {
        let requested = symbol.trim();
        let mut matched: Option<Position> = None;
        for raw in &account.positions {
            let Some(position) = self.normalize_account_position(raw)? else {
                continue;
            };
            // Zero is flat; any nonzero magnitude is exposure. Convention is
            // unsigned `quantity` + `side`, but a negative row must still
            // block entries rather than read as flat.
            if position.quantity == Decimal::ZERO
                || !position.symbol.eq_ignore_ascii_case(requested)
            {
                continue;
            }
            if matched.is_some() {
                anyhow::bail!(
                    "{} account has multiple positions resolving to {}; refusing to guess exposure",
                    self.venue_id(),
                    requested
                );
            }
            let mut position = position;
            position.symbol = requested.to_string();
            matched = Some(position);
        }
        Ok(matched)
    }

    /// Fetch 24h notional trading volume (quote currency, ~USD) per venue
    /// asset name. Venues that do not expose bulk volume statistics return an
    /// empty map and simply contribute nothing to cross-venue aggregates.
    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        Ok(HashMap::new())
    }

    /// Submit a single order.
    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse>;

    /// The base-unit quantity this venue would actually trade for `qty`
    /// after its own wire-grid normalization — the largest grid-aligned
    /// quantity `<= qty` (lot step, contract size, size decimals, wad
    /// precision). Each adapter independently normalizes inside
    /// `place_order`, so an un-snapped quantity can fill different sizes
    /// on the two legs of a hedged entry — a naked residual by
    /// construction. Two-leg entries snap to the finer common grid via
    /// this method and refuse when the grids are irreconcilable.
    /// Default: `qty` unchanged (venues that pass quantity through raw).
    async fn wire_base_qty(&self, _symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        Ok(qty)
    }

    /// Whether the venue accepts `reduce_only` on an order for `symbol`.
    /// Spot instruments have no position model — the venue itself refuses
    /// an oversell, which is the same guarantee the flag buys on perps —
    /// so the flag must stay unset rather than be rejected as unsupported.
    /// Default: `true` (perp-style venues).
    fn supports_reduce_only(&self, _symbol: &str) -> bool {
        true
    }

    /// Cancel an existing order.
    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()>;

    /// Poll a previously submitted order, returning the venue-attributed
    /// current state (status, cumulative fill quantity, average fill price,
    /// fee when reported). `symbol` is the engine's canonical symbol;
    /// adapters map it to the venue spelling internally. `order_id` is the
    /// persisted venue order id; `client_order_id` is the engine-generated
    /// id persisted alongside it — venues that key orders by client index
    /// (Lighter) require it, and other venues may cross-check it.
    /// Default: fail-closed — venues without an order-status endpoint report
    /// unsupported so the reconciliation loop skips them rather than
    /// assuming a state.
    async fn fetch_order(
        &self,
        _symbol: &str,
        _order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(
            "Order-status polling is not implemented for venue {}",
            self.name()
        )
    }

    /// Subscribe to live L2 orderbook updates via WebSocket.
    async fn subscribe_orderbook(
        &self,
        _symbols: &[String],
    ) -> anyhow::Result<broadcast::Receiver<OrderBook>> {
        anyhow::bail!(
            "WebSocket orderbook streaming not implemented for venue {}",
            self.name()
        );
    }

    /// Subscribe to live funding rate updates via WebSocket.
    async fn subscribe_funding(
        &self,
        _symbols: &[String],
    ) -> anyhow::Result<broadcast::Receiver<FundingRate>> {
        anyhow::bail!(
            "WebSocket funding streaming not implemented for venue {}",
            self.name()
        );
    }

    /// Set or update leverage for a given symbol/perp contract.
    /// Default implementation fails closed: venues without a leverage API must not silently pretend to apply it.
    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!(
            "Leverage updates are not implemented for venue {}",
            self.name()
        );
    }

    /// Whether this adapter has implemented live order placement and cancellation.
    fn supports_live_execution(&self) -> bool {
        false
    }

    /// Whether `place_order` genuinely submits to the venue. Several adapters
    /// report `supports_live_execution() == true` while their `place_order`
    /// is still a fail-closed stub, so live preflights must consult THIS,
    /// not that flag, before promising anything submittable.
    fn live_order_submission_implemented(&self) -> bool {
        false
    }

    /// Whether `fetch_order` returns authoritative venue state for a
    /// persisted order id — the adapter-level primitive the reconciliation
    /// supervisor polls. True only when the venue endpoint is wired and its
    /// response is validated fail-closed.
    fn order_status_polling_implemented(&self) -> bool {
        false
    }

    /// Whether the adapter participates in live order reconciliation. The
    /// desktop supervisor provides the machinery — terminal-state polling,
    /// restart recovery, residual-exposure blocking, opt-in residual
    /// auto-resolution, and bounded cancel retry — so the per-adapter
    /// requirement reduces to a working `fetch_order`.
    fn live_order_reconciliation_implemented(&self) -> bool {
        self.order_status_polling_implemented()
    }

    /// Whether this adapter is safe for automated multi-leg strategy entry.
    /// Submission transport + polling are necessary but not sufficient (venues
    /// may still lack gas/friction modeling or venue evidence).
    fn live_strategy_entry_implemented(&self) -> bool {
        self.live_order_reconciliation_implemented()
    }

    /// Whether authenticated live trading credentials are loaded in memory for this adapter.
    fn has_live_credentials(&self) -> bool {
        false
    }
}
