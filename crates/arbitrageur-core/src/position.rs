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
        let target_base = crate::symbol::canonical_base(symbol);
        self.positions.iter().find(|p| {
            p.symbol.eq_ignore_ascii_case(symbol)
                || crate::symbol::canonical_base(&p.symbol) == target_base
        })
    }

    pub fn get_balance(&self, currency: &str) -> Option<&Balance> {
        self.balances
            .iter()
            .find(|b| b.currency.eq_ignore_ascii_case(currency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_get_position_matches_canonical_base() {
        let snap = AccountSnapshot {
            venue: "lighter".into(),
            equity: dec!(1000),
            free_margin: dec!(1000),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![],
            positions: vec![Position {
                venue: "lighter".into(),
                symbol: "BTC-USDC".into(),
                side: Side::Buy,
                quantity: dec!(0.1),
                entry_price: dec!(60000),
                mark_price: dec!(61000),
                unrealized_pnl: dec!(100),
                realized_pnl: dec!(0),
                leverage: dec!(1),
                liquidation_price: None,
                initial_margin: dec!(6000),
                maintenance_margin: dec!(300),
                updated_at: Utc::now(),
            }],
            timestamp: Utc::now(),
        };

        // Bare symbol matches normalized position
        assert!(snap.get_position("BTC").is_some());
        // Exact symbol matches
        assert!(snap.get_position("BTC-USDC").is_some());
        // Alternative perp spelling matches via canonical_base
        assert!(snap.get_position("PERP_BTC_USDC").is_some());
        // Unrelated symbol does not match
        assert!(snap.get_position("ETH").is_none());
    }
}
