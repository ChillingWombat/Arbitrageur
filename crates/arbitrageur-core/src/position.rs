use crate::types::{dec, Decimal, Side, VenueId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Derivative / Perp / Spot position details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub venue: VenueId,
    pub symbol: String,
    pub side: Side,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub liquidation_price: Option<Decimal>,
    pub leverage: Decimal,
    pub initial_margin: Decimal,
    pub maintenance_margin: Decimal,
    pub updated_at: DateTime<Utc>,
}

impl Position {
    pub fn notional(&self) -> Decimal {
        self.quantity * self.mark_price
    }

    /// Signed quantity (+ for Long, - for Short).
    pub fn signed_quantity(&self) -> Decimal {
        self.quantity * self.side.sign()
    }

    /// Margin ratio = Maintenance Margin / Equity (or Notional)
    pub fn margin_ratio(&self, equity: Decimal) -> Decimal {
        if equity <= dec!(0) {
            dec!(1)
        } else {
            self.maintenance_margin / equity
        }
    }

    /// Distance to liquidation in percent.
    pub fn liquidation_distance_pct(&self) -> Option<Decimal> {
        let liq = self.liquidation_price?;
        if self.mark_price <= dec!(0) {
            return None;
        }
        match self.side {
            Side::Buy => Some((self.mark_price - liq) / self.mark_price),
            Side::Sell => Some((liq - self.mark_price) / self.mark_price),
        }
    }
}

/// Asset balance in an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub venue: VenueId,
    pub currency: String,
    pub total: Decimal,
    pub free: Decimal,
    pub locked: Decimal,
}

/// Consolidated snapshot of an account at a venue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub venue: VenueId,
    pub equity: Decimal,
    pub free_margin: Decimal,
    pub maintenance_margin: Decimal,
    pub margin_usage_pct: Decimal,
    pub balances: Vec<Balance>,
    pub positions: Vec<Position>,
    pub timestamp: DateTime<Utc>,
}

impl AccountSnapshot {
    pub fn get_position(&self, symbol: &str) -> Option<&Position> {
        self.positions
            .iter()
            .find(|p| p.symbol.eq_ignore_ascii_case(symbol))
    }

    pub fn get_balance(&self, currency: &str) -> Option<&Balance> {
        self.balances
            .iter()
            .find(|b| b.currency.eq_ignore_ascii_case(currency))
    }
}
