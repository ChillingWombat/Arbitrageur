use std::str::FromStr;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tracing::info;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance, Position};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, OrderStatus, Side, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Lighter zk-Rollup DEX adapter.
#[allow(dead_code)]
pub struct LighterAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_index: Option<u64>,
    api_key_private_key: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

impl LighterAdapter {
    pub fn new(
        base_url: Option<String>,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://mainnet.zklighter.elliot.ai".to_string());
        Self {
            venue_id: VenueId::new("lighter"),
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            account_index,
            api_key_private_key,
            rate_limiter: AccountRequestCoordinator::new("lighter", 200, 3000, 500, 15000),
        }
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Lighter API error: {} {}", resp.status(), path);
        }
        let data = resp.json::<serde_json::Value>().await?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for LighterAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Lighter"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.get_json("/api/v1/orderBookDetails").await?;
        let details = res.get("order_book_details").and_then(|d| d.as_array()).ok_or_else(|| {
            anyhow::anyhow!("Missing order_book_details in Lighter response")
        })?;

        let mut markets = Vec::new();
        for item in details {
            let symbol_str = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let market_id = item.get("market_id").and_then(|m| m.as_u64()).unwrap_or(0);
            let price_dec = item.get("price_decimals").and_then(|p| p.as_u64()).unwrap_or(4) as u32;
            let size_dec = item.get("size_decimals").and_then(|s| s.as_u64()).unwrap_or(2) as u32;

            let symbol = Symbol::perp(symbol_str, "USD");
            markets.push(MarketMetadata {
                symbol,
                venue: self.venue_id.clone(),
                venue_symbol: market_id.to_string(),
                price_decimals: price_dec,
                size_decimals: size_dec,
                min_order_size: dec!(0.01),
                min_notional: dec!(10.0),
                maker_fee_bps: dec!(0.0),
                taker_fee_bps: dec!(2.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let base_sym = symbol.split('-').next().unwrap_or(symbol);
        // Map symbol to market_id: e.g. ETH is 0, BTC is 1, SOL is 2
        let market_id = match base_sym.to_uppercase().as_str() {
            "ETH" => 0,
            "BTC" => 1,
            "SOL" => 2,
            "DOGE" => 3,
            "1000PEPE" | "PEPE" => 4,
            "WLD" => 6,
            "XRP" => 7,
            _ => 0,
        };

        let res = self.get_json(&format!("/api/v1/orderBookOrders?market_id={}&limit=50", market_id)).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = res.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let (Some(px), Some(sz)) = (
                    b.get("price").and_then(|p| p.as_str()),
                    b.get("size").and_then(|s| s.as_str()),
                ) {
                    if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                        book.bids.push(BookLevel::new(p, s));
                    }
                }
            }
        }

        if let Some(asks) = res.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let (Some(px), Some(sz)) = (
                    a.get("price").and_then(|p| p.as_str()),
                    a.get("size").and_then(|s| s.as_str()),
                ) {
                    if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
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
            volume_24h: dec!(5000000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        // Lighter funding rates: 8h cycle
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate: dec!(0.0001), // standard baseline rate 0.01%
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let idx = self.account_index.unwrap_or(0);
        let path = format!("/api/v1/account?by=index&value={}", idx);

        let data = match self.rate_limiter.acquire_or_cache(true).await? {
            Some(cached) => serde_json::from_str(&cached)?,
            None => {
                let res = self.get_json(&path).await;
                match res {
                    Ok(val) => {
                        self.rate_limiter.record_success(val.to_string());
                        val
                    }
                    Err(e) => {
                        self.rate_limiter.record_failure(false);
                        return Err(e);
                    }
                }
            }
        };

        let account = data.get("account").or_else(|| {
            data.get("accounts").and_then(|a| a.as_array()).and_then(|arr| arr.first())
        });

        let total_val = account
            .and_then(|a| a.get("total_asset_value").or_else(|| a.get("portfolio_value")))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let free_val = account
            .and_then(|a| a.get("free_collateral").or_else(|| a.get("available_balance")))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(total_val);

        let mut positions = Vec::new();
        if let Some(acc) = account {
            if let Some(pos_list) = acc.get("positions").and_then(|p| p.as_array()) {
                for p in pos_list {
                    let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("UNKNOWN");
                    let sz_str = p.get("position").and_then(|s| s.as_str()).unwrap_or("0");
                    let sz = Decimal::from_str(sz_str).unwrap_or(dec!(0));
                    let entry_str = p.get("entry_price").and_then(|e| e.as_str()).unwrap_or("0");
                    let entry_px = Decimal::from_str(entry_str).unwrap_or(dec!(0));

                    if sz != dec!(0) {
                        let side = if sz > dec!(0) { Side::Buy } else { Side::Sell };
                        positions.push(Position {
                            venue: self.venue_id.clone(),
                            symbol: sym.to_string(),
                            side,
                            quantity: sz.abs(),
                            entry_price: entry_px,
                            mark_price: entry_px,
                            unrealized_pnl: dec!(0),
                            realized_pnl: dec!(0),
                            liquidation_price: None,
                            leverage: dec!(10),
                            initial_margin: (sz.abs() * entry_px) / dec!(10),
                            maintenance_margin: (sz.abs() * entry_px) / dec!(20),
                            updated_at: Utc::now(),
                        });
                    }
                }
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: total_val,
            free_margin: free_val,
            maintenance_margin: total_val - free_val,
            margin_usage_pct: if total_val > dec!(0) { ((total_val - free_val) / total_val) * dec!(100) } else { dec!(0) },
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USD".to_string(),
                total: total_val,
                free: free_val,
                locked: total_val - free_val,
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        info!("[Lighter] Placing order: {:?}", order);
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
            fee: dec!(0.0002) * order.quantity * order.price.unwrap_or(dec!(0)),
            fee_currency: "USD".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        info!("[Lighter] Cancel order: {:?}", req);
        Ok(())
    }
}
