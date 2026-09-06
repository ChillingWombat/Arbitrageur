pub mod rolling_stats;
pub mod spatial_spread;
pub mod funding_carry;
pub mod ou_process;
pub mod optimal_thresholds;
pub mod kalman_filter;
pub mod cointegration;
pub mod prediction_synthetic;
pub mod risk_math;

pub use rolling_stats::*;
pub use spatial_spread::*;
pub use funding_carry::*;
pub use ou_process::*;
pub use optimal_thresholds::*;
pub use kalman_filter::*;
pub use cointegration::*;
pub use prediction_synthetic::*;
pub use risk_math::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrager_core::types::dec;
    use arbitrager_core::quote::{BookLevel, OrderBook, FundingRate};
    use chrono::Utc;

    #[test]
    fn test_welford_stats() {
        let mut stats = OnlineWelfordStats::new();
        let values = [10.0, 12.0, 8.0, 14.0, 16.0];
        for v in values {
            stats.update(v);
        }
        assert_eq!(stats.count(), 5);
        assert!((stats.mean() - 12.0).abs() < 1e-6);
        assert!((stats.variance() - 10.0).abs() < 1e-6);
        assert!((stats.std_dev() - 10.0_f64.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn test_spatial_spread_evaluation() {
        let mut buy_book = OrderBook::new("venue_a".into(), "BTC-USD".into());
        buy_book.asks.push(BookLevel::new(dec!(60000), dec!(1.5)));
        buy_book.asks.push(BookLevel::new(dec!(60010), dec!(2.0)));

        let mut sell_book = OrderBook::new("venue_b".into(), "BTC-USD".into());
        sell_book.bids.push(BookLevel::new(dec!(60100), dec!(1.0)));
        sell_book.bids.push(BookLevel::new(dec!(60090), dec!(2.0)));

        let opp = evaluate_spatial_spread(
            &buy_book,
            &sell_book,
            dec!(2), // 2 bps fee
            dec!(2), // 2 bps fee
            dec!(1.0),
        ).expect("Opportunity should exist");

        assert!(opp.gross_spread > dec!(0));
        assert!(opp.net_profit_per_unit > dec!(0));
        assert_eq!(opp.executable_qty, dec!(1.0));
    }

    #[test]
    fn test_funding_carry_evaluation() {
        let now = Utc::now();
        let rate_a = FundingRate {
            venue: "hyperliquid".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.0003), // 0.03% per hour
            predicted_rate: None,
            interval_hours: 1,
            next_funding_time: now,
            timestamp: now,
        };

        let rate_b = FundingRate {
            venue: "lighter".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.0001), // 0.01% per 8 hours
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: now,
            timestamp: now,
        };

        let carry = evaluate_funding_carry(&rate_a, &rate_b, dec!(5), dec!(10000))
            .expect("Should find carry opportunity");

        assert_eq!(carry.short_venue, "hyperliquid");
        assert_eq!(carry.long_venue, "lighter");
        assert!(carry.net_apr > dec!(0));
        assert!(carry.daily_income_usd > dec!(0));
    }

    #[test]
    fn test_kalman_filter_convergence() {
        let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
        // True relationship: y = 2.5 * x + 5.0
        for i in 1..=100 {
            let x = i as f64 * 0.5;
            let y = 2.5 * x + 5.0 + ((i % 3) as f64 - 1.0) * 0.05;
            kf.update(x, y);
        }
        assert!((kf.hedge_ratio() - 2.5).abs() < 0.2);
    }

    #[test]
    fn test_prediction_synthetic_arbitrage() {
        let opp = evaluate_prediction_cross_venue(
            "Polymarket",
            dec!(0.40), // YES
            dec!(0.60), // NO
            "Kalshi",
            dec!(0.48), // YES
            dec!(0.55), // NO
            dec!(10),   // 10 BPS fee
        ).expect("Arbitrage should exist: Polymarket YES (0.40) + Kalshi NO (0.55) = 0.95 < 1.00");

        assert_eq!(opp.buy_yes_venue, "Polymarket");
        assert_eq!(opp.buy_no_venue, "Kalshi");
        assert!(opp.net_profit > dec!(0));
    }
}
