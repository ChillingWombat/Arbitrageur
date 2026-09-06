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
    async fn test_rate_limiter_caching() {
        let limiter = AccountRequestCoordinator::new("test_venue", 50, 500, 100, 1000);
        let first = limiter.acquire_or_cache(true).await.expect("first call");
        assert!(first.is_none());

        limiter.record_success("{\"balance\": 1000}".to_string());

        let cached = limiter.acquire_or_cache(true).await.expect("cached call");
        assert_eq!(cached, Some("{\"balance\": 1000}".to_string()));
    }
}
