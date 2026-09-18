use async_trait::async_trait;
use std::collections::HashMap;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::MarketMetadata;
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::AccountSnapshot;
use arbitrager_core::quote::{FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{Decimal, VenueId};
use crate::lighter::LighterAdapter;

/// Robinhood DEX adapter, serving the Lighter protocol on Robinhood Chain
/// (at https://api.rh.lighter.xyz).
#[allow(dead_code)]
pub struct RobinhoodAdapter {
    inner: LighterAdapter,
}

impl RobinhoodAdapter {
    pub fn new(
        base_url: Option<String>,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
        _extra: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.rh.lighter.xyz".to_string());
        Self {
            inner: LighterAdapter::new_with_venue(
                Some(base),
                account_index,
                api_key_private_key,
                "robinhood",
            ),
        }
    }

    pub fn update_credentials(
        &self,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
    ) {
        self.inner.update_credentials(account_index, api_key_private_key);
    }
}

#[async_trait]
impl ExchangeAdapter for RobinhoodAdapter {
    fn venue_id(&self) -> VenueId {
        self.inner.venue_id()
    }

    fn name(&self) -> &str {
        "Robinhood"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        self.inner.fetch_markets().await
    }

    async fn ping(&self) -> anyhow::Result<()> {
        self.inner.ping().await
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        self.inner.fetch_orderbook(symbol).await
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        self.inner.fetch_ticker(symbol).await
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        self.inner.fetch_funding_rate(symbol).await
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        self.inner.fetch_funding_rates().await
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        self.inner.fetch_account().await
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        self.inner.fetch_volumes_24h().await
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        self.inner.place_order(order).await
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        self.inner.cancel_order(req).await
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        self.inner.set_leverage(symbol, leverage).await
    }

    fn supports_live_execution(&self) -> bool {
        self.inner.supports_live_execution()
    }

    fn has_live_credentials(&self) -> bool {
        self.inner.has_live_credentials()
    }
}
