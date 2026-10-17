//! Real-Time Order Book Queue Priority Tracking & Active Quote Seniority Monitoring.
//!
//! Tracks resting limit order queue progression in FIFO order books:
//! 1. Initial queue ahead $q_{\text{ahead}}(t_0) = \max(0, Q_{\text{level}}(P, t_0) - q_{\text{our}})$.
//! 2. Continuous updates on trades and cancellations:
//!    $$q_{\text{ahead}}(t + \Delta t) = \max\left(0, q_{\text{ahead}}(t) - \Delta V_{\text{trade}}(t) - \frac{q_{\text{ahead}}(t)}{Q_{\text{level}}(t)} \cdot \Delta V_{\text{cancel}}(t)\right)$$
//! 3. Real-time queue seniority:
//!    $$S_{\text{queue}}(t) = 1 - \frac{q_{\text{ahead}}(t)}{Q_{\text{level}}(t)}$$
//! 4. Integration with `QueuePriorityHazardEngine` for proactive pick-off quote evacuation.

use arbitrageur_core::types::{Decimal, Side};
use arbitrageur_math::queue_priority::{
    QueueHazardRecommendation, QueuePositionSnapshot, QueuePriorityHazardEngine,
};
use serde::{Deserialize, Serialize};

/// Real-time tracker for a resting limit order's queue position and seniority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuePriorityTracker {
    pub order_id: String,
    pub price: Decimal,
    pub side: Side,
    pub initial_depth: Decimal,
    pub our_qty: Decimal,
    pub q_ahead: Decimal,
    pub total_depth: Decimal,
    pub initial_mid: Decimal,
    pub current_mid: Decimal,
    pub created_at_ms: i64,
    pub last_update_ms: i64,
}

impl QueuePriorityTracker {
    pub fn new(
        order_id: String,
        price: Decimal,
        side: Side,
        total_depth_at_entry: Decimal,
        our_qty: Decimal,
        initial_mid: Decimal,
    ) -> Self {
        let q_ahead = (total_depth_at_entry - our_qty).max(Decimal::ZERO);
        let now_ms = chrono::Utc::now().timestamp_millis();
        Self {
            order_id,
            price,
            side,
            initial_depth: total_depth_at_entry,
            our_qty,
            q_ahead,
            total_depth: total_depth_at_entry.max(our_qty),
            initial_mid,
            current_mid: initial_mid,
            created_at_ms: now_ms,
            last_update_ms: now_ms,
        }
    }

    /// Updates queue position based on a fresh orderbook snapshot and matched trade volume.
    pub fn update_from_book(
        &mut self,
        new_total_depth: Decimal,
        trade_volume_at_price: Decimal,
        current_mid: Decimal,
    ) {
        let trades = trade_volume_at_price.max(Decimal::ZERO);
        let depth_delta = self.total_depth - new_total_depth;
        let cancels_total = (depth_delta - trades).max(Decimal::ZERO);

        let cancel_prob = if self.total_depth > Decimal::ZERO {
            (self.q_ahead / self.total_depth).min(Decimal::ONE)
        } else {
            Decimal::ZERO
        };

        let cancels_ahead = cancels_total * cancel_prob;
        self.q_ahead = (self.q_ahead - trades - cancels_ahead).max(Decimal::ZERO);
        self.total_depth = new_total_depth.max(self.our_qty);
        self.current_mid = current_mid;
        self.last_update_ms = chrono::Utc::now().timestamp_millis();
    }

    /// Queue seniority ratio $S_{\text{queue}} \in [0.0, 1.0]$.
    pub fn seniority_ratio(&self) -> f64 {
        if self.total_depth <= Decimal::ZERO {
            return 1.0;
        }
        let ratio = Decimal::ONE - (self.q_ahead / self.total_depth);
        ratio
            .to_string()
            .parse::<f64>()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }

    /// Computes adverse mid-price drift in basis points.
    pub fn adverse_drift_bps(&self) -> f64 {
        if self.initial_mid <= Decimal::ZERO {
            return 0.0;
        }
        let drift_pct = match self.side {
            // For buy orders, adverse drift is when market falls below initial mid
            Side::Buy => (self.initial_mid - self.current_mid) / self.initial_mid,
            // For sell orders, adverse drift is when market rises above initial mid
            Side::Sell => (self.current_mid - self.initial_mid) / self.initial_mid,
        };

        let bps = drift_pct * Decimal::from(10000);
        bps.to_string().parse::<f64>().unwrap_or(0.0).max(0.0)
    }

    /// Evaluates resting order health against the Queue Priority Hazard Engine.
    pub fn evaluate_action(
        &self,
        engine: &QueuePriorityHazardEngine,
    ) -> (QueueHazardRecommendation, QueuePositionSnapshot) {
        let q_ahead_f64 = self.q_ahead.to_string().parse::<f64>().unwrap_or(0.0);
        let total_depth_f64 = self.total_depth.to_string().parse::<f64>().unwrap_or(1.0);
        let dwell_time_sec = ((self.last_update_ms - self.created_at_ms).max(0) as f64) / 1000.0;
        let adverse_drift = self.adverse_drift_bps();

        engine.evaluate_resting_quote(q_ahead_f64, total_depth_f64, dwell_time_sec, adverse_drift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::dec;

    #[test]
    fn test_queue_priority_tracker_depletion_on_trades() {
        let mut tracker = QueuePriorityTracker::new(
            "order-123".into(),
            dec!(3000.0),
            Side::Buy,
            dec!(100.0), // Total depth
            dec!(10.0),  // Our qty -> q_ahead = 90.0
            dec!(3000.0),
        );
        assert_eq!(tracker.q_ahead, dec!(90.0));
        assert!((tracker.seniority_ratio() - 0.10).abs() < 1e-4);

        // Trade arrives consuming 50.0 ahead
        tracker.update_from_book(dec!(50.0), dec!(50.0), dec!(3000.0));
        assert_eq!(tracker.q_ahead, dec!(40.0));
        assert!((tracker.seniority_ratio() - 0.20).abs() < 1e-4);

        // Another trade consumes the remaining 40.0 ahead -> we are now front of queue
        tracker.update_from_book(dec!(10.0), dec!(40.0), dec!(3000.0));
        assert_eq!(tracker.q_ahead, dec!(0.0));
        assert_eq!(tracker.seniority_ratio(), 1.0);
    }

    #[test]
    fn test_queue_priority_adverse_drift_and_pickoff() {
        let mut tracker = QueuePriorityTracker::new(
            "order-456".into(),
            dec!(3000.0),
            Side::Buy,
            dec!(100.0),
            dec!(5.0),
            dec!(3000.0),
        );

        // Adverse price jump: mid drops from 3000 to 2991 (30 bps drop against our resting buy)
        tracker.update_from_book(dec!(100.0), dec!(0.0), dec!(2991.0));
        assert!(tracker.adverse_drift_bps() >= 29.0);

        let engine = QueuePriorityHazardEngine::default();
        let (rec, snap) = tracker.evaluate_action(&engine);
        assert!(matches!(
            rec,
            QueueHazardRecommendation::ProactiveQuoteWithdrawal { .. }
        ));
        assert!(snap.pickoff_hazard_index >= 0.70);
    }
}
