use tracing::warn;
use arbitrager_core::position::{AccountSnapshot, Position};
use arbitrager_core::types::{dec, Decimal};

/// Health status of an account or position margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginHealthStatus {
    Healthy,
    Warning,
    Critical,
    Breached,
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
