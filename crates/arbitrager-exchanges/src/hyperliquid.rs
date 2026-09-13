use std::collections::HashMap;
use std::str::FromStr;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance, Position};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, Side, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Hyperliquid L1 Perpetual and Spot exchange adapter.
#[allow(dead_code)]
pub struct HyperliquidAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    wallet_address: Option<String>,
    private_key: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

impl HyperliquidAdapter {
    pub fn new(
        base_url: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.hyperliquid.xyz".to_string());
        Self {
            venue_id: VenueId::new("hyperliquid"),
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            wallet_address,
            private_key,
            rate_limiter: AccountRequestCoordinator::new("hyperliquid", 100, 2000, 500, 10000),
        }
    }

    async fn post_info(&self, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/info", self.base_url);
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Hyperliquid /info error status: {}", resp.status());
        }
        let json_val = resp.json::<serde_json::Value>().await?;
        Ok(json_val)
    }
}

#[async_trait]
impl ExchangeAdapter for HyperliquidAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Hyperliquid"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.post_info(json!({"type": "meta"})).await?;
        let universe = res.get("universe").and_then(|u| u.as_array()).ok_or_else(|| {
            anyhow::anyhow!("Missing universe array in Hyperliquid meta response")
        })?;

        let mut markets = Vec::new();
        for item in universe {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let sz_decimals = item.get("szDecimals").and_then(|s| s.as_u64()).unwrap_or(3) as u32;
                let symbol = Symbol::perp(name, "USDC");
                markets.push(MarketMetadata {
                    symbol,
                    venue: self.venue_id.clone(),
                    venue_symbol: name.to_string(),
                    price_decimals: 5,
                    size_decimals: sz_decimals,
                    min_order_size: dec!(0.001),
                    min_notional: dec!(10.0),
                    maker_fee_bps: dec!(-0.2), // Hyperliquid maker rebate or low fee
                    taker_fee_bps: dec!(2.5),  // 2.5 bps taker fee
                    contract_size: dec!(1.0),
                    is_active: true,
                });
            }
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        // Strip suffixes if present, Hyperliquid expects bare coin e.g. "BTC" or "ETH"
        let coin = symbol.split('-').next().unwrap_or(symbol);
        let res = self.post_info(json!({"type": "l2Book", "coin": coin})).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(levels) = res.get("levels").and_then(|l| l.as_array()) {
            if levels.len() >= 2 {
                // levels[0] = bids, levels[1] = asks
                if let Some(bids) = levels[0].as_array() {
                    for b in bids {
                        if let (Some(px), Some(sz)) = (
                            b.get("px").and_then(|p| p.as_str()),
                            b.get("sz").and_then(|s| s.as_str()),
                        ) {
                            if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                                book.bids.push(BookLevel::new(p, s));
                            }
                        }
                    }
                }

                if let Some(asks) = levels[1].as_array() {
                    for a in asks {
                        if let (Some(px), Some(sz)) = (
                            a.get("px").and_then(|p| p.as_str()),
                            a.get("sz").and_then(|s| s.as_str()),
                        ) {
                            if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                                book.asks.push(BookLevel::new(p, s));
                            }
                        }
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
            volume_24h: dec!(10000000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let coin = symbol.split('-').next().unwrap_or(symbol);
        let res = self.post_info(json!({"type": "metaAndAssetCtxs"})).await?;

        // Format is [meta, [assetCtx, ...]]
        if let Some(arr) = res.as_array() {
            if arr.len() >= 2 {
                let universe = arr[0].get("universe").and_then(|u| u.as_array());
                let contexts = arr[1].as_array();

                if let (Some(uni), Some(ctxs)) = (universe, contexts) {
                    for (i, item) in uni.iter().enumerate() {
                        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                            if name.eq_ignore_ascii_case(coin) {
                                if let Some(ctx) = ctxs.get(i) {
                                    let rate_str = ctx.get("funding").and_then(|f| f.as_str()).unwrap_or("0");
                                    let rate = Decimal::from_str(rate_str).unwrap_or(dec!(0));
                                    return Ok(FundingRate {
                                        venue: self.venue_id.clone(),
                                        symbol: symbol.to_string(),
                                        rate,
                                        predicted_rate: None,
                                        interval_hours: 1, // Hyperliquid hourly continuous funding
                                        next_funding_time: Utc::now(),
                                        timestamp: Utc::now(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        anyhow::bail!("Funding rate not found for coin: {}", coin)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.post_info(json!({"type": "metaAndAssetCtxs"})).await?;

        let arr = res.as_array().ok_or_else(|| anyhow::anyhow!("Unexpected metaAndAssetCtxs response"))?;
        if arr.len() < 2 {
            anyhow::bail!("metaAndAssetCtxs response missing universe/contexts");
        }
        let universe = arr[0].get("universe").and_then(|u| u.as_array())
            .ok_or_else(|| anyhow::anyhow!("metaAndAssetCtxs missing universe"))?;
        let contexts = arr[1].as_array()
            .ok_or_else(|| anyhow::anyhow!("metaAndAssetCtxs missing asset contexts"))?;

        let mut volumes = HashMap::new();
        for (i, item) in universe.iter().enumerate() {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if let Some(ctx) = contexts.get(i) {
                if let Some(vol_str) = ctx.get("dayNtlVlm").and_then(|v| v.as_str()) {
                    if let Ok(vol) = Decimal::from_str(vol_str) {
                        volumes.insert(name.to_string(), vol);
                    }
                }
            }
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let user = self.wallet_address.as_deref().unwrap_or("0x0000000000000000000000000000000000000000");
        let res = self.post_info(json!({"type": "clearinghouseState", "user": user})).await?;

        let margin_summary = res.get("marginSummary");
        let account_value = margin_summary
            .and_then(|m| m.get("accountValue"))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let total_margin_used = margin_summary
            .and_then(|m| m.get("totalMarginUsed"))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let free_margin = (account_value - total_margin_used).max(dec!(0));

        let mut positions = Vec::new();
        if let Some(pos_arr) = res.get("assetPositions").and_then(|p| p.as_array()) {
            for p in pos_arr {
                if let Some(pos_obj) = p.get("position") {
                    let coin = pos_obj.get("coin").and_then(|c| c.as_str()).unwrap_or("");
                    let szi_str = pos_obj.get("szi").and_then(|s| s.as_str()).unwrap_or("0");
                    let entry_str = pos_obj.get("entryPx").and_then(|e| e.as_str()).unwrap_or("0");

                    let szi = Decimal::from_str(szi_str).unwrap_or(dec!(0));
                    let entry_px = Decimal::from_str(entry_str).unwrap_or(dec!(0));

                    if szi != dec!(0) {
                        let side = if szi > dec!(0) { Side::Buy } else { Side::Sell };
                        let qty = szi.abs();
                        positions.push(Position {
                            venue: self.venue_id.clone(),
                            symbol: format!("{}-USDC", coin),
                            side,
                            quantity: qty,
                            entry_price: entry_px,
                            mark_price: entry_px,
                            unrealized_pnl: dec!(0),
                            realized_pnl: dec!(0),
                            liquidation_price: None,
                            leverage: dec!(5),
                            initial_margin: (qty * entry_px) / dec!(5),
                            maintenance_margin: (qty * entry_px) / dec!(10),
                            updated_at: Utc::now(),
                        });
                    }
                }
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: account_value,
            free_margin,
            maintenance_margin: total_margin_used,
            margin_usage_pct: if account_value > dec!(0) { (total_margin_used / account_value) * dec!(100) } else { dec!(0) },
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: account_value,
                free: free_margin,
                locked: total_margin_used,
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
