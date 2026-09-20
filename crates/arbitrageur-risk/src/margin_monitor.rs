use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::types::{dec, Decimal};
use tracing::warn;

/// Health status of an account or position margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginHealthStatus {
    /// Funded account with comfortable margin.
    Healthy,
    Warning,
    Critical,
    Breached,
    /// Unfunded account: zero equity, no open positions, no margin locked.
    /// Nothing is at risk, so this is informational — never critical.
    Empty,
}

/// Margin monitor configuration.
#[derive(Debug, Clone)]
pub struct MarginMonitorConfig {
    /// Warning threshold for margin utilization percentage (e.g. 70%)
    pub warning_margin_usage_pct: Decimal,
    /// Critical threshold for margin utilization percentage (e.g. 85%)
    pub critical_margin_usage_pct: Decimal,
    /// Minimum allowed liquidation buffer distance (e.g. 15%)
    pub min_liquidation_distance_pct: Decimal,
}

impl Default for MarginMonitorConfig {
    fn default() -> Self {
        Self {
            warning_margin_usage_pct: dec!(70.0),
            critical_margin_usage_pct: dec!(85.0),
            min_liquidation_distance_pct: dec!(15.0),
        }
    }
}

pub struct MarginMonitor {
    config: MarginMonitorConfig,
}

impl MarginMonitor {
    pub fn new(config: MarginMonitorConfig) -> Self {
        Self { config }
    }

    /// Assess overall health of an account snapshot.
    pub fn evaluate_account(&self, account: &AccountSnapshot) -> MarginHealthStatus {
        if account.equity <= dec!(0) {
            // Unfunded and flat: nothing at risk — informational, not a breach.
            // Zero/negative equity WITH positions or locked margin stays Breached.
            if account.positions.is_empty() && account.maintenance_margin <= dec!(0) {
                return MarginHealthStatus::Empty;
            }
            return MarginHealthStatus::Breached;
        }

        let usage = account.margin_usage_pct;
        if usage >= self.config.critical_margin_usage_pct {
            warn!(
                "[{}] CRITICAL margin utilization: {}% (limit: {}%)",
                account.venue, usage, self.config.critical_margin_usage_pct
            );
            MarginHealthStatus::Critical
        } else if usage >= self.config.warning_margin_usage_pct {
            warn!(
                "[{}] High margin utilization: {}% (warning: {}%)",
                account.venue, usage, self.config.warning_margin_usage_pct
            );
            MarginHealthStatus::Warning
        } else {
            MarginHealthStatus::Healthy
        }
    }

    /// Assess position liquidation proximity.
    pub fn evaluate_position(&self, pos: &Position) -> MarginHealthStatus {
        if let Some(distance) = pos.liquidation_distance_pct() {
            let distance_pct = distance * dec!(100.0);
            if distance_pct <= dec!(5.0) {
                warn!(
                    "[{}] Position {} distance to liquidation is dangerously low: {:.2}%",
                    pos.venue, pos.symbol, distance_pct
                );
                MarginHealthStatus::Critical
            } else if distance_pct <= self.config.min_liquidation_distance_pct {
                warn!(
                    "[{}] Position {} liquidation buffer below safe threshold: {:.2}%",
                    pos.venue, pos.symbol, distance_pct
                );
                MarginHealthStatus::Warning
            } else {
                MarginHealthStatus::Healthy
            }
        } else {
            MarginHealthStatus::Healthy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::VenueId;

    fn empty_account() -> AccountSnapshot {
        AccountSnapshot {
            venue: VenueId("hyperliquid".into()),
            equity: dec!(0),
            free_margin: dec!(0),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![],
            positions: vec![],
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn unfunded_flat_account_is_empty_not_breached() {
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        assert_eq!(
            m.evaluate_account(&empty_account()),
            MarginHealthStatus::Empty
        );
    }

    #[test]
    fn zero_equity_with_positions_stays_breached() {
        use arbitrageur_core::position::Position;
        use arbitrageur_core::types::Side;
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        let mut acc = empty_account();
        acc.positions.push(Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-USDC".into(),
            side: Side::Buy,
            quantity: dec!(1),
            entry_price: dec!(60000),
            mark_price: dec!(60000),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(12000),
            maintenance_margin: dec!(6000),
            updated_at: chrono::Utc::now(),
        });
        assert_eq!(m.evaluate_account(&acc), MarginHealthStatus::Breached);
    }
}
