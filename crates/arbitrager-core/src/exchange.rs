use async_trait::async_trait;
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

    /// Fetch latest L2 orderbook snapshot.
    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook>;

    /// Fetch latest ticker.
    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker>;

    /// Fetch current funding rate (for perpetual contracts).
    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate>;

    /// Fetch consolidated account balance, equity and positions.
    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot>;

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
}
