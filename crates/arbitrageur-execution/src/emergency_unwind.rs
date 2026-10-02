use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{OrderRequest, OrderResponse};
use arbitrageur_core::types::{Decimal, OrderStatus, Side};
use std::sync::Arc;
use tracing::{error, info};

/// Emergency unwind must be fast, but async-ack venues need a bounded
/// window to reach terminal state: ~7.5s of `fetch_order` polling covers
/// normal fill propagation without stalling the unwind path.
const UNWIND_POLL_ATTEMPTS: u32 = 25;
const UNWIND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);
/// Consecutive poll failures tolerated before giving up — a venue that
/// cannot report order state at all fails closed rather than guessing.
#[cfg(not(test))]
const UNWIND_POLL_MISS_LIMIT: u32 = 8;
#[cfg(test)]
const UNWIND_POLL_MISS_LIMIT: u32 = 3;

/// Handles auto-rollback and market unwinding when an atomic multi-leg arbitrage leg fails.
pub struct EmergencyUnwindHandler;

impl EmergencyUnwindHandler {
    /// Executes emergency unwind on an unhedged filled leg:
    /// Submits an immediate market order in the opposite direction for the filled quantity.
    pub async fn unwind_leg(
        exchange: &Arc<dyn ExchangeAdapter>,
        symbol: &str,
        filled_side: Side,
        unhedged_qty: Decimal,
    ) -> anyhow::Result<OrderResponse> {
        let unwind_side = filled_side.opposite();
        error!(
            "🚨 CRITICAL: Executing EMERGENCY UNWIND on {} {}! Unwinding {} {:?} with market order",
            exchange.name(),
            symbol,
            unhedged_qty,
            unwind_side
        );

        let mut unwind_order =
            OrderRequest::new_market(exchange.venue_id(), symbol, unwind_side, unhedged_qty);
        // Spot instruments have no reduce-only concept — the venue refuses
        // an oversell itself, which is the same guarantee.
        unwind_order.reduce_only = exchange.supports_reduce_only(symbol);
        let unwind_client_id = unwind_order.client_order_id.clone();

        match exchange.place_order(unwind_order).await {
            Ok(resp) => {
                // Async-ack venues (Lighter, Decibel, PopDEX…) return `New`
                // for an accepted market order that fills milliseconds later.
                // The submit acknowledgement is never a fill — poll
                // `fetch_order` for the venue's authoritative terminal state.
                let mut cur = resp;
                let mut poll_misses = 0u32;
                for _ in 0..UNWIND_POLL_ATTEMPTS {
                    // A terminal status with an incomplete fill record is not
                    // yet trustworthy: some venues (Arcus) publish `FILLED`
                    // before enriching `avgFillPrice`/`filledSize`. Treating
                    // that snapshot as final would fail-closed on venue truth
                    // that arrives milliseconds later — keep polling while
                    // the record is incomplete.
                    let fill_record_incomplete = cur.status == OrderStatus::Filled
                        && (cur
                            .avg_fill_price
                            .map(|p| p <= Decimal::ZERO)
                            .unwrap_or(true)
                            || cur.filled_qty < unhedged_qty);
                    if (cur.status.is_final() && !fill_record_incomplete) || cur.order_id.is_empty()
                    {
                        break;
                    }
                    tokio::time::sleep(UNWIND_POLL_INTERVAL).await;
                    match exchange
                        .fetch_order(symbol, &cur.order_id, Some(&unwind_client_id))
                        .await
                    {
                        Ok(polled) => {
                            cur = polled;
                            poll_misses = 0;
                        }
                        Err(e) => {
                            poll_misses += 1;
                            error!(
                                "Emergency unwind order {} poll failed on {} {}: {e:#} (miss {poll_misses}/{UNWIND_POLL_MISS_LIMIT})",
                                cur.order_id,
                                exchange.name(),
                                symbol,
                            );
                            if poll_misses >= UNWIND_POLL_MISS_LIMIT {
                                break;
                            }
                        }
                    }
                }
                let residual_qty = if unhedged_qty > cur.filled_qty {
                    unhedged_qty - cur.filled_qty
                } else {
                    Decimal::ZERO
                };
                let avg_px = cur.avg_fill_price.unwrap_or(Decimal::ZERO);
                let residual_notional_usd = residual_qty * avg_px;
                // An unwind order satisfies the exposure if:
                // 1) cur.filled_qty == unhedged_qty (exact match), OR
                // 2) cur.filled_qty == cur.original_qty (venue filled 100% of the wire order)
                //    AND the unfilled remainder is sub-lot/wire dust (< $5.00 notional or < 1.0 unit)
                //    caused by the venue's step-size wire quantization.
                let qty_satisfied = cur.filled_qty == unhedged_qty
                    || (cur.filled_qty == cur.original_qty
                        && cur.remaining_qty == Decimal::ZERO
                        && (residual_notional_usd < rust_decimal_macros::dec!(5.0)
                            || residual_qty < rust_decimal_macros::dec!(1.0)));

                let valid = cur.status == OrderStatus::Filled
                    && qty_satisfied
                    && cur.remaining_qty == Decimal::ZERO
                    && cur.venue == exchange.venue_id()
                    && cur.symbol.as_str() == symbol
                    && cur.client_order_id == unwind_client_id
                    && !cur.order_id.is_empty()
                    && avg_px > Decimal::ZERO
                    && cur.fee.map(|f| f >= Decimal::ZERO).unwrap_or(true);
                if valid {
                    info!(
                        "Emergency unwind order filled successfully on {}: order_id={}",
                        exchange.name(),
                        cur.order_id
                    );
                    Ok(cur)
                } else {
                    error!(
                        "FATAL: EMERGENCY UNWIND INCOMPLETE ON {} {}: status={:?} filled={}/{} remaining={} venue={} symbol={} client_order_id={} order_id={} avg_px={:?} fee={:?}. MANUAL OPERATOR INTERVENTION REQUIRED!",
                        exchange.name(),
                        symbol,
                        cur.status,
                        cur.filled_qty,
                        unhedged_qty,
                        cur.remaining_qty,
                        cur.venue,
                        cur.symbol,
                        cur.client_order_id,
                        cur.order_id,
                        cur.avg_fill_price,
                        cur.fee
                    );
                    anyhow::bail!(
                        "Emergency unwind incomplete on {} {}: status={:?} filled={}/{} remaining={} venue={} symbol={} client_order_id={} order_id={} avg_px={:?} fee={:?}",
                        exchange.name(),
                        symbol,
                        cur.status,
                        cur.filled_qty,
                        unhedged_qty,
                        cur.remaining_qty,
                        cur.venue,
                        cur.symbol,
                        cur.client_order_id,
                        cur.order_id,
                        cur.avg_fill_price,
                        cur.fee
                    )
                }
            }
            Err(e) => {
                error!(
                    "FATAL: EMERGENCY UNWIND FAILED ON {} {}: {}. MANUAL OPERATOR INTERVENTION REQUIRED!",
                    exchange.name(),
                    symbol,
                    e
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct CaptureExchange {
        captured: Arc<parking_lot::Mutex<Option<OrderRequest>>>,
    }

    #[async_trait]
    impl ExchangeAdapter for CaptureExchange {
        fn venue_id(&self) -> arbitrageur_core::types::VenueId {
            "test-venue".into()
        }

        fn name(&self) -> &str {
            "test"
        }

        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            anyhow::bail!("not used")
        }

        async fn fetch_orderbook(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
            anyhow::bail!("not used")
        }

        async fn fetch_ticker(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            anyhow::bail!("not used")
        }

        async fn fetch_funding_rate(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            anyhow::bail!("not used")
        }

        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            anyhow::bail!("not used")
        }

        async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
            let response = OrderResponse {
                order_id: "unwind_1".to_string(),
                client_order_id: order.client_order_id.clone(),
                venue: order.venue.clone(),
                symbol: order.symbol.clone(),
                status: OrderStatus::Filled,
                original_qty: order.quantity,
                filled_qty: order.quantity,
                remaining_qty: Decimal::ZERO,
                avg_fill_price: Some(arbitrageur_core::types::dec!(100)),
                fee: Some(Decimal::ZERO),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            };
            *self.captured.lock() = Some(order);
            Ok(response)
        }

        async fn cancel_order(
            &self,
            _req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            anyhow::bail!("not used")
        }
    }

    /// Venue that acks `New`, then reports `FILLED` before enriching the
    /// fill record (avgFillPrice arrives a poll later — the real Arcus
    /// testnet behavior observed 2026-09).
    struct StaggeredFillExchange {
        polls: std::sync::atomic::AtomicU32,
    }

    #[async_trait]
    impl ExchangeAdapter for StaggeredFillExchange {
        fn venue_id(&self) -> arbitrageur_core::types::VenueId {
            "staggered".into()
        }

        fn name(&self) -> &str {
            "staggered"
        }

        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            anyhow::bail!("not used")
        }

        async fn fetch_orderbook(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
            anyhow::bail!("not used")
        }

        async fn fetch_ticker(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            anyhow::bail!("not used")
        }

        async fn fetch_funding_rate(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            anyhow::bail!("not used")
        }

        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            anyhow::bail!("not used")
        }

        async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
            Ok(OrderResponse {
                order_id: "ord_1".to_string(),
                client_order_id: order.client_order_id.clone(),
                venue: order.venue.clone(),
                symbol: order.symbol.clone(),
                status: OrderStatus::New,
                original_qty: order.quantity,
                filled_qty: Decimal::ZERO,
                remaining_qty: order.quantity,
                avg_fill_price: None,
                fee: None,
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            })
        }

        async fn fetch_order(
            &self,
            symbol: &str,
            order_id: &str,
            client_order_id: Option<&str>,
        ) -> anyhow::Result<OrderResponse> {
            let n = self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let enriched = n >= 1;
            Ok(OrderResponse {
                order_id: order_id.to_string(),
                client_order_id: client_order_id.unwrap_or_default().to_string(),
                venue: self.venue_id(),
                symbol: symbol.to_string(),
                status: OrderStatus::Filled,
                original_qty: arbitrageur_core::types::dec!(0.5),
                filled_qty: arbitrageur_core::types::dec!(0.5),
                remaining_qty: Decimal::ZERO,
                avg_fill_price: enriched.then(|| arbitrageur_core::types::dec!(101)),
                fee: Some(Decimal::ZERO),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            })
        }

        async fn cancel_order(
            &self,
            _req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            anyhow::bail!("not used")
        }
    }

    #[tokio::test]
    async fn test_unwind_waits_for_enriched_fill_record() {
        let exchange: Arc<dyn ExchangeAdapter> =
            Arc::new(StaggeredFillExchange { polls: 0.into() });
        let resp = EmergencyUnwindHandler::unwind_leg(
            &exchange,
            "BTC-USD",
            Side::Sell,
            arbitrageur_core::types::dec!(0.5),
        )
        .await
        .expect("unwind must succeed once the venue enriches the fill record");
        assert_eq!(resp.status, OrderStatus::Filled);
        assert_eq!(
            resp.avg_fill_price,
            Some(arbitrageur_core::types::dec!(101))
        );
    }

    #[tokio::test]
    async fn test_unwind_leg_places_reduce_only_market_order() {
        let captured: Arc<parking_lot::Mutex<Option<OrderRequest>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let exchange: Arc<dyn ExchangeAdapter> = Arc::new(CaptureExchange {
            captured: captured.clone(),
        });

        EmergencyUnwindHandler::unwind_leg(
            &exchange,
            "BTC-USDC",
            Side::Buy,
            arbitrageur_core::types::dec!(1.0),
        )
        .await
        .unwrap();

        let order = captured.lock().take().expect("place_order must be called");
        assert!(order.reduce_only, "emergency unwind must be reduce-only");
        assert_eq!(order.side, Side::Sell);
        assert_eq!(order.quantity, arbitrageur_core::types::dec!(1.0));
        assert_eq!(order.symbol, "BTC-USDC");
    }

    #[tokio::test]
    async fn test_unwind_leg_accepts_wire_quantization_dust() {
        struct QuantizedExchange;
        #[async_trait]
        impl ExchangeAdapter for QuantizedExchange {
            fn venue_id(&self) -> arbitrageur_core::types::VenueId {
                "vanta".into()
            }
            fn name(&self) -> &str {
                "Vanta.exchange"
            }
            async fn fetch_markets(
                &self,
            ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
                anyhow::bail!("not used")
            }
            async fn fetch_orderbook(
                &self,
                _symbol: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
                anyhow::bail!("not used")
            }
            async fn fetch_ticker(
                &self,
                _symbol: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
                anyhow::bail!("not used")
            }
            async fn fetch_funding_rate(
                &self,
                _symbol: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
                anyhow::bail!("not used")
            }
            async fn fetch_account(
                &self,
            ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
                anyhow::bail!("not used")
            }
            async fn cancel_order(
                &self,
                _req: arbitrageur_core::order::CancelRequest,
            ) -> anyhow::Result<()> {
                anyhow::bail!("not used")
            }
            async fn place_order(&self, req: OrderRequest) -> anyhow::Result<OrderResponse> {
                // Venue truncates 50.4 to 50.0 due to integer base lot sizing
                let wire_qty = req.quantity.floor();
                Ok(OrderResponse {
                    order_id: "unw-123".into(),
                    client_order_id: req.client_order_id,
                    venue: req.venue,
                    symbol: req.symbol,
                    status: OrderStatus::Filled,
                    original_qty: wire_qty,
                    filled_qty: wire_qty,
                    remaining_qty: Decimal::ZERO,
                    avg_fill_price: Some(arbitrageur_core::types::dec!(1.18)),
                    fee: Some(arbitrageur_core::types::dec!(0.05)),
                    fee_currency: "USDC".into(),
                    raw_response: None,
                    timestamp: chrono::Utc::now(),
                })
            }
            async fn fetch_order(
                &self,
                _symbol: &str,
                _id: &str,
                _cid: Option<&str>,
            ) -> anyhow::Result<OrderResponse> {
                anyhow::bail!("not used")
            }
        }
        let exchange: Arc<dyn ExchangeAdapter> = Arc::new(QuantizedExchange);
        let resp = EmergencyUnwindHandler::unwind_leg(
            &exchange,
            "PERP_SUI_USDC",
            Side::Buy,
            arbitrageur_core::types::dec!(50.4),
        )
        .await
        .expect(
            "unwind must succeed when venue fills 100% of wire-quantized order with sub-unit dust",
        );
        assert_eq!(resp.status, OrderStatus::Filled);
        assert_eq!(resp.filled_qty, arbitrageur_core::types::dec!(50.0));
    }
}
