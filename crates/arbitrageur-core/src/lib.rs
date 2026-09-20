pub mod exchange;
pub mod hub;
pub mod market;
pub mod order;
pub mod position;
pub mod quote;
pub mod types;

pub use exchange::*;
pub use hub::*;
pub use market::*;
pub use order::*;
pub use position::*;
pub use quote::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use types::dec;

    #[test]
    fn test_orderbook_vwap_and_slippage() {
        let mut book = OrderBook::new("venue_x".into(), "BTC-USDC".into());
        book.asks.push(BookLevel::new(dec!(100.0), dec!(1.0)));
        book.asks.push(BookLevel::new(dec!(110.0), dec!(2.0)));
        book.bids.push(BookLevel::new(dec!(99.0), dec!(1.0)));
        book.bids.push(BookLevel::new(dec!(90.0), dec!(2.0)));

        // Buy 2.0: 1.0 @ 100 + 1.0 @ 110 = 210 / 2 = 105.0 VWAP
        let (vwap_buy, unfilled_buy, slippage_buy) =
            book.buy_execution_vwap(dec!(2.0)).expect("buy vwap");
        assert_eq!(vwap_buy, dec!(105.0));
        assert_eq!(unfilled_buy, dec!(0.0));
        assert_eq!(slippage_buy, (dec!(105.0) - dec!(100.0)) / dec!(100.0));

        // Sell 2.0: 1.0 @ 99 + 1.0 @ 90 = 189 / 2 = 94.5 VWAP
        let (vwap_sell, unfilled_sell, slippage_sell) =
            book.sell_execution_vwap(dec!(2.0)).expect("sell vwap");
        assert_eq!(vwap_sell, dec!(94.5));
        assert_eq!(unfilled_sell, dec!(0.0));
        assert_eq!(slippage_sell, (dec!(99.0) - dec!(94.5)) / dec!(99.0));
    }

    #[test]
    fn test_market_type_properties() {
        assert!(MarketType::Perpetual.has_funding_rate());
        assert!(!MarketType::Spot.has_funding_rate());
        assert!(MarketType::PredictionBinary.is_prediction());
        assert!(MarketType::TraditionalEquity.is_traditional());
        assert!(MarketType::Spot.is_crypto());
    }

    #[test]
    fn test_position_liquidation_distance() {
        let long_pos = Position {
            venue: "hyperliquid".into(),
            symbol: "BTC-USDC".into(),
            side: Side::Buy,
            quantity: dec!(1.0),
            entry_price: dec!(60000.0),
            mark_price: dec!(60000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: Some(dec!(54000.0)),
            leverage: dec!(10),
            initial_margin: dec!(6000.0),
            maintenance_margin: dec!(3000.0),
            updated_at: chrono::Utc::now(),
        };

        // Long liquidation distance = (60000 - 54000) / 60000 = 10%
        let dist = long_pos.liquidation_distance_pct().expect("long dist");
        assert_eq!(dist, dec!(0.10));

        let short_pos = Position {
            venue: "lighter".into(),
            symbol: "BTC-USDC".into(),
            side: Side::Sell,
            quantity: dec!(1.0),
            entry_price: dec!(60000.0),
            mark_price: dec!(60000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: Some(dec!(66000.0)),
            leverage: dec!(10),
            initial_margin: dec!(6000.0),
            maintenance_margin: dec!(3000.0),
            updated_at: chrono::Utc::now(),
        };

        // Short liquidation distance = (66000 - 60000) / 60000 = 10%
        let dist_short = short_pos.liquidation_distance_pct().expect("short dist");
        assert_eq!(dist_short, dec!(0.10));
    }

    #[tokio::test]
    async fn test_market_data_hub_ingestion_and_broadcast() {
        let hub = MarketDataHub::new(128, 1000);
        let mut subscriber = hub.subscribe();

        let mut book = OrderBook::new("hyperliquid".into(), "BTC-USDC".into());
        book.bids.push(BookLevel::new(dec!(60000.0), dec!(1.5)));
        book.asks.push(BookLevel::new(dec!(60005.0), dec!(2.0)));

        let event = hub.update_orderbook(book.clone());
        assert_eq!(event.symbol, "BTC-USDC");
        assert_eq!(event.best_bid, Some(dec!(60000.0)));
        assert_eq!(event.best_ask, Some(dec!(60005.0)));
        assert_eq!(event.sequence, 1);

        // Verify broadcast delivery
        let received = subscriber.recv().await.expect("receive broadcast event");
        assert_eq!(received.sequence, 1);
        assert_eq!(received.venue_id, VenueId::new("hyperliquid"));

        // Verify cache retrieval
        let cached = hub
            .get_orderbook(&VenueId::new("hyperliquid"), "BTC-USDC")
            .expect("cached book");
        assert_eq!(cached.best_bid_price(), Some(dec!(60000.0)));
        assert_eq!(cached.best_ask_price(), Some(dec!(60005.0)));
    }
}
