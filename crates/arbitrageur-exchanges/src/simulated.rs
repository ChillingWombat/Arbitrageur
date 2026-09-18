use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::time::sleep;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side, VenueId};

/// In-memory microsecond simulated exchange for rigorous testing and live replay.
pub struct SimulatedExchange {
    venue_id: VenueId,
    name: String,
    latency: Duration,
    fee_bps: Decimal,
    books: DashMap<String, Arc<RwLock<OrderBook>>>,
    funding_rates: DashMap<String, FundingRate>,
    leverages: DashMap<String, u32>,
    account: Arc<RwLock<AccountSnapshot>>,
    /// Seed equity the paper account started with — the baseline realized
    /// P&L is measured against. Updated by `reset_account` when the
    /// configured starting balance changes.
    initial_equity: RwLock<Decimal>,
    /// Per-account leverage ceiling (1..=125, `None` = unlimited). New
    /// positions are booked at min(symbol leverage, cap) and `set_leverage`
    /// above the cap is refused, so order-time min(strategy, account)
    /// holds even if a caller passes the raw strategy target.
    max_leverage_cap: RwLock<Option<u32>>,
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
            leverages: DashMap::new(),
            account: Arc::new(RwLock::new(account)),
            initial_equity: RwLock::new(initial_equity),
            max_leverage_cap: RwLock::new(None),
        }
    }

    /// The seed equity this paper account was started with — the baseline
    /// for realized P&L, not the current equity.
    pub fn initial_equity(&self) -> Decimal {
        *self.initial_equity.read()
    }

    /// Current per-account leverage ceiling (`None` = unlimited).
    pub fn max_leverage_cap(&self) -> Option<u32> {
        *self.max_leverage_cap.read()
    }

    /// Set the per-account leverage ceiling. `None` clears it (unlimited);
    /// `Some(v)` must be within 1..=125. The cap survives `reset_account`:
    /// resets move equity, not account policy.
    pub fn set_max_leverage_cap(&self, cap: Option<u32>) -> anyhow::Result<()> {
        if let Some(v) = cap {
            if !(1..=125).contains(&v) {
                anyhow::bail!(
                    "[{}] max leverage cap {} out of range; must be 1..=125 or unset",
                    self.name,
                    v
                );
            }
        }
        *self.max_leverage_cap.write() = cap;
        Ok(())
    }

    /// Effective booking leverage for a symbol: min(configured, cap).
    /// Strategy default is 5x; no venue term and no 5x ceiling.
    fn effective_leverage_for(&self, symbol: &str) -> u32 {
        let configured = self.leverages.get(symbol).map(|v| *v).unwrap_or(5).clamp(1, 125);
        match self.max_leverage_cap() {
            Some(cap) => configured.min(cap.clamp(1, 125)),
            None => configured,
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

    /// Recompute account-level margin aggregates from open positions.
    fn recompute_account_margins(acc: &mut AccountSnapshot) {
        let total_im: Decimal = acc.positions.iter().map(|p| p.initial_margin).sum();
        let total_mm: Decimal = acc.positions.iter().map(|p| p.maintenance_margin).sum();
        acc.maintenance_margin = total_mm;
        acc.free_margin = (acc.equity - total_im).max(dec!(0));
        acc.margin_usage_pct = if acc.equity > dec!(0) {
            (total_im / acc.equity) * dec!(100)
        } else {
            dec!(0)
        };
        let eq = acc.equity;
        let free = acc.free_margin;
        if let Some(bal) = acc.balances.first_mut() {
            bal.total = eq;
            bal.free = free;
            bal.locked = (eq - free).max(dec!(0));
        }
    }

    /// Check if a symbol market has been seeded.
    pub fn has_market(&self, symbol: &str) -> bool {
        self.books.contains_key(symbol)
    }

    /// Reinitializes the paper account to `starting_equity` with zero open
    /// positions. Used when the user changes the configured paper starting
    /// balance; a new starting balance means a fresh paper account, so open
    /// simulated positions are cleared rather than fabricating a partial
    /// reset.
    pub fn reset_account(&self, starting_equity: Decimal) {
        *self.initial_equity.write() = starting_equity;
        let mut acc = self.account.write();
        acc.equity = starting_equity;
        acc.positions.clear();
        acc.timestamp = Utc::now();
        Self::recompute_account_margins(&mut acc);
    }

    /// Replace a symbol's book with an externally sourced, execution-validated book.
    pub fn replace_orderbook(&self, book: OrderBook) -> anyhow::Result<()> {
        if book.venue != self.venue_id {
            anyhow::bail!(
                "[{}] Refusing order book attributed to venue '{}'",
                self.name,
                book.venue
            );
        }
        book.validate_for_execution().map_err(anyhow::Error::msg)?;
        self.books.insert(book.symbol.clone(), Arc::new(RwLock::new(book)));
        Ok(())
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
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        if let Some(rate) = self.funding_rates.get(symbol) {
            Ok(rate.clone())
        } else {
            anyhow::bail!("[{}] No seeded funding rate for {}", self.name, symbol)
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

        if order.quantity <= dec!(0) {
            anyhow::bail!("[{}] Order quantity must be positive", self.name);
        }
        if let Some(limit) = order.price {
            if limit <= dec!(0) {
                anyhow::bail!("[{}] Order limit price must be positive", self.name);
            }
        }

        let book_lock = self
            .books
            .get(&order.symbol)
            .map(|r| Arc::clone(r.value()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[{}] Market {} not found for order placement",
                    self.name,
                    order.symbol
                )
            })?;

        // Walk only price levels acceptable to the order: a limit buy consumes
        // asks <= limit, a limit sell consumes bids >= limit, market orders
        // consume everything available. Levels are sorted, so the first
        // unacceptable price ends the walk.
        let acceptable = |price: Decimal| match (order.side, order.price) {
            (Side::Buy, Some(limit)) => price <= limit,
            (Side::Sell, Some(limit)) => price >= limit,
            _ => true,
        };

        let mut book = book_lock.write();
        let mut acc = self.account.write();

        let execution_qty = if order.reduce_only {
            match acc.positions.iter().find(|p| p.symbol == order.symbol) {
                Some(pos) if pos.side != order.side => order.quantity.min(pos.quantity),
                _ => {
                    return Ok(OrderResponse {
                        order_id: format!("sim-{}", uuid::Uuid::new_v4()),
                        client_order_id: order.client_order_id,
                        venue: self.venue_id.clone(),
                        symbol: order.symbol,
                        status: OrderStatus::Rejected,
                        original_qty: order.quantity,
                        filled_qty: dec!(0),
                        remaining_qty: order.quantity,
                        avg_fill_price: None,
                        fee: dec!(0),
                        fee_currency: "USDC".to_string(),
                        raw_response: None,
                        timestamp: Utc::now(),
                    });
                }
            }
        } else {
            order.quantity
        };

        let levels = match order.side {
            Side::Buy => &book.asks,
            Side::Sell => &book.bids,
        };

        let mut filled_qty = dec!(0);
        let mut cost = dec!(0);
        for level in levels.iter() {
            if !acceptable(level.price) {
                break;
            }
            let take = (execution_qty - filled_qty).min(level.size);
            filled_qty += take;
            cost += take * level.price;
            if filled_qty >= execution_qty {
                break;
            }
        }
        let fill_px_opt = if filled_qty > dec!(0) {
            Some(cost / filled_qty)
        } else {
            None
        };

        let remaining_qty = order.quantity - filled_qty;
        if filled_qty <= dec!(0) {
            return Ok(OrderResponse {
                order_id: format!("sim-{}", uuid::Uuid::new_v4()),
                client_order_id: order.client_order_id,
                venue: self.venue_id.clone(),
                symbol: order.symbol,
                status: OrderStatus::Rejected,
                original_qty: order.quantity,
                filled_qty,
                remaining_qty,
                avg_fill_price: None,
                fee: dec!(0),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: Utc::now(),
            });
        }

        let fill_px = fill_px_opt.unwrap_or(dec!(0));
        let notional = filled_qty * fill_px;
        let fee = notional * (self.fee_bps / dec!(10000));
        let leverage_dec = Decimal::from(self.effective_leverage_for(&order.symbol));

        // Opening additional exposure requires incremental initial margin;
        // reducing or flipping a position only needs margin on the new side's
        // residual quantity. Reject before consuming any book liquidity.
        let existing_qty = acc
            .positions
            .iter()
            .find(|p| p.symbol == order.symbol && p.side != order.side)
            .map(|p| p.quantity);
        let opening_qty = match existing_qty {
            Some(pos_qty) => (filled_qty - pos_qty).max(dec!(0)),
            None => filled_qty,
        };
        let required_im = opening_qty * fill_px / leverage_dec;
        if required_im > acc.free_margin {
            anyhow::bail!(
                "[{}] Insufficient free margin: required {} > available {}",
                self.name,
                required_im,
                acc.free_margin
            );
        }

        let levels = match order.side {
            Side::Buy => &mut book.asks,
            Side::Sell => &mut book.bids,
        };
        let mut to_consume = filled_qty;
        let mut consumed = dec!(0);
        levels.retain_mut(|level| {
            if to_consume <= dec!(0) || !acceptable(level.price) {
                return true;
            }
            let take = to_consume.min(level.size);
            to_consume -= take;
            consumed += take;
            level.size -= take;
            level.size > dec!(0)
        });
        if consumed != filled_qty {
            anyhow::bail!(
                "[{}] Internal fill accounting mismatch: consumed {} of previewed {}",
                self.name,
                consumed,
                filled_qty
            );
        }

        // Update position and equity with realized PnL in simulated account
        {
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
                        pos.leverage = leverage_dec;
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
                    leverage: leverage_dec,
                    initial_margin: notional / leverage_dec,
                    maintenance_margin: notional / (leverage_dec * dec!(2)),
                    updated_at: Utc::now(),
                });
            }

            // Remove fully closed positions
            acc.positions.retain(|p| p.quantity > dec!(0));

            acc.equity += realized_pnl - fee;
            Self::recompute_account_margins(&mut acc);
        }

        let status = if filled_qty == order.quantity && remaining_qty == dec!(0) {
            OrderStatus::Filled
        } else if filled_qty > dec!(0) {
            OrderStatus::PartiallyFilled
        } else {
            OrderStatus::Rejected
        };

        Ok(OrderResponse {
            order_id: format!("sim-{}", uuid::Uuid::new_v4()),
            client_order_id: order.client_order_id,
            venue: self.venue_id.clone(),
            symbol: order.symbol,
            status,
            original_qty: order.quantity,
            filled_qty,
            remaining_qty,
            avg_fill_price: fill_px_opt,
            fee,
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        Ok(())
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        if symbol.trim().is_empty() {
            anyhow::bail!("[{}] Symbol must be nonempty for leverage update", self.name);
        }
        if !(1..=125).contains(&leverage) {
            anyhow::bail!("[{}] Leverage {} out of range; must be 1..=125", self.name, leverage);
        }
        if let Some(cap) = self.max_leverage_cap() {
            if leverage > cap {
                anyhow::bail!(
                    "[{}] Leverage {} exceeds this account's max leverage cap of {}",
                    self.name,
                    leverage,
                    cap
                );
            }
        }
        self.leverages.insert(symbol.to_string(), leverage);
        let mut acc = self.account.write();
        let leverage_dec = Decimal::from(leverage);
        for pos in &mut acc.positions {
            if pos.symbol == symbol {
                pos.leverage = leverage_dec;
                pos.initial_margin = (pos.quantity * pos.mark_price) / leverage_dec;
                pos.maintenance_margin = (pos.quantity * pos.mark_price) / (leverage_dec * dec!(2));
            }
        }
        Self::recompute_account_margins(&mut acc);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> SimulatedExchange {
        let ex = SimulatedExchange::new("sim", "Simulated", 0, dec!(0.0), dec!(100000.0));
        ex.seed_market("BTC-USDC", dec!(100.0), dec!(101.0), dec!(5.0), dec!(0.0));
        ex
    }

    fn market_order(
        ex: &SimulatedExchange,
        side: Side,
        qty: Decimal,
        reduce_only: bool,
    ) -> OrderRequest {
        let mut order = OrderRequest::new_market(ex.venue_id(), "BTC-USDC", side, qty);
        order.reduce_only = reduce_only;
        order
    }

    #[tokio::test]
    async fn reduce_only_order_while_flat_is_rejected() {
        let ex = seeded();
        let resp = ex
            .place_order(market_order(&ex, Side::Sell, dec!(1.0), true))
            .await
            .expect("reduce-only produces a rejection response");
        assert_eq!(resp.status, OrderStatus::Rejected);
        assert_eq!(resp.filled_qty, dec!(0));
        assert_eq!(resp.remaining_qty, dec!(1.0));

        let acc = ex.fetch_account().await.expect("account");
        assert!(
            acc.positions.is_empty(),
            "reduce-only must never open a position"
        );
        let book = ex.fetch_orderbook("BTC-USDC").await.expect("book");
        assert_eq!(book.bids.len(), 10);
        assert_eq!(
            book.bids[0].size,
            dec!(5.0),
            "rejection must not consume depth"
        );
    }

    #[tokio::test]
    async fn reduce_only_same_side_is_rejected() {
        let ex = seeded();
        ex.place_order(market_order(&ex, Side::Buy, dec!(1.0), false))
            .await
            .expect("opening order");

        let resp = ex
            .place_order(market_order(&ex, Side::Buy, dec!(0.5), true))
            .await
            .expect("reduce-only produces a rejection response");
        assert_eq!(resp.status, OrderStatus::Rejected);
        assert_eq!(resp.filled_qty, dec!(0));
        assert_eq!(resp.remaining_qty, dec!(0.5));

        let acc = ex.fetch_account().await.expect("account");
        let pos = acc.get_position("BTC-USDC").expect("position");
        assert_eq!(pos.side, Side::Buy);
        assert_eq!(
            pos.quantity,
            dec!(1.0),
            "same-side reduce-only must not add"
        );
    }

    #[tokio::test]
    async fn reduce_only_oversized_close_caps_at_position() {        let ex = seeded();
        ex.place_order(market_order(&ex, Side::Buy, dec!(0.5), false))
            .await
            .expect("opening order");

        let resp = ex
            .place_order(market_order(&ex, Side::Sell, dec!(1.0), true))
            .await
            .expect("close order");
        assert_eq!(resp.status, OrderStatus::PartiallyFilled);
        assert_eq!(resp.original_qty, dec!(1.0));
        assert_eq!(
            resp.filled_qty,
            dec!(0.5),
            "fill is capped at the open position"
        );
        assert_eq!(resp.remaining_qty, dec!(0.5));

        let acc = ex.fetch_account().await.expect("account");
        assert!(
            acc.positions.is_empty(),
            "reduce-only must not leave a residual or reverse position: {:?}",
            acc.positions
        );
    }

    #[test]
    fn max_leverage_cap_validation_bounds() {
        let ex = seeded();
        assert_eq!(ex.max_leverage_cap(), None);
        assert!(ex.set_max_leverage_cap(Some(0)).is_err());
        assert!(ex.set_max_leverage_cap(Some(126)).is_err());
        ex.set_max_leverage_cap(Some(6)).expect("above old 5x ceiling ok");
        ex.set_max_leverage_cap(Some(50)).expect("venue-scale cap ok");
        ex.set_max_leverage_cap(Some(3)).expect("valid cap");
        assert_eq!(ex.max_leverage_cap(), Some(3));
        ex.set_max_leverage_cap(None).expect("clear");
        assert_eq!(ex.max_leverage_cap(), None);
    }

    #[tokio::test]
    async fn set_leverage_above_cap_is_refused() {
        let ex = seeded();
        ex.set_max_leverage_cap(Some(2)).expect("cap");
        assert!(ex.set_leverage("BTC-USDC", 3).await.is_err());
        assert!(ex.set_leverage("BTC-USDC", 5).await.is_err());
        ex.set_leverage("BTC-USDC", 2).await.expect("at cap ok");
        ex.set_leverage("BTC-USDC", 1).await.expect("below cap ok");
    }

    #[tokio::test]
    async fn sim_margin_books_at_capped_leverage() {
        let ex = seeded();
        ex.set_max_leverage_cap(Some(2)).expect("cap");
        ex.place_order(market_order(&ex, Side::Buy, dec!(1.0), false))
            .await
            .expect("opening order");
        let acc = ex.fetch_account().await.expect("account");
        let pos = acc.get_position("BTC-USDC").expect("position");
        assert_eq!(pos.leverage, dec!(2), "booking must use min(configured, cap)");
        let px = pos.entry_price;
        assert_eq!(
            pos.initial_margin,
            (dec!(1.0) * px) / dec!(2),
            "initial margin must reflect the capped leverage"
        );
        let total_im: Decimal = acc.positions.iter().map(|p| p.initial_margin).sum();
        assert_eq!(acc.free_margin, (acc.equity - total_im).max(dec!(0)));
    }

    #[tokio::test]
    async fn fills_only_equity_with_exact_fee_accounting() {
        let ex = SimulatedExchange::new("sim", "Simulated", 0, dec!(10.0), dec!(100000.0));
        ex.seed_market("BTC-USDC", dec!(100.0), dec!(101.0), dec!(5.0), dec!(0.0));
        let start = ex.fetch_account().await.expect("account").equity;
        assert_eq!(start, dec!(100000.0));

        let open = ex
            .place_order(market_order(&ex, Side::Buy, dec!(1.0), false))
            .await
            .expect("open");
        assert_eq!(open.status, OrderStatus::Filled);
        let after_open = ex.fetch_account().await.expect("account").equity;
        let open_fee = dec!(1.0) * dec!(101.0) * dec!(10.0) / dec!(10000);
        assert_eq!(
            after_open,
            start - open_fee,
            "opening only pays the fee; no PnL while flat-to-long"
        );

        let close = ex
            .place_order(market_order(&ex, Side::Sell, dec!(1.0), false))
            .await
            .expect("close");
        assert_eq!(close.status, OrderStatus::Filled);
        let after_close = ex.fetch_account().await.expect("account").equity;
        let close_fee = dec!(1.0) * dec!(100.0) * dec!(10.0) / dec!(10000);
        let realized = (dec!(100.0) - dec!(101.0)) * dec!(1.0);
        assert_eq!(
            after_close,
            start - open_fee - close_fee + realized,
            "equity moves only by fills (realized PnL) minus fees"
        );
        assert!(ex.fetch_account().await.expect("account").positions.is_empty());

        ex.reset_account(dec!(250000.0));
        let reset = ex.fetch_account().await.expect("account");
        assert_eq!(reset.equity, dec!(250000.0));
        assert_eq!(ex.initial_equity(), dec!(250000.0));
        assert!(reset.positions.is_empty());
    }
}
