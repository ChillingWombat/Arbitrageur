use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, VenueId};

/// Polymarket CTF CLOB adapter for binary prediction markets.
#[allow(dead_code)]
pub struct PolymarketAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl PolymarketAdapter {
    pub fn new(base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            venue_id: VenueId::new("polymarket"),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://clob.polymarket.com".to_string()),
            api_key,
        }
    }
}

#[async_trait]
impl ExchangeAdapter for PolymarketAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Polymarket"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let symbol = Symbol::prediction_binary("FED-CHAIR-WARSH-YES", "USDC");
        Ok(vec![MarketMetadata {
            symbol,
            venue: self.venue_id.clone(),
            venue_symbol: "0x1234...poly".to_string(),
            price_decimals: 2,
            size_decimals: 2,
            min_order_size: dec!(5.0),
            min_notional: dec!(1.0),
            maker_fee_bps: dec!(0.0),
            taker_fee_bps: dec!(10.0),
            contract_size: dec!(1.0),
            is_active: true,
        }])
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        // Typical prediction market prices: e.g. 42 cents YES
        book.bids.push(BookLevel::new(dec!(0.41), dec!(1000.0)));
        book.bids.push(BookLevel::new(dec!(0.40), dec!(2500.0)));

        book.asks.push(BookLevel::new(dec!(0.42), dec!(1500.0)));
        book.asks.push(BookLevel::new(dec!(0.43), dec!(3000.0)));

        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Prediction markets do not have funding rates")
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

/// Kalshi CFTC-regulated prediction market exchange adapter.
#[allow(dead_code)]
pub struct KalshiAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl KalshiAdapter {
    pub fn new(base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            venue_id: VenueId::new("kalshi"),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://trading-api.kalshi.com".to_string()),
            api_key,
        }
    }
}

#[async_trait]
impl ExchangeAdapter for KalshiAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Kalshi"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let symbol = Symbol::prediction_binary("FED-CHAIR-WARSH-NO", "USD");
        Ok(vec![MarketMetadata {
            symbol,
            venue: self.venue_id.clone(),
            venue_symbol: "kxfedchairnom-29".to_string(),
            price_decimals: 2,
            size_decimals: 0,
            min_order_size: dec!(1.0),
            min_notional: dec!(1.0),
            maker_fee_bps: dec!(0.0),
            taker_fee_bps: dec!(7.0),
            contract_size: dec!(1.0),
            is_active: true,
        }])
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        // Kalshi NO price: e.g. 56 cents bid, 57 cents ask
        book.bids.push(BookLevel::new(dec!(0.56), dec!(2000.0)));
        book.bids.push(BookLevel::new(dec!(0.55), dec!(4000.0)));

        book.asks.push(BookLevel::new(dec!(0.57), dec!(1200.0)));
        book.asks.push(BookLevel::new(dec!(0.58), dec!(3500.0)));

        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Kalshi does not have perpetual funding rates")
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
