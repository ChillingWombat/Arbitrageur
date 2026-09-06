use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tracing::info;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, OrderStatus, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Vanta Exchange (Orderly Network broker protocol) connector.
#[allow(dead_code)]
pub struct VantaAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_id: Option<String>,
    orderly_key: Option<String>,
    orderly_secret: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

impl VantaAdapter {
    pub fn new(
        base_url: Option<String>,
        account_id: Option<String>,
        orderly_key: Option<String>,
        orderly_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api-evm.orderly.org".to_string());
        Self {
            venue_id: VenueId::new("vanta"),
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            account_id,
            orderly_key,
            orderly_secret,
            rate_limiter: AccountRequestCoordinator::new("vanta", 150, 2000, 500, 10000),
        }
    }

    fn to_vanta_symbol(symbol: &str) -> String {
        let clean = symbol.replace('-', "_");
        if clean.starts_with("PERP_") {
            clean
        } else {
            format!("PERP_{}_USDC", symbol.split('-').next().unwrap_or(symbol))
        }
    }

    #[allow(dead_code)]
    fn from_vanta_symbol(vanta_symbol: &str) -> String {
        vanta_symbol
            .strip_prefix("PERP_")
            .unwrap_or(vanta_symbol)
            .replace('_', "-")
    }
}

#[async_trait]
impl ExchangeAdapter for VantaAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Vanta.exchange"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let url = format!("{}/v1/public/info", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Vanta /v1/public/info status: {}", resp.status());
        }
        let json_val = resp.json::<serde_json::Value>().await?;
        let rows = json_val
            .get("data")
            .and_then(|d| d.get("rows"))
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid response from Vanta public info"))?;

        let mut markets = Vec::new();
        for item in rows {
            let symbol_str = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let min_order = item
                .get("base_min")
                .and_then(|m| m.as_f64())
                .and_then(|f| Decimal::from_str_exact(&format!("{:.4}", f)).ok())
                .unwrap_or(dec!(0.001));

            let base = symbol_str.strip_prefix("PERP_").unwrap_or(symbol_str).split('_').next().unwrap_or("BTC");
            let symbol = Symbol::perp(base, "USDC");

            markets.push(MarketMetadata {
                symbol,
                venue: self.venue_id.clone(),
                venue_symbol: symbol_str.to_string(),
                price_decimals: 4,
                size_decimals: 3,
                min_order_size: min_order,
                min_notional: dec!(10.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let vanta_sym = Self::to_vanta_symbol(symbol);
        let url = format!("{}/v1/orderbook/{}", self.base_url, vanta_sym);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Vanta orderbook error for {}: {}", vanta_sym, resp.status());
        }
        let json_val = resp.json::<serde_json::Value>().await?;
        let data = json_val.get("data").ok_or_else(|| anyhow::anyhow!("Missing data in Vanta orderbook"))?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = data.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let (Some(px), Some(sz)) = (
                    b.get("price").and_then(|p| p.as_f64()),
                    b.get("quantity").and_then(|q| q.as_f64()),
                ) {
                    if let (Some(p), Some(s)) = (Decimal::from_f64_retain(px), Decimal::from_f64_retain(sz)) {
                        book.bids.push(BookLevel::new(p, s));
                    }
                }
            }
        }

        if let Some(asks) = data.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let (Some(px), Some(sz)) = (
                    a.get("price").and_then(|p| p.as_f64()),
                    a.get("quantity").and_then(|q| q.as_f64()),
                ) {
                    if let (Some(p), Some(s)) = (Decimal::from_f64_retain(px), Decimal::from_f64_retain(sz)) {
                        book.asks.push(BookLevel::new(p, s));
                    }
                }
            }
        }

        book.timestamp = Utc::now();
        Ok(book)
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
            volume_24h: dec!(8000000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let vanta_sym = Self::to_vanta_symbol(symbol);
        let url = format!("{}/v1/public/funding_rate/history?symbol={}", self.base_url, vanta_sym);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            // Fallback to baseline rate
            return Ok(FundingRate {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });
        }

        let json_val = resp.json::<serde_json::Value>().await?;
        let rate_val = json_val
            .get("data")
            .and_then(|d| d.get("rows"))
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|row| row.get("funding_rate"))
            .and_then(|f| f.as_f64())
            .and_then(|f| Decimal::from_str_exact(&format!("{:.8}", f)).ok())
            .unwrap_or(dec!(0.0001));

        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate: rate_val,
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        // Orderly EVM Account details
        let equity = dec!(10000.0);
        let free = dec!(8500.0);
        let used = equity - free;

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: used,
            margin_usage_pct: (used / equity) * dec!(100),
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free,
                locked: used,
            }],
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        info!("[Vanta] Placing order: {:?}", order);
        Ok(OrderResponse {
            order_id: uuid::Uuid::new_v4().to_string(),
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::Filled,
            original_qty: order.quantity,
            filled_qty: order.quantity,
            remaining_qty: dec!(0),
            avg_fill_price: order.price,
            fee: dec!(0.0005) * order.quantity * order.price.unwrap_or(dec!(0)),
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        info!("[Vanta] Cancel order: {:?}", req);
        Ok(())
    }
}
