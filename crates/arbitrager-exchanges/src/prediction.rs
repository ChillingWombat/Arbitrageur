use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tracing::info;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, OrderStatus, VenueId};

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
            client: Client::builder().build().unwrap_or_default(),
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
        let bid = book.best_bid_price().unwrap_or(dec!(0.41));
        let ask = book.best_ask_price().unwrap_or(dec!(0.42));

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            volume_24h: dec!(500000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Prediction markets do not have funding rates")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let equity = dec!(5000.0);
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: equity,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free: equity,
                locked: dec!(0),
            }],
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        info!("[Polymarket] Executing order: {:?}", order);
        Ok(OrderResponse {
            order_id: format!("pm-{}", uuid::Uuid::new_v4()),
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::Filled,
            original_qty: order.quantity,
            filled_qty: order.quantity,
            remaining_qty: dec!(0),
            avg_fill_price: order.price,
            fee: dec!(0.001) * order.quantity * order.price.unwrap_or(dec!(0.5)),
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        info!("[Polymarket] Cancel order: {:?}", req);
        Ok(())
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
            client: Client::builder().build().unwrap_or_default(),
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
        let bid = book.best_bid_price().unwrap_or(dec!(0.56));
        let ask = book.best_ask_price().unwrap_or(dec!(0.57));

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            volume_24h: dec!(350000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Kalshi does not have perpetual funding rates")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let equity = dec!(5000.0);
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: equity,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USD".to_string(),
                total: equity,
                free: equity,
                locked: dec!(0),
            }],
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        info!("[Kalshi] Executing order: {:?}", order);
        Ok(OrderResponse {
            order_id: format!("kalshi-{}", uuid::Uuid::new_v4()),
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::Filled,
            original_qty: order.quantity,
            filled_qty: order.quantity,
            remaining_qty: dec!(0),
            avg_fill_price: order.price,
            fee: dec!(0.0007) * order.quantity * order.price.unwrap_or(dec!(0.5)),
            fee_currency: "USD".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        info!("[Kalshi] Cancel order: {:?}", req);
        Ok(())
    }
}
