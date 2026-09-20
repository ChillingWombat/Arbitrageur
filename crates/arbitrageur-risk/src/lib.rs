pub mod circuit_breaker;
pub mod delta_guard;
pub mod margin_monitor;
pub mod safety_guards;

pub use circuit_breaker::*;
pub use delta_guard::*;
pub use margin_monitor::*;
pub use safety_guards::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::position::Position;
    use arbitrageur_core::types::{dec, Side};
    use chrono::Utc;

    #[test]
    fn test_circuit_breaker_drawdown() {
        let config = CircuitBreakerConfig {
            max_daily_drawdown_usd: dec!(500.0),
            max_daily_drawdown_pct: dec!(5.0),
            max_consecutive_errors: 3,
            max_spread_divergence_bps: dec!(200.0),
        };

        let cb = CircuitBreaker::new(config, dec!(10000.0));
        assert!(cb.is_trading_allowed().is_ok());

        cb.update_equity(dec!(9800.0));
        assert!(cb.is_trading_allowed().is_ok());

        cb.update_equity(dec!(9400.0)); // Down 600 USD (> 500)
        assert!(cb.is_trading_allowed().is_err());
    }

    #[test]
    fn test_delta_guard_neutrality() {
        let guard = DeltaGuard::new(dec!(200.0)); // $200 tolerance

        let pos1 = Position {
            venue: "hyperliquid".into(),
            symbol: "ETH-PERP".into(),
            side: Side::Buy,
            quantity: dec!(1.0),
            entry_price: dec!(3000.0),
            mark_price: dec!(3000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(600.0),
            maintenance_margin: dec!(300.0),
            updated_at: Utc::now(),
        };

        let pos2 = Position {
            venue: "lighter".into(),
            symbol: "ETH-PERP".into(),
            side: Side::Sell,
            quantity: dec!(0.95), // 0.05 ETH unhedged = $150 (< $200)
            entry_price: dec!(3000.0),
            mark_price: dec!(3000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(600.0),
            maintenance_margin: dec!(300.0),
            updated_at: Utc::now(),
        };

        assert!(guard.check_delta_neutrality(&[pos1.clone(), pos2]).is_ok());

        let pos3 = Position {
            venue: "lighter".into(),
            symbol: "ETH-PERP".into(),
            side: Side::Sell,
            quantity: dec!(0.80), // 0.20 ETH unhedged = $600 (> $200)
            entry_price: dec!(3000.0),
            mark_price: dec!(3000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(600.0),
            maintenance_margin: dec!(300.0),
            updated_at: Utc::now(),
        };

        assert!(guard.check_delta_neutrality(&[pos1, pos3]).is_err());
    }
}
