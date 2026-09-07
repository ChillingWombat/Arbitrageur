use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::time::sleep;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance, Position};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, OrderStatus, Side, VenueId};

/// In-memory microsecond simulated exchange for rigorous testing and live replay.
pub struct SimulatedExchange {
    venue_id: VenueId,
    name: String,
    latency: Duration,
    fee_bps: Decimal,
    books: DashMap<String, Arc<RwLock<OrderBook>>>,
    funding_rates: DashMap<String, FundingRate>,
    account: Arc<RwLock<AccountSnapshot>>,
}

impl SimulatedExchange {
    pub fn new(
        venue_id: impl Into<VenueId>,
        name: impl Into<String>,
        latency_ms: u64,
        fee_bps: Decimal,
        initial_equity: Decimal,
    ) -> Self {
        let vid = venue_id.into();
        let name_str = name.into();

        let account = AccountSnapshot {
            venue: vid.clone(),
            equity: initial_equity,
            free_margin: initial_equity,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![Balance {
                venue: vid.clone(),
                currency: "USDC".to_string(),
                total: initial_equity,
                free: initial_equity,
                locked: dec!(0),
            }],
            positions: Vec::new(),
            timestamp: Utc::now(),
        };

        Self {
            venue_id: vid,
            name: name_str,
            latency: Duration::from_millis(latency_ms),
            fee_bps,
            books: DashMap::new(),
            funding_rates: DashMap::new(),
            account: Arc::new(RwLock::new(account)),
        }
    }

    /// Seed a market with an orderbook and funding rate.
    pub fn seed_market(
        &self,
        symbol: &str,
        best_bid: Decimal,
        best_ask: Decimal,
        size_per_level: Decimal,
        funding_rate_8h: Decimal,
    ) {
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        for i in 0..10 {
            let offset = Decimal::from(i) * dec!(0.5);
            book.bids.push(BookLevel::new(best_bid - offset, size_per_level * (dec!(1) + Decimal::from(i) * dec!(0.2))));
            book.asks.push(BookLevel::new(best_ask + offset, size_per_level * (dec!(1) + Decimal::from(i) * dec!(0.2))));
        }
        book.timestamp = Utc::now();
        self.books.insert(symbol.to_string(), Arc::new(RwLock::new(book)));

        self.funding_rates.insert(
            symbol.to_string(),
            FundingRate {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                rate: funding_rate_8h,
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now() + chrono::Duration::hours(8),
                timestamp: Utc::now(),
            },
        );
    }

    /// Check if a symbol market has been seeded.
    pub fn has_market(&self, symbol: &str) -> bool {
        self.books.contains_key(symbol)
    }
}

#[async_trait]
impl ExchangeAdapter for SimulatedExchange {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut markets = Vec::new();
        for item in self.books.iter() {
            let sym_str = item.key();
            let base = sym_str.split('-').next().unwrap_or(sym_str);
            let symbol = Symbol::perp(base, "USDC");
            markets.push(MarketMetadata {
                symbol,
                venue: self.venue_id.clone(),
                venue_symbol: sym_str.clone(),
                price_decimals: 2,
                size_decimals: 4,
                min_order_size: dec!(0.001),
                min_notional: dec!(1.0),
                maker_fee_bps: self.fee_bps * dec!(0.5),
                taker_fee_bps: self.fee_bps,
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        if self.latency > Duration::ZERO {
            sleep(self.latency).await;
        }
        if let Some(book_lock) = self.books.get(symbol) {
            let b = book_lock.read().clone();
            Ok(b)
        } else {
            anyhow::bail!("[{}] Market not found: {}", self.name, symbol)
        }
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
            volume_24h: dec!(1000000),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        if let Some(rate) = self.funding_rates.get(symbol) {
            Ok(rate.clone())
        } else {
            Ok(FundingRate {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            })
        }
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        if self.latency > Duration::ZERO {
            sleep(self.latency).await;
        }
        let snap = self.account.read().clone();
        Ok(snap)
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if self.latency > Duration::ZERO {
            sleep(self.latency).await;
        }

        let book_lock = self.books.get(&order.symbol).ok_or_else(|| {
            anyhow::anyhow!("[{}] Market {} not found for order placement", self.name, order.symbol)
        })?;

        let (fill_px, filled_qty) = {
            let mut book = book_lock.write();
            match order.side {
                Side::Buy => {
                    let (vwap, unfilled, _) = book.buy_execution_vwap(order.quantity).ok_or_else(|| {
                        anyhow::anyhow!("[{}] Insufficient ask liquidity for buy order", self.name)
                    })?;
                    let filled = order.quantity - unfilled;
                    // Realistically consume ask levels
                    let mut to_consume = filled;
                    book.asks.retain_mut(|level| {
                        if to_consume <= dec!(0) {
                            true
                        } else if level.size <= to_consume {
                            to_consume -= level.size;
                            false
                        } else {
                            level.size -= to_consume;
                            to_consume = dec!(0);
                            true
                        }
                    });
                    (vwap, filled)
                }
                Side::Sell => {
                    let (vwap, unfilled, _) = book.sell_execution_vwap(order.quantity).ok_or_else(|| {
                        anyhow::anyhow!("[{}] Insufficient bid liquidity for sell order", self.name)
                    })?;
                    let filled = order.quantity - unfilled;
                    // Realistically consume bid levels
                    let mut to_consume = filled;
                    book.bids.retain_mut(|level| {
                        if to_consume <= dec!(0) {
                            true
                        } else if level.size <= to_consume {
                            to_consume -= level.size;
                            false
                        } else {
                            level.size -= to_consume;
                            to_consume = dec!(0);
                            true
                        }
                    });
                    (vwap, filled)
                }
            }
        };

        let notional = filled_qty * fill_px;
        let fee = notional * (self.fee_bps / dec!(10000));

        // Update position and equity with realized PnL in simulated account
        {
            let mut acc = self.account.write();
            let mut realized_pnl = dec!(0);

            let pos_opt = acc.positions.iter_mut().find(|p| p.symbol == order.symbol);
            if let Some(pos) = pos_opt {
                if pos.side == order.side {
                    let total_qty = pos.quantity + filled_qty;
                    let total_cost = (pos.quantity * pos.entry_price) + notional;
                    pos.entry_price = total_cost / total_qty;
                    pos.quantity = total_qty;
                    pos.mark_price = fill_px;
                    pos.initial_margin = (total_qty * fill_px) / pos.leverage;
                    pos.maintenance_margin = (total_qty * fill_px) / (pos.leverage * dec!(2));
                } else {
                    let closing_qty = pos.quantity.min(filled_qty);
                    let pnl_per_unit = if pos.side == Side::Buy {
                        fill_px - pos.entry_price
                    } else {
                        pos.entry_price - fill_px
                    };
                    realized_pnl += pnl_per_unit * closing_qty;

                    if pos.quantity > filled_qty {
                        pos.quantity -= filled_qty;
                        pos.mark_price = fill_px;
                        pos.realized_pnl += realized_pnl;
                        pos.initial_margin = (pos.quantity * fill_px) / pos.leverage;
                        pos.maintenance_margin = (pos.quantity * fill_px) / (pos.leverage * dec!(2));
                    } else {
                        let remaining_new_qty = filled_qty - pos.quantity;
                        pos.side = order.side;
                        pos.quantity = remaining_new_qty;
                        pos.entry_price = fill_px;
                        pos.mark_price = fill_px;
                        pos.realized_pnl += realized_pnl;
                        pos.initial_margin = (remaining_new_qty * fill_px) / pos.leverage;
                        pos.maintenance_margin = (remaining_new_qty * fill_px) / (pos.leverage * dec!(2));
                    }
                }
            } else {
                acc.positions.push(Position {
                    venue: self.venue_id.clone(),
                    symbol: order.symbol.clone(),
                    side: order.side,
                    quantity: filled_qty,
                    entry_price: fill_px,
                    mark_price: fill_px,
                    unrealized_pnl: dec!(0),
                    realized_pnl: dec!(0),
                    liquidation_price: None,
                    leverage: dec!(10),
                    initial_margin: notional / dec!(10),
                    maintenance_margin: notional / dec!(20),
                    updated_at: Utc::now(),
                });
            }

            // Remove fully closed positions
            acc.positions.retain(|p| p.quantity > dec!(0));

            acc.equity += realized_pnl - fee;
            let total_mm: Decimal = acc.positions.iter().map(|p| p.maintenance_margin).sum();
            acc.maintenance_margin = total_mm;
            acc.free_margin = (acc.equity - total_mm).max(dec!(0));
            acc.margin_usage_pct = if acc.equity > dec!(0) { (total_mm / acc.equity) * dec!(100) } else { dec!(0) };

            let eq = acc.equity;
            let free = acc.free_margin;
            if let Some(bal) = acc.balances.first_mut() {
                bal.total = eq;
                bal.free = free;
                bal.locked = (eq - free).max(dec!(0));
            }
        }

        Ok(OrderResponse {
            order_id: format!("sim-{}", uuid::Uuid::new_v4()),
            client_order_id: order.client_order_id,
            venue: self.venue_id.clone(),
            symbol: order.symbol,
            status: OrderStatus::Filled,
            original_qty: order.quantity,
            filled_qty,
            remaining_qty: order.quantity - filled_qty,
            avg_fill_price: Some(fill_px),
            fee,
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        Ok(())
    }
}
