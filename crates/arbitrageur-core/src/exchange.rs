use async_trait::async_trait;
use std::collections::HashMap;
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use crate::market::MarketMetadata;
use crate::order::{CancelRequest, OrderRequest, OrderResponse};
use crate::position::AccountSnapshot;
use crate::quote::{FundingRate, OrderBook, Ticker};
use crate::types::VenueId;

/// Unified async exchange connector interface.
#[async_trait]
pub trait ExchangeAdapter: Send + Sync {
    /// Unique identifier of the venue.
    fn venue_id(&self) -> VenueId;

    /// Human readable name.
    fn name(&self) -> &str;

    /// List available markets and instrument metadata.
    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>>;

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

    /// Fetch 24h notional trading volume (quote currency, ~USD) per venue
    /// asset name. Venues that do not expose bulk volume statistics return an
    /// empty map and simply contribute nothing to cross-venue aggregates.
    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        Ok(HashMap::new())
    }

    /// Submit a single order.
    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse>;

    /// Cancel an existing order.
    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()>;

    /// Subscribe to live L2 orderbook updates via WebSocket.
    async fn subscribe_orderbook(&self, _symbols: &[String]) -> anyhow::Result<broadcast::Receiver<OrderBook>> {
        anyhow::bail!("WebSocket orderbook streaming not implemented for venue {}", self.name());
    }

    /// Subscribe to live funding rate updates via WebSocket.
    async fn subscribe_funding(&self, _symbols: &[String]) -> anyhow::Result<broadcast::Receiver<FundingRate>> {
        anyhow::bail!("WebSocket funding streaming not implemented for venue {}", self.name());
    }

    /// Set or update leverage for a given symbol/perp contract.
    /// Default implementation fails closed: venues without a leverage API must not silently pretend to apply it.
    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Leverage updates are not implemented for venue {}", self.name());
    }

    /// Whether this adapter has implemented live order placement and cancellation.
    fn supports_live_execution(&self) -> bool {
        false
    }

    /// Whether authenticated live trading credentials are loaded in memory for this adapter.
    fn has_live_credentials(&self) -> bool {
        false
    }
}
