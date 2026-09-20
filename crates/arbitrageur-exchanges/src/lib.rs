pub mod rate_limiter;
pub mod wallet;
pub mod hyperliquid;
pub mod lighter;
pub mod lighter_signer;
pub mod popdex;
pub mod vanta;
pub mod ws;
pub mod robinhood;
pub mod arcus;
pub mod aster;
pub mod binance;
pub mod okx;
pub mod coinbase;
pub mod kraken;
pub mod decibel;
pub mod prediction;
pub mod simulated;

pub use rate_limiter::*;
pub use wallet::*;
pub use hyperliquid::*;
pub use lighter::*;
pub use popdex::*;
pub use vanta::*;
pub use ws::*;
pub use robinhood::*;
pub use arcus::*;
pub use aster::*;
pub use binance::*;
pub use okx::*;
pub use coinbase::*;
pub use kraken::*;
pub use decibel::*;
pub use prediction::*;
pub use simulated::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use arbitrageur_core::exchange::ExchangeAdapter;
    use arbitrageur_core::order::OrderRequest;
    use arbitrageur_core::types::{dec, Decimal, Side, VenueId};

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
        let exchange = SimulatedExchange::new("sim_b", "Simulated Exchange B", 0, dec!(0.0), dec!(1000000.0));
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
        let exchange = SimulatedExchange::new("sim_c", "Simulated Exchange C", 0, dec!(0.0), dec!(1000000.0));
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
        assert_eq!(acc_closed.equity, dec!(1000000.0) + dec!(490.0));
        assert_eq!(acc_closed.maintenance_margin, dec!(0));
    }

    #[tokio::test]
    async fn test_simulated_exchange_partial_fill_on_insufficient_depth() {
        let exchange = SimulatedExchange::new("sim_d", "Simulated Exchange D", 0, dec!(0.0), dec!(1000000.0));
        // 2.0 per level * (1 + i*0.2) for 10 levels: total ask depth = 2.0 * (10 + 0.2*45) = 38.0
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        let order = OrderRequest::new_limit(
            exchange.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(70000.0),
            dec!(50.0),
        );
        let resp = exchange.place_order(order).await.expect("partial fill order");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::PartiallyFilled);
        assert_eq!(resp.filled_qty, dec!(38.0));
        assert_eq!(resp.remaining_qty, dec!(12.0));
        assert!(resp.avg_fill_price.is_some());
    }

    #[tokio::test]
    async fn test_simulated_limit_buy_below_best_ask_rejects() {
        let exchange = SimulatedExchange::new("sim_e", "Simulated Exchange E", 0, dec!(0.0), dec!(10000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60000.0), dec!(1.0));
        let resp = exchange.place_order(order).await.expect("place");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::Rejected);
        assert_eq!(resp.filled_qty, dec!(0));
        assert_eq!(resp.remaining_qty, dec!(1.0));
        assert_eq!(resp.avg_fill_price, None);
        assert_eq!(resp.fee, dec!(0));

        let book = exchange.fetch_orderbook("BTC-USDC").await.expect("book");
        assert_eq!(book.asks[0].size, dec!(2.0));
        let acc = exchange.fetch_account().await.expect("acc");
        assert!(acc.positions.is_empty());
        assert_eq!(acc.equity, dec!(10000.0));
    }

    #[tokio::test]
    async fn test_simulated_limit_buy_fills_only_eligible_asks() {
        let exchange = SimulatedExchange::new("sim_f", "Simulated Exchange F", 0, dec!(0.0), dec!(1000000.0));
        // asks: 60010 @ 2.0, 60010.5 @ 2.4, ...
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        // Limit 60010 accepts only the first ask level: partial fill of 2.0
        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(3.0));
        let resp = exchange.place_order(order).await.expect("place");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::PartiallyFilled);
        assert_eq!(resp.filled_qty, dec!(2.0));
        assert_eq!(resp.remaining_qty, dec!(1.0));
        assert_eq!(resp.avg_fill_price, Some(dec!(60010.0)));

        // Second level must be untouched
        let book = exchange.fetch_orderbook("BTC-USDC").await.expect("book");
        assert_eq!(book.asks[0].price, dec!(60010.5));
        assert_eq!(book.asks[0].size, dec!(2.4));
    }

    #[tokio::test]
    async fn test_simulated_limit_sell_fills_only_eligible_bids() {
        let exchange = SimulatedExchange::new("sim_g", "Simulated Exchange G", 0, dec!(0.0), dec!(1000000.0));
        // bids: 60000 @ 2.0, 59999.5 @ 2.4, ...
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        // Limit 59999.75 accepts only the top bid: partial fill of 2.0
        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Sell, dec!(59999.75), dec!(3.0));
        let resp = exchange.place_order(order).await.expect("place");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::PartiallyFilled);
        assert_eq!(resp.filled_qty, dec!(2.0));
        assert_eq!(resp.remaining_qty, dec!(1.0));
        assert_eq!(resp.avg_fill_price, Some(dec!(60000.0)));

        // Limit above all bids rejects entirely
        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Sell, dec!(60001.0), dec!(1.0));
        let resp = exchange.place_order(order).await.expect("place");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::Rejected);
        assert_eq!(resp.filled_qty, dec!(0));
    }

    #[tokio::test]
    async fn test_replace_orderbook_validates() {
        let exchange = SimulatedExchange::new("sim_h", "Simulated Exchange H", 0, dec!(0.0), dec!(10000.0));

        // Malformed: crossed book rejected
        let mut bad = arbitrageur_core::quote::OrderBook::new(exchange.venue_id(), "ETH-USDC".into());
        bad.bids.push(arbitrageur_core::quote::BookLevel::new(dec!(3100.0), dec!(1.0)));
        bad.asks.push(arbitrageur_core::quote::BookLevel::new(dec!(3000.0), dec!(1.0)));
        assert!(exchange.replace_orderbook(bad).is_err());
        assert!(!exchange.has_market("ETH-USDC"));

        let mut foreign = arbitrageur_core::quote::OrderBook::new("other_venue".into(), "ETH-USDC".into());
        foreign.bids.push(arbitrageur_core::quote::BookLevel::new(dec!(2999.0), dec!(1.0)));
        foreign.asks.push(arbitrageur_core::quote::BookLevel::new(dec!(3001.0), dec!(1.0)));
        assert!(exchange.replace_orderbook(foreign).is_err());
        assert!(!exchange.has_market("ETH-USDC"));

        // Valid book with matching venue accepted and retrievable
        let mut good = arbitrageur_core::quote::OrderBook::new(exchange.venue_id(), "ETH-USDC".into());
        good.bids.push(arbitrageur_core::quote::BookLevel::new(dec!(2999.0), dec!(1.0)));
        good.asks.push(arbitrageur_core::quote::BookLevel::new(dec!(3001.0), dec!(1.0)));
        exchange.replace_orderbook(good).expect("replace");
        let book = exchange.fetch_orderbook("ETH-USDC").await.expect("fetch");
        assert_eq!(book.best_ask_price(), Some(dec!(3001.0)));
    }

    #[tokio::test]
    async fn test_simulated_set_leverage_rejects_out_of_range() {
        let exchange = SimulatedExchange::new("sim_i", "Simulated Exchange I", 0, dec!(0.0), dec!(10000.0));
        assert!(exchange.set_leverage("BTC-USDC", 0).await.is_err());
        assert!(exchange.set_leverage("BTC-USDC", 126).await.is_err());
        assert!(exchange.set_leverage("BTC-USDC", 1).await.is_ok());
        assert!(exchange.set_leverage("BTC-USDC", 5).await.is_ok());
        assert!(exchange.set_leverage("BTC-USDC", 50).await.is_ok());
    }

    #[tokio::test]
    async fn test_simulated_set_leverage_applies_to_new_position() {
        let exchange = SimulatedExchange::new("sim_j", "Simulated Exchange J", 0, dec!(0.0), dec!(100000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));
        exchange.set_leverage("BTC-USDC", 2).await.expect("set leverage");

        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(1.0));
        exchange.place_order(order).await.expect("place");

        let acc = exchange.fetch_account().await.expect("acc");
        let pos = &acc.positions[0];
        assert_eq!(pos.leverage, dec!(2));
        assert_eq!(pos.initial_margin, dec!(1.0) * dec!(60010.0) / dec!(2));
        assert_eq!(pos.maintenance_margin, dec!(1.0) * dec!(60010.0) / dec!(4));
        assert_eq!(acc.free_margin, dec!(100000.0) - pos.initial_margin);
    }

    #[tokio::test]
    async fn test_simulated_set_leverage_updates_existing_position() {
        let exchange = SimulatedExchange::new("sim_k", "Simulated Exchange K", 0, dec!(0.0), dec!(100000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(1.0));
        exchange.place_order(order).await.expect("place");

        exchange.set_leverage("BTC-USDC", 1).await.expect("set leverage");

        let acc = exchange.fetch_account().await.expect("acc");
        let pos = &acc.positions[0];
        assert_eq!(pos.leverage, dec!(1));
        assert_eq!(pos.initial_margin, dec!(1.0) * dec!(60010.0));
        assert_eq!(acc.free_margin, dec!(100000.0) - dec!(60010.0));
    }

    #[tokio::test]
    async fn test_simulated_insufficient_margin_rejects_without_consumption() {
        let exchange = SimulatedExchange::new("sim_l", "Simulated Exchange L", 0, dec!(0.0), dec!(1000.0));
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(2.0), dec!(0.0));

        // 1.0 BTC @ 60010 needs 12002 initial margin at 5x; only 1000 free
        let order = OrderRequest::new_limit(exchange.venue_id(), "BTC-USDC", Side::Buy, dec!(60010.0), dec!(1.0));
        let res = exchange.place_order(order).await;
        assert!(res.is_err());

        let book = exchange.fetch_orderbook("BTC-USDC").await.expect("book");
        assert_eq!(book.asks[0].price, dec!(60010.0));
        assert_eq!(book.asks[0].size, dec!(2.0));
        let acc = exchange.fetch_account().await.expect("acc");
        assert!(acc.positions.is_empty());
        assert_eq!(acc.equity, dec!(1000.0));
        assert_eq!(acc.free_margin, dec!(1000.0));
    }

    #[tokio::test]
    async fn test_simulated_fetch_funding_rate_requires_seed() {
        let exchange = SimulatedExchange::new("sim_m", "Simulated Exchange M", 0, dec!(0.0), dec!(10000.0));
        assert!(exchange.fetch_funding_rate("BTC-USDC").await.is_err());
        exchange.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(1.0), dec!(0.0003));
        let fr = exchange.fetch_funding_rate("BTC-USDC").await.expect("seeded rate");
        assert_eq!(fr.rate, dec!(0.0003));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_simulated_concurrent_orders_cannot_overcommit_margin() {
        let exchange = Arc::new(SimulatedExchange::new(
            "sim_conc",
            "Simulated Exchange Concurrent",
            1,
            dec!(0.0),
            dec!(10000.0),
        ));
        exchange.seed_market("AAA-USDC", dec!(100.0), dec!(100.1), dec!(1000.0), dec!(0.0));
        exchange.seed_market("BBB-USDC", dec!(100.0), dec!(100.1), dec!(1000.0), dec!(0.0));

        let ex_a = exchange.clone();
        let ex_b = exchange.clone();
        let h1 = tokio::spawn(async move {
            ex_a.place_order(OrderRequest::new_limit(
                ex_a.venue_id(),
                "AAA-USDC",
                Side::Buy,
                dec!(100.1),
                dec!(400.0),
            ))
            .await
        });
        let h2 = tokio::spawn(async move {
            ex_b.place_order(OrderRequest::new_limit(
                ex_b.venue_id(),
                "BBB-USDC",
                Side::Buy,
                dec!(100.1),
                dec!(400.0),
            ))
            .await
        });
        let ra = h1.await.expect("join a");
        let rb = h2.await.expect("join b");

        let (a_ok, b_ok) = (ra.is_ok(), rb.is_ok());
        assert_ne!(a_ok, b_ok, "exactly one order must succeed: {:?} {:?}", ra, rb);
        let err = if a_ok { rb.unwrap_err() } else { ra.unwrap_err() };
        assert!(
            err.to_string().contains("Insufficient free margin"),
            "unexpected error: {}",
            err
        );

        let acc = exchange.fetch_account().await.expect("account");
        assert_eq!(acc.positions.len(), 1);
        let total_im: Decimal = acc.positions.iter().map(|p| p.initial_margin).sum();
        assert!(total_im <= acc.equity, "initial margin {} exceeds equity {}", total_im, acc.equity);
        assert!(acc.free_margin >= dec!(0));
        assert_eq!(acc.free_margin, acc.equity - total_im);

        let rejected_symbol = if a_ok { "BBB-USDC" } else { "AAA-USDC" };
        let book = exchange.fetch_orderbook(rejected_symbol).await.expect("book");
        assert_eq!(book.asks.len(), 10);
        assert_eq!(book.asks[0].size, dec!(1000.0));
        let untouched: Decimal = book.asks.iter().map(|l| l.size).sum();
        assert_eq!(untouched, dec!(19000.0), "rejected order consumed book depth");
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

        let mut book = arbitrageur_core::quote::OrderBook::new("lighter".into(), "BTC-USDC".into());
        if let Some(bids) = sample_json.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                let sz = b.get("remaining_base_amount").and_then(|s| s.as_str()).unwrap();
                let px = b.get("price").and_then(|p| p.as_str()).unwrap();
                book.bids.push(arbitrageur_core::quote::BookLevel::new(px.parse().unwrap(), sz.parse().unwrap()));
            }
        }
        if let Some(asks) = sample_json.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                let sz = a.get("remaining_base_amount").and_then(|s| s.as_str()).unwrap();
                let px = a.get("price").and_then(|p| p.as_str()).unwrap();
                book.asks.push(arbitrageur_core::quote::BookLevel::new(px.parse().unwrap(), sz.parse().unwrap()));
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

    #[tokio::test]
    async fn test_arcus_adapter() {
        let arcus = ArcusAdapter::new(None, None, None);
        assert_eq!(arcus.name(), "Arcus");
        assert_eq!(arcus.venue_id(), VenueId::new("arcus"));
        assert!(arcus.fetch_orderbook("BTC-USDC").await.is_err());
    }

    #[tokio::test]
    async fn test_aster_adapter() {
        let aster = AsterAdapter::new(None, None, None);
        assert_eq!(aster.name(), "Aster");
        assert_eq!(aster.venue_id(), VenueId::new("aster"));
        // Unknown bases fail closed (no quote guessing); offline everything fails closed.
        assert!(aster.fetch_orderbook("ZZZNOTREAL").await.is_err());
        assert!(aster.fetch_account().await.is_err());
        assert!(aster.set_leverage("BTC-USDT", 5).await.is_err());
    }

    #[tokio::test]
    async fn test_new_universal_adapters() {
        let binance = BinanceAdapter::new(None, None, None);
        assert_eq!(binance.name(), "Binance");
        assert_eq!(binance.venue_id(), VenueId::new("binance"));
        assert!(binance.fetch_account().await.is_err());

        let okx = OkxAdapter::new(None, None, None, None);
        assert_eq!(okx.name(), "OKX");
        assert_eq!(okx.venue_id(), VenueId::new("okx"));
        assert!(okx.fetch_account().await.is_err());

        let coinbase = CoinbaseAdapter::new(None, None, None, None);
        assert_eq!(coinbase.name(), "Coinbase");
        assert_eq!(coinbase.venue_id(), VenueId::new("coinbase"));
        assert!(coinbase.fetch_account().await.is_err());

        let kraken = KrakenAdapter::new(None, None, None, None);
        assert_eq!(kraken.name(), "Kraken");
        assert_eq!(kraken.venue_id(), VenueId::new("kraken"));
        assert!(kraken.fetch_account().await.is_err());

        let decibel = DecibelAdapter::new(None, None, None);
        assert_eq!(decibel.name(), "Decibel");
        assert_eq!(decibel.venue_id(), VenueId::new("decibel"));
        assert!(decibel.fetch_account().await.is_err());

        let robinhood = RobinhoodAdapter::new(None, None, None, None);
        assert_eq!(robinhood.name(), "Robinhood");
        assert_eq!(robinhood.venue_id(), VenueId::new("robinhood"));
        assert!(robinhood.fetch_account().await.is_err());
    }
}
