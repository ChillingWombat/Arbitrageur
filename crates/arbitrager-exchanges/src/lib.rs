pub mod rate_limiter;
pub mod hyperliquid;
pub mod lighter;
pub mod vanta;
pub mod robinhood;
pub mod prediction;
pub mod simulated;

pub use rate_limiter::*;
pub use hyperliquid::*;
pub use lighter::*;
pub use vanta::*;
pub use robinhood::*;
pub use prediction::*;
pub use simulated::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrager_core::exchange::ExchangeAdapter;
    use arbitrager_core::order::OrderRequest;
    use arbitrager_core::types::{dec, Side};

    #[tokio::test]
    async fn test_simulated_exchange_trading() {
        let exchange = SimulatedExchange::new("sim_a", "Simulated Exchange A", 0, dec!(2.0), dec!(10000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0001));

        let book = exchange.fetch_orderbook("BTC-USDC").await.expect("Fetch book");
        assert_eq!(book.best_bid_price(), Some(dec!(60000.0)));
        assert_eq!(book.best_ask_price(), Some(dec!(60010.0)));

        let order = OrderRequest::new_limit(
            exchange.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60010.0),
            dec!(0.5),
        );
        let resp = exchange.place_order(order).await.expect("Place order");
        assert_eq!(resp.filled_qty, dec!(0.5));
        assert_eq!(resp.avg_fill_price, Some(dec!(60010.0)));

        let account = exchange.fetch_account().await.expect("Fetch account");
        assert_eq!(account.positions.len(), 1);
        assert_eq!(account.positions[0].quantity, dec!(0.5));
    }

    #[tokio::test]
    async fn test_simulated_exchange_depth_consumption() {
        let exchange = SimulatedExchange::new("sim_b", "Simulated Exchange B", 0, dec!(0.0), dec!(10000.0));
        // Seed market with 2.0 BTC ask at level 0
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        let initial_book = exchange.fetch_orderbook("BTC-USDC").await.expect("initial book");
        assert_eq!(initial_book.asks[0].size, dec!(2.0));

        // Buy 1.5 BTC: should consume 1.5 BTC leaving 0.5 BTC at best ask
        let order1 = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(1.5));
        exchange.place_order(order1).await.expect("fill 1");

        let book_after = exchange.fetch_orderbook("BTC-USDC").await.expect("book after");
        assert_eq!(book_after.asks[0].price, dec!(60010.0));
        assert_eq!(book_after.asks[0].size, dec!(0.5));

        // Buy another 0.5 BTC: level 0 should be fully consumed, moving best ask to next level
        let order2 = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60020.0), dec!(0.5));
        exchange.place_order(order2).await.expect("fill 2");

        let book_after2 = exchange.fetch_orderbook("BTC-USDC").await.expect("book after 2");
        assert_ne!(book_after2.asks[0].price, dec!(60010.0));
    }

    #[tokio::test]
    async fn test_simulated_exchange_realized_pnl_and_margin() {
        let exchange = SimulatedExchange::new("sim_c", "Simulated Exchange C", 0, dec!(0.0), dec!(10000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(5.0), dec!(0.0));

        // Open Long 1.0 BTC @ 60010
        let buy_order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(1.0));
        exchange.place_order(buy_order).await.expect("buy");

        let acc_open = exchange.fetch_account().await.expect("acc open");
        assert_eq!(acc_open.positions.len(), 1);
        assert_eq!(acc_open.positions[0].quantity, dec!(1.0));

        // Price moves up: seed with higher bids
        exchange.seed_market("BTC-USDC", dec!(60500.0), dec!(60510.0), dec!(5.0), dec!(0.0));

        // Close Long by Selling 1.0 BTC @ 60500 (+490 profit)
        let sell_order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Sell, dec!(60500.0), dec!(1.0));
        exchange.place_order(sell_order).await.expect("sell");

        let acc_closed = exchange.fetch_account().await.expect("acc closed");
        // Position should be removed completely
        assert_eq!(acc_closed.positions.len(), 0);
        // Equity should have increased by realized profit (60500 - 60010 = +490)
        assert_eq!(acc_closed.equity, dec!(10000.0) + dec!(490.0));
        assert_eq!(acc_closed.maintenance_margin, dec!(0));
    }

    #[test]
    fn test_lighter_order_parsing_remaining_base_amount() {
        let sample_json = serde_json::json!({
            "code": 200,
            "bids": [{
                "order_id": "123",
                "price": "79569.8",
                "remaining_base_amount": "0.00007",
                "initial_base_amount": "0.00020"
            }],
            "asks": [{
                "order_id": "456",
                "price": "79570.2",
                "remaining_base_amount": "0.00126",
                "initial_base_amount": "0.00126"
            }]
        });

        let mut book = arbitrager_core::quote::OrderBook::new("lighter".into(), "BTC-USDC".into());
        if let Some(bids) = sample_json.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                let sz = b.get("remaining_base_amount").and_then(|s| s.as_str()).unwrap();
                let px = b.get("price").and_then(|p| p.as_str()).unwrap();
                book.bids.push(arbitrager_core::quote::BookLevel::new(px.parse().unwrap(), sz.parse().unwrap()));
            }
        }
        if let Some(asks) = sample_json.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                let sz = a.get("remaining_base_amount").and_then(|s| s.as_str()).unwrap();
                let px = a.get("price").and_then(|p| p.as_str()).unwrap();
                book.asks.push(arbitrager_core::quote::BookLevel::new(px.parse().unwrap(), sz.parse().unwrap()));
            }
        }

        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.bids[0].price, dec!(79569.8));
        assert_eq!(book.bids[0].size, dec!(0.00007));
        assert_eq!(book.asks[0].price, dec!(79570.2));
        assert_eq!(book.asks[0].size, dec!(0.00126));
    }

    #[tokio::test]
    async fn test_rate_limiter_caching() {
        let limiter = AccountRequestCoordinator::new("test_venue", 50, 500, 100, 1000);
        let first = limiter.acquire_or_cache(true).await.expect("first call");
        assert!(first.is_none());

        limiter.record_success("{\"balance\": 1000}".to_string());

        let cached = limiter.acquire_or_cache(true).await.expect("cached call");
        assert_eq!(cached, Some("{\"balance\": 1000}".to_string()));
    }
}
