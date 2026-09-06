use std::collections::HashMap;
use tracing::warn;
use arbitrager_core::position::Position;
use arbitrager_core::types::{dec, Decimal};

/// Portfolio delta neutrality tracker.
#[derive(Debug, Clone)]
pub struct DeltaGuard {
    /// Maximum allowed unhedged notional in USD per base asset
    max_unhedged_usd_per_asset: Decimal,
}

impl DeltaGuard {
    pub fn new(max_unhedged_usd_per_asset: Decimal) -> Self {
        Self {
            max_unhedged_usd_per_asset,
        }
    }

    /// Aggregates positions across all venues by base currency and returns net delta and USD exposure.
    pub fn calculate_net_delta(&self, positions: &[Position]) -> HashMap<String, (Decimal, Decimal)> {
        let mut delta_map: HashMap<String, (Decimal, Decimal)> = HashMap::new();

        for pos in positions {
            let base = pos.symbol.split('-').next().unwrap_or(&pos.symbol).to_uppercase();
            let signed_qty = pos.signed_quantity();
            let signed_usd = signed_qty * pos.mark_price;

            let entry = delta_map.entry(base).or_insert((dec!(0), dec!(0)));
            entry.0 += signed_qty;
            entry.1 += signed_usd;
        }

        delta_map
    }

    /// Checks if any base asset violates delta neutrality limits.
    pub fn check_delta_neutrality(&self, positions: &[Position]) -> Result<(), (String, Decimal)> {
        let deltas = self.calculate_net_delta(positions);

        for (base, (_qty, usd_exposure)) in deltas {
            let abs_usd = usd_exposure.abs();
            if abs_usd > self.max_unhedged_usd_per_asset {
                warn!(
                    "Unhedged delta breach on asset {}: Net USD exposure ${:.2} (limit: ${:.2})",
                    base, usd_exposure, self.max_unhedged_usd_per_asset
                );
                return Err((base, usd_exposure));
            }
        }

        Ok(())
    }
}
