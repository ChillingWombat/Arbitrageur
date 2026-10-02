use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::types::{dec, Decimal};
use tracing::{debug, warn};

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
    /// Liquidation/margin data missing — safety cannot be verified.
    /// Never reported as Healthy: an unverifiable position is a risk the
    /// monitor cannot rule out.
    Unknown,
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
            debug!(
                "[{}] CRITICAL margin utilization: {}% (limit: {}%)",
                account.venue, usage, self.config.critical_margin_usage_pct
            );
            MarginHealthStatus::Critical
        } else if usage >= self.config.warning_margin_usage_pct {
            debug!(
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
            // No liquidation price from the venue — the distance to a
            // liquidation cannot be computed, so the position's safety is
            // unverified. Fail closed: never claim Healthy.
            MarginHealthStatus::Unknown
        }
    }

    /// Portfolio-margin liquidation distance: the fraction of account equity
    /// that can evaporate before equity reaches the maintenance requirement
    /// and the venue liquidates. Venues like Deribit never emit per-position
    /// liq prices — liquidation strikes at the account level — so this is
    /// the honest distance for books that hold them. `None` when equity is
    /// non-positive (already at/inside the boundary) or the venue reported
    /// no maintenance figure at all.
    pub fn account_liquidation_distance(account: &AccountSnapshot) -> Option<Decimal> {
        if account.equity <= dec!(0) || account.maintenance_margin <= dec!(0) {
            return None;
        }
        Some((account.equity - account.maintenance_margin) / account.equity)
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
    fn account_liq_distance_uses_equity_vs_maintenance() {
        let mut acc = empty_account();
        acc.equity = dec!(1000);
        acc.maintenance_margin = dec!(200);
        // Liquidation when equity falls to maintenance: (1000-200)/1000 = 80%.
        assert_eq!(
            MarginMonitor::account_liquidation_distance(&acc),
            Some(dec!(0.8))
        );
        // No maintenance figure → cannot compute.
        acc.maintenance_margin = dec!(0);
        assert_eq!(MarginMonitor::account_liquidation_distance(&acc), None);
        // Equity already at/under the boundary → no distance to report.
        acc.equity = dec!(0);
        acc.maintenance_margin = dec!(200);
        assert_eq!(MarginMonitor::account_liquidation_distance(&acc), None);
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

    /// Regression (#46): a position with no liquidation price cannot be
    /// verified safe — it must never report Healthy.
    #[test]
    fn position_without_liquidation_price_is_unknown_not_healthy() {
        use arbitrageur_core::position::Position;
        use arbitrageur_core::types::Side;
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        let pos = Position {
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
        };
        assert_eq!(m.evaluate_position(&pos), MarginHealthStatus::Unknown);

        // With a liquidation price far away, the same position is Healthy.
        let mut pos = pos;
        pos.liquidation_price = Some(dec!(30000));
        assert_eq!(m.evaluate_position(&pos), MarginHealthStatus::Healthy);
    }
}
