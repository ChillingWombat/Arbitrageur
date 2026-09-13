use std::collections::HashMap;
use std::str::FromStr;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance, Position};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, Side, VenueId};
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
        // 1. Check rate limiter and in-memory single-flight cache
        if let Ok(Some(cached_str)) = self.rate_limiter.acquire_or_cache_path(path, true).await {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };

        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Lighter API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
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
        let clean_sym = symbol.replace('/', "-").to_uppercase();
        let base_sym = clean_sym.split('-').next().unwrap_or(&clean_sym);
        
        // Map symbol to market_id: Crypto perps + Lighter Robinhood synthetic equities
        let market_id = if let Ok(numeric_id) = clean_sym.parse::<u64>() {
            numeric_id
        } else {
            match base_sym {
                // Perps
                "ETH" if clean_sym.contains("PERP") || !clean_sym.contains("SPOT") => 0,
                "BTC" => 1,
                "SOL" => 2,
                "DOGE" => 3,
                "1000PEPE" | "PEPE" => 4,
                "WLD" => 6,
                "XRP" => 7,
                // Robinhood Lighter Tokenized Equities (Web3 -> Traditional)
                "RHNVDA" | "NVDA" => 2056,
                "RHSPY" | "SPY" => 2057,
                "RHQQQ" | "QQQ" => 2058,
                // Lighter Spot
                "ETH" => 2048,
                "LIT" => 2049,
                "LINK" => 2050,
                "UNI" => 2051,
                "AAVE" => 2052,
                "SKY" => 2053,
                "LDO" => 2054,
                "AZTEC" => 2055,
                _ => 1,
            }
        };

        let res = self.get_json(&format!("/api/v1/orderBookOrders?market_id={}&limit=50", market_id)).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = res.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                let px_opt = b.get("price").and_then(|p| p.as_str());
                let sz_opt = b.get("remaining_base_amount")
                    .or_else(|| b.get("size"))
                    .or_else(|| b.get("initial_base_amount"))
                    .and_then(|s| s.as_str());

                if let (Some(px), Some(sz)) = (px_opt, sz_opt) {
                    if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                        book.bids.push(BookLevel::new(p, s));
                    }
                }
            }
        }

        if let Some(asks) = res.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                let px_opt = a.get("price").and_then(|p| p.as_str());
                let sz_opt = a.get("remaining_base_amount")
                    .or_else(|| a.get("size"))
                    .or_else(|| a.get("initial_base_amount"))
                    .and_then(|s| s.as_str());

                if let (Some(px), Some(sz)) = (px_opt, sz_opt) {
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
        let _ = symbol;
        anyhow::bail!(
            "Lighter funding-rate ingestion is not implemented; refusing to return a baseline rate"
        )
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.get_json("/api/v1/orderBookDetails").await?;
        let details = res.get("order_book_details").and_then(|d| d.as_array()).ok_or_else(|| {
            anyhow::anyhow!("Missing order_book_details in Lighter response")
        })?;

        fn as_decimal(v: &serde_json::Value) -> Option<Decimal> {
            v.as_str()
                .and_then(|s| Decimal::from_str(s).ok())
                .or_else(|| v.as_f64().and_then(|f| Decimal::try_from(f).ok()))
        }

        let mut volumes = HashMap::new();
        for item in details {
            let symbol_str = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            if symbol_str.is_empty() {
                continue;
            }
            let quote_vol = item.get("daily_quote_token_volume").and_then(as_decimal);
            let base_vol = item.get("daily_base_token_volume").and_then(as_decimal);
            let last_px = item.get("last_trade_price").and_then(as_decimal);
            let vol = quote_vol.or_else(|| match (base_vol, last_px) {
                (Some(b), Some(p)) => Some(b * p),
                _ => None,
            });
            if let Some(v) = vol {
                volumes.insert(symbol_str.to_string(), v);
            }
        }
        Ok(volumes)
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
                            leverage: dec!(5),
                            initial_margin: (sz.abs() * entry_px) / dec!(5),
                            maintenance_margin: (sz.abs() * entry_px) / dec!(10),
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

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Leverage updates are not implemented for venue {}", self.name())
    }
}
