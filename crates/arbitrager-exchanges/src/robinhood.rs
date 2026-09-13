use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::AccountSnapshot;
use arbitrager_core::quote::{FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, VenueId};

/// Robinhood EVM DEX & Bridge router adapter for Lighter cross-chain trading.
#[allow(dead_code)]
pub struct RobinhoodAdapter {
    venue_id: VenueId,
    client: Client,
    rpc_url: String,
    router_address: String,
    wallet_address: Option<String>,
    private_key: Option<String>,
}

impl RobinhoodAdapter {
    pub fn new(
        rpc_url: Option<String>,
        router_address: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> Self {
        Self {
            venue_id: VenueId::new("robinhood"),
            client: Client::builder().build().unwrap_or_default(),
            rpc_url: rpc_url.unwrap_or_else(|| "https://rpc.robinhood.network".to_string()),
            // Default Uniswap V3 / Taoli Router on Robinhood EVM
            router_address: router_address.unwrap_or_else(|| "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba".to_string()),
            wallet_address,
            private_key,
        }
    }
}

#[async_trait]
impl ExchangeAdapter for RobinhoodAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Robinhood"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let symbols = vec!["ETH-USDC", "BTC-USDC", "SOL-USDC", "DOGE-USDC"];
        let mut markets = Vec::new();
        for sym in symbols {
            let symbol = Symbol::spot(sym.split('-').next().unwrap_or("ETH"), "USDC");
            markets.push(MarketMetadata {
                symbol,
                venue: self.venue_id.clone(),
                venue_symbol: sym.to_string(),
                price_decimals: 4,
                size_decimals: 4,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(1.0),
                taker_fee_bps: dec!(3.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        // Robinhood EVM exposes no public orderbook feed. Previously this
        // proxied Binance bookTicker (and Hyperliquid l2Book on failure)
        // padded with fabricated depth — misattributing another venue's
        // liquidity to `robinhood` and corrupting per-venue spread samples.
        anyhow::bail!(
            "Robinhood orderbook unavailable for {}; refusing to proxy another venue's book",
            symbol
        )
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        let last = (bid + ask) / dec!(2);

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Robinhood EVM is a spot DEX and does not have perpetual funding rates")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
    }
}
