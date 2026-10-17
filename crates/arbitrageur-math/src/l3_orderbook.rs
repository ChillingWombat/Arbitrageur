//! Level-3 (L3) Order-by-Order Queue Simulation and Priority Tracking.
//!
//! Provides deterministic microstructural reconstruction of high-frequency exchange order books:
//! 1. Order-level discrete event mechanics: Add, Cancel, Match, Replace.
//! 2. Strict Price-Time (FIFO) priority tracking for resting maker orders:
//!    exact ordinal rank $k$ and cumulative volume ahead $V_{\text{ahead}} = \sum_{j < k} \text{size}_j$.
//! 3. Closed-form conditional fill probability estimation $P(\text{Fill} \mid V_{\text{ahead}}, \tau)$
//!    under competing queue trade matching hazard $\lambda_m$ and cancellation hazard $\lambda_c$.
//! 4. Zero-copy conversion to Level-2 (L2) `OrderBook` snapshots for downstream strategies.

use arbitrageur_core::quote::{BookLevel, OrderBook};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Single order entry in an L3 price queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3OrderEntry {
    pub order_id: String,
    pub price: Decimal,
    pub size: Decimal,
    pub priority_timestamp_ns: u64,
}

impl L3OrderEntry {
    pub fn new(
        order_id: String,
        price: Decimal,
        size: Decimal,
        priority_timestamp_ns: u64,
    ) -> Self {
        Self {
            order_id,
            price,
            size,
            priority_timestamp_ns,
        }
    }
}

/// Queue position telemetry for a specific resting order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuePosition {
    pub order_id: String,
    /// 1-indexed queue rank (1 = next in line to be filled)
    pub rank: usize,
    /// Cumulative volume ahead of this order in the FIFO queue
    pub volume_ahead: Decimal,
    /// Quantity of this resting order
    pub order_size: Decimal,
    /// Total volume resting at this price level
    pub total_level_volume: Decimal,
    /// Proportion of total level volume represented by this order
    pub queue_share: f64,
}

/// Execution fill event on an individual L3 resting order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3MatchFill {
    pub order_id: String,
    pub fill_size: Decimal,
    pub price: Decimal,
    pub remaining_order_size: Decimal,
}

/// FIFO Priority Queue at a single discrete price tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3PriceQueue {
    pub price: Decimal,
    pub total_volume: Decimal,
    orders: VecDeque<L3OrderEntry>,
}

impl L3PriceQueue {
    pub fn new(price: Decimal) -> Self {
        Self {
            price,
            total_volume: dec!(0),
            orders: VecDeque::new(),
        }
    }

    /// Number of distinct resting orders in this queue.
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Appends a new limit order to the back of the FIFO queue.
    pub fn add_order(&mut self, order: L3OrderEntry) {
        self.total_volume += order.size;
        self.orders.push_back(order);
    }

    /// Cancels or reduces an existing resting order.
    ///
    /// If `cancel_size` is `None` or >= existing size, the order is completely purged.
    /// If `cancel_size` is strictly less than existing size, the order size is decremented in-place,
    /// strictly preserving its front FIFO priority rank.
    ///
    /// Returns the actual size subtracted from the book.
    pub fn cancel_order(
        &mut self,
        order_id: &str,
        cancel_size: Option<Decimal>,
    ) -> Option<Decimal> {
        let idx = self.orders.iter().position(|o| o.order_id == order_id)?;
        let entry = &mut self.orders[idx];

        let to_remove = match cancel_size {
            Some(sz) if sz > dec!(0) && sz < entry.size => sz,
            _ => entry.size,
        };

        if to_remove >= entry.size {
            let removed = self.orders.remove(idx).expect("index checked");
            self.total_volume = (self.total_volume - removed.size).max(dec!(0));
            Some(removed.size)
        } else {
            entry.size -= to_remove;
            self.total_volume = (self.total_volume - to_remove).max(dec!(0));
            Some(to_remove)
        }
    }

    /// Consumes resting orders from the front of the queue (FIFO price-time matching).
    pub fn match_front(&mut self, mut match_size: Decimal) -> Vec<L3MatchFill> {
        let mut fills = Vec::new();
        while match_size > dec!(0) && !self.orders.is_empty() {
            let front = self.orders.front_mut().expect("non-empty");
            if front.size <= match_size {
                let fill_qty = front.size;
                match_size -= fill_qty;
                self.total_volume = (self.total_volume - fill_qty).max(dec!(0));
                let filled = self.orders.pop_front().expect("front exists");
                fills.push(L3MatchFill {
                    order_id: filled.order_id,
                    fill_size: fill_qty,
                    price: self.price,
                    remaining_order_size: dec!(0),
                });
            } else {
                let fill_qty = match_size;
                front.size -= fill_qty;
                self.total_volume = (self.total_volume - fill_qty).max(dec!(0));
                let remaining = front.size;
                match_size = dec!(0);
                fills.push(L3MatchFill {
                    order_id: front.order_id.clone(),
                    fill_size: fill_qty,
                    price: self.price,
                    remaining_order_size: remaining,
                });
            }
        }
        fills
    }

    /// Replaces an order's size and/or price.
    ///
    /// Rules of exchange matching engines (CME, Binance, Hyperliquid):
    /// - Size decrease at same price: keeps existing FIFO queue rank.
    /// - Size increase at same price: loses priority (moved to back of queue).
    /// - Any price modification: loses priority (must be re-routed to new queue).
    pub fn replace_order(
        &mut self,
        order_id: &str,
        new_size: Decimal,
        timestamp_ns: u64,
    ) -> Result<Option<L3OrderEntry>, String> {
        if new_size <= dec!(0) {
            self.cancel_order(order_id, None);
            return Ok(None);
        }

        let idx = self
            .orders
            .iter()
            .position(|o| o.order_id == order_id)
            .ok_or_else(|| format!("order {} not found in queue", order_id))?;

        let old_size = self.orders[idx].size;
        if new_size <= old_size {
            // Size decrease: keep priority!
            let diff = old_size - new_size;
            self.orders[idx].size = new_size;
            self.total_volume = (self.total_volume - diff).max(dec!(0));
            Ok(None)
        } else {
            // Size increase: loses priority, move to tail
            let mut order = self.orders.remove(idx).expect("idx valid");
            self.total_volume = (self.total_volume - old_size).max(dec!(0));
            order.size = new_size;
            order.priority_timestamp_ns = timestamp_ns;
            self.add_order(order.clone());
            Ok(Some(order))
        }
    }

    /// Computes queue position telemetry for a specific resting order.
    pub fn queue_position_of(&self, order_id: &str) -> Option<QueuePosition> {
        let mut volume_ahead = dec!(0);
        for (rank_zero, entry) in self.orders.iter().enumerate() {
            if entry.order_id == order_id {
                let total_vol = self.total_volume;
                let share = if total_vol > dec!(0) {
                    (entry.size / total_vol)
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                return Some(QueuePosition {
                    order_id: entry.order_id.clone(),
                    rank: rank_zero + 1,
                    volume_ahead,
                    order_size: entry.size,
                    total_level_volume: total_vol,
                    queue_share: share,
                });
            }
            volume_ahead += entry.size;
        }
        None
    }

    /// Estimates conditional fill probability $P(\text{Fill} \mid V_{\text{ahead}}, \tau)$
    /// using Cont-de Larrard (2013) absorbing Markov pure-death queue depletion dynamics:
    ///
    /// The order fills when cumulative market sweeps plus cancellations ahead reach $V_{\text{ahead}} + s$.
    ///
    /// $$P_{\text{fill}}(\tau \mid V_{\text{ahead}}) = \Phi\left(\frac{\mu \tau - V_{\text{ahead}}}{\sigma \sqrt{\tau}}\right) - \exp\left(\frac{2 \mu V_{\text{ahead}}}{\sigma^2}\right) \Phi\left(\frac{-\mu \tau - V_{\text{ahead}}}{\sigma \sqrt{\tau}}\right)$$
    pub fn estimate_fill_probability(
        &self,
        order_id: &str,
        horizon_sec: f64,
        match_rate: f64,
        cancel_rate: f64,
        diffusion_sigma: f64,
    ) -> Option<f64> {
        let pos = self.queue_position_of(order_id)?;
        if horizon_sec <= 0.0 {
            return Some(if pos.rank == 1 { 1.0 } else { 0.0 });
        }

        let v_ahead = pos.volume_ahead.to_string().parse::<f64>().unwrap_or(0.0);
        let v_total = pos
            .total_level_volume
            .to_string()
            .parse::<f64>()
            .unwrap_or(1.0);
        let s_order = pos.order_size.to_string().parse::<f64>().unwrap_or(1.0);

        // Required volume to drain: all volume ahead plus half of our own order
        let threshold = v_ahead + 0.5 * s_order;
        if threshold <= 0.0 {
            return Some(1.0);
        }

        // Effective depletion velocity: trade matches plus cancellations ahead
        let cancel_share = if v_total > 1e-6 {
            (v_ahead / v_total).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let mu = (match_rate + cancel_rate * cancel_share).max(1e-4);
        let sigma = diffusion_sigma.max(0.1);

        let sqrt_tau = horizon_sec.sqrt();
        let sigma_sqrt_tau = sigma * sqrt_tau;

        let d1 = (mu * horizon_sec - threshold) / sigma_sqrt_tau;
        let d2 = (-mu * horizon_sec - threshold) / sigma_sqrt_tau;

        let phi1 = crate::autotune::norm_cdf(d1).unwrap_or(0.5);
        let phi2 = crate::autotune::norm_cdf(d2).unwrap_or(0.0);

        let exp_arg = (2.0 * mu * threshold) / (sigma * sigma);
        let exp_term = if exp_arg > 60.0 { 0.0 } else { exp_arg.exp() };

        let p_fill = phi1 - exp_term * phi2;
        Some(p_fill.clamp(0.0, 1.0))
    }

    /// Read-only slice of orders in FIFO priority.
    pub fn orders(&self) -> &VecDeque<L3OrderEntry> {
        &self.orders
    }

    /// Evaluates the queue drain hazard for a specific resting order using the L3 hazard forecaster.
    pub fn evaluate_drain_hazard(
        &self,
        order_id: &str,
        forecaster: &crate::l3_hazard::QueueDrainHazardForecaster,
        is_toxic_flow: bool,
    ) -> Option<(
        crate::l3_hazard::QueueHazardDecision,
        crate::l3_hazard::QueueDrainHazardMetrics,
    )> {
        let pos = self.queue_position_of(order_id)?;
        Some(forecaster.evaluate_hazard(pos.volume_ahead, pos.total_level_volume, is_toxic_flow))
    }
}

/// Discrete event types reported on L3 feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderEventType {
    Add,
    Cancel,
    Match,
    Replace,
}

/// Incoming discrete L3 event from exchange direct feed or gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3OrderEvent {
    pub event_id: u64,
    pub timestamp_ns: u64,
    pub order_id: String,
    pub is_bid: bool,
    pub price: Decimal,
    pub size: Decimal,
    pub event_type: OrderEventType,
    pub new_price: Option<Decimal>,
    pub new_size: Option<Decimal>,
}

/// Outcome of applying an L3 event to the book.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum L3EventResult {
    OrderAdded {
        price: Decimal,
        size: Decimal,
        rank: usize,
    },
    OrderCancelled {
        price: Decimal,
        cancelled_size: Decimal,
        remaining_size: Decimal,
    },
    OrderMatched {
        price: Decimal,
        matched_fills: Vec<L3MatchFill>,
    },
    OrderReplaced {
        old_price: Decimal,
        new_price: Decimal,
        new_size: Decimal,
        preserved_priority: bool,
    },
    Ignored(String),
}

/// High-performance Level-3 Order-by-Order Reconstructed Book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3OrderBook {
    pub symbol: String,
    /// Bids sorted ascending by price (highest bid is bids.iter().next_back())
    bids: BTreeMap<Decimal, L3PriceQueue>,
    /// Asks sorted ascending by price (lowest ask is asks.iter().next())
    asks: BTreeMap<Decimal, L3PriceQueue>,
    /// Fast locator: order_id -> (is_bid, price)
    order_index: HashMap<String, (bool, Decimal)>,
    pub sequence: u64,
}

impl L3OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            order_index: HashMap::new(),
            sequence: 0,
        }
    }

    /// Best resting bid (Price, Total Size).
    pub fn best_bid(&self) -> Option<(Decimal, Decimal)> {
        self.bids
            .iter()
            .next_back()
            .map(|(px, q)| (*px, q.total_volume))
    }

    /// Best resting ask (Price, Total Size).
    pub fn best_ask(&self) -> Option<(Decimal, Decimal)> {
        self.asks.iter().next().map(|(px, q)| (*px, q.total_volume))
    }

    /// Mid price between best bid and best ask.
    pub fn mid_price(&self) -> Option<Decimal> {
        let (bb, _) = self.best_bid()?;
        let (ba, _) = self.best_ask()?;
        Some((bb + ba) / dec!(2))
    }

    /// Top of book spread in basis points: $\frac{P_{\text{ask}} - P_{\text{bid}}}{P_{\text{mid}}} \times 10^4$.
    pub fn spread_bps(&self) -> Option<Decimal> {
        let (bb, _) = self.best_bid()?;
        let (ba, _) = self.best_ask()?;
        let mid = (bb + ba) / dec!(2);
        if mid <= dec!(0) {
            return None;
        }
        Some(((ba - bb) / mid) * dec!(10000))
    }

    /// Volume-weighted Micro-Price:
    ///
    /// $$P_{\text{micro}} = \frac{V_{\text{bid}} P_{\text{ask}} + V_{\text{ask}} P_{\text{bid}}}{V_{\text{bid}} + V_{\text{ask}}}$$
    pub fn micro_price(&self) -> Option<Decimal> {
        let (bb, vb) = self.best_bid()?;
        let (ba, va) = self.best_ask()?;
        let total_vol = vb + va;
        if total_vol <= dec!(0) {
            return None;
        }
        Some((vb * ba + va * bb) / total_vol)
    }

    /// Looks up an active order's exact FIFO priority queue position across the entire book.
    pub fn find_order_position(&self, order_id: &str) -> Option<QueuePosition> {
        let &(is_bid, price) = self.order_index.get(order_id)?;
        if is_bid {
            self.bids.get(&price)?.queue_position_of(order_id)
        } else {
            self.asks.get(&price)?.queue_position_of(order_id)
        }
    }

    /// Applies an atomic discrete L3 event to the order book.
    pub fn apply_event(&mut self, event: L3OrderEvent) -> Result<L3EventResult, String> {
        self.sequence += 1;
        match event.event_type {
            OrderEventType::Add => {
                if event.size <= dec!(0) {
                    return Ok(L3EventResult::Ignored("zero or negative size".into()));
                }
                let entry = L3OrderEntry::new(
                    event.order_id.clone(),
                    event.price,
                    event.size,
                    event.timestamp_ns,
                );

                let rank = if event.is_bid {
                    let queue = self
                        .bids
                        .entry(event.price)
                        .or_insert_with(|| L3PriceQueue::new(event.price));
                    queue.add_order(entry);
                    queue.order_count()
                } else {
                    let queue = self
                        .asks
                        .entry(event.price)
                        .or_insert_with(|| L3PriceQueue::new(event.price));
                    queue.add_order(entry);
                    queue.order_count()
                };

                self.order_index
                    .insert(event.order_id, (event.is_bid, event.price));
                Ok(L3EventResult::OrderAdded {
                    price: event.price,
                    size: event.size,
                    rank,
                })
            }
            OrderEventType::Cancel => {
                let (is_bid, price) = match self.order_index.get(&event.order_id) {
                    Some(&(b, p)) => (b, p),
                    None => {
                        // Fall back to event parameters if unindexed
                        (event.is_bid, event.price)
                    }
                };

                let target_book = if is_bid {
                    &mut self.bids
                } else {
                    &mut self.asks
                };
                let (removed, remaining) = if let Some(queue) = target_book.get_mut(&price) {
                    let cancel_size = if event.size > dec!(0) {
                        Some(event.size)
                    } else {
                        None
                    };
                    let rem_qty = queue.cancel_order(&event.order_id, cancel_size);
                    let left = queue
                        .queue_position_of(&event.order_id)
                        .map(|p| p.order_size)
                        .unwrap_or(dec!(0));
                    if queue.is_empty() {
                        target_book.remove(&price);
                    }
                    (rem_qty.unwrap_or(dec!(0)), left)
                } else {
                    (dec!(0), dec!(0))
                };

                if remaining == dec!(0) {
                    self.order_index.remove(&event.order_id);
                }

                Ok(L3EventResult::OrderCancelled {
                    price,
                    cancelled_size: removed,
                    remaining_size: remaining,
                })
            }
            OrderEventType::Match => {
                let target_book = if event.is_bid {
                    &mut self.bids
                } else {
                    &mut self.asks
                };
                if let Some(queue) = target_book.get_mut(&event.price) {
                    let fills = queue.match_front(event.size);
                    for f in &fills {
                        if f.remaining_order_size == dec!(0) {
                            self.order_index.remove(&f.order_id);
                        }
                    }
                    if queue.is_empty() {
                        target_book.remove(&event.price);
                    }
                    Ok(L3EventResult::OrderMatched {
                        price: event.price,
                        matched_fills: fills,
                    })
                } else {
                    Ok(L3EventResult::Ignored(
                        "matched price level not found".into(),
                    ))
                }
            }
            OrderEventType::Replace => {
                let new_px = event.new_price.unwrap_or(event.price);
                let new_sz = event.new_size.unwrap_or(event.size);

                let (is_bid, old_px) = match self.order_index.get(&event.order_id) {
                    Some(&(b, p)) => (b, p),
                    None => return Err(format!("order {} not found for replace", event.order_id)),
                };

                if new_px == old_px {
                    // Same price: check if size reduced (preserves priority) or increased (loses priority)
                    let queue = if is_bid {
                        self.bids.get_mut(&old_px).ok_or("queue missing")?
                    } else {
                        self.asks.get_mut(&old_px).ok_or("queue missing")?
                    };

                    let was_requeued =
                        queue.replace_order(&event.order_id, new_sz, event.timestamp_ns)?;
                    let preserved = was_requeued.is_none();
                    Ok(L3EventResult::OrderReplaced {
                        old_price: old_px,
                        new_price: new_px,
                        new_size: new_sz,
                        preserved_priority: preserved,
                    })
                } else {
                    // Price changed: cancel from old queue and insert into new queue (priority lost)
                    let old_target = if is_bid {
                        &mut self.bids
                    } else {
                        &mut self.asks
                    };
                    if let Some(q) = old_target.get_mut(&old_px) {
                        q.cancel_order(&event.order_id, None);
                        if q.is_empty() {
                            old_target.remove(&old_px);
                        }
                    }

                    let new_entry = L3OrderEntry::new(
                        event.order_id.clone(),
                        new_px,
                        new_sz,
                        event.timestamp_ns,
                    );

                    let new_target = if is_bid {
                        &mut self.bids
                    } else {
                        &mut self.asks
                    };
                    let queue = new_target
                        .entry(new_px)
                        .or_insert_with(|| L3PriceQueue::new(new_px));
                    queue.add_order(new_entry);

                    self.order_index.insert(event.order_id, (is_bid, new_px));
                    Ok(L3EventResult::OrderReplaced {
                        old_price: old_px,
                        new_price: new_px,
                        new_size: new_sz,
                        preserved_priority: false,
                    })
                }
            }
        }
    }

    /// Converts the dynamic L3 order book into a standard Level-2 `OrderBook` with aggregated depth.
    pub fn to_l2_orderbook(&self, venue: VenueId, max_depth: usize) -> OrderBook {
        let mut book = OrderBook::new(venue, self.symbol.clone());
        book.sequence = self.sequence;
        book.timestamp = Utc::now();

        // Bids sorted descending: highest price first
        for (price, queue) in self.bids.iter().rev().take(max_depth) {
            if queue.total_volume > dec!(0) {
                book.bids.push(BookLevel::new(*price, queue.total_volume));
            }
        }

        // Asks sorted ascending: lowest price first
        for (price, queue) in self.asks.iter().take(max_depth) {
            if queue.total_volume > dec!(0) {
                book.asks.push(BookLevel::new(*price, queue.total_volume));
            }
        }

        book
    }

    /// Total cumulative bid depth in USD across top $N$ price levels.
    pub fn bid_depth_usd(&self, max_levels: usize) -> Decimal {
        self.bids
            .iter()
            .rev()
            .take(max_levels)
            .map(|(px, q)| *px * q.total_volume)
            .sum()
    }

    /// Total cumulative ask depth in USD across top $N$ price levels.
    pub fn ask_depth_usd(&self, max_levels: usize) -> Decimal {
        self.asks
            .iter()
            .take(max_levels)
            .map(|(px, q)| *px * q.total_volume)
            .sum()
    }

    /// Evaluates queue drain hazard for an active resting order across either bids or asks.
    pub fn evaluate_order_drain_hazard(
        &self,
        order_id: &str,
        forecaster: &crate::l3_hazard::QueueDrainHazardForecaster,
        is_toxic_flow: bool,
    ) -> Option<(
        crate::l3_hazard::QueueHazardDecision,
        crate::l3_hazard::QueueDrainHazardMetrics,
    )> {
        for queue in self.bids.values() {
            if let Some(res) = queue.evaluate_drain_hazard(order_id, forecaster, is_toxic_flow) {
                return Some(res);
            }
        }
        for queue in self.asks.values() {
            if let Some(res) = queue.evaluate_drain_hazard(order_id, forecaster, is_toxic_flow) {
                return Some(res);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_l3_price_queue_fifo_matching() {
        let mut queue = L3PriceQueue::new(dec!(100.0));
        queue.add_order(L3OrderEntry::new(
            "order1".into(),
            dec!(100.0),
            dec!(2.0),
            100,
        ));
        queue.add_order(L3OrderEntry::new(
            "order2".into(),
            dec!(100.0),
            dec!(3.0),
            101,
        ));
        queue.add_order(L3OrderEntry::new(
            "order3".into(),
            dec!(100.0),
            dec!(5.0),
            102,
        ));

        assert_eq!(queue.total_volume, dec!(10.0));
        assert_eq!(queue.order_count(), 3);

        // Check queue position of order3: rank 3, volume ahead = 2.0 + 3.0 = 5.0
        let pos3 = queue.queue_position_of("order3").unwrap();
        assert_eq!(pos3.rank, 3);
        assert_eq!(pos3.volume_ahead, dec!(5.0));
        assert_eq!(pos3.order_size, dec!(5.0));
        assert_eq!(pos3.total_level_volume, dec!(10.0));
        assert!((pos3.queue_share - 0.50).abs() < 1e-4);

        // Match 4.0 from front: fully consumes order1 (2.0) and partially consumes order2 (2.0 of 3.0)
        let fills = queue.match_front(dec!(4.0));
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].order_id, "order1");
        assert_eq!(fills[0].fill_size, dec!(2.0));
        assert_eq!(fills[0].remaining_order_size, dec!(0));

        assert_eq!(fills[1].order_id, "order2");
        assert_eq!(fills[1].fill_size, dec!(2.0));
        assert_eq!(fills[1].remaining_order_size, dec!(1.0));

        assert_eq!(queue.total_volume, dec!(6.0));
        assert_eq!(queue.order_count(), 2);

        // Order 3 is now rank 2, volume ahead = 1.0 (from order2)
        let pos3_after = queue.queue_position_of("order3").unwrap();
        assert_eq!(pos3_after.rank, 2);
        assert_eq!(pos3_after.volume_ahead, dec!(1.0));
    }

    #[test]
    fn test_l3_replace_order_priority_rules() {
        let mut queue = L3PriceQueue::new(dec!(100.0));
        queue.add_order(L3OrderEntry::new(
            "order1".into(),
            dec!(100.0),
            dec!(5.0),
            100,
        ));
        queue.add_order(L3OrderEntry::new(
            "order2".into(),
            dec!(100.0),
            dec!(5.0),
            101,
        ));

        // Size decrease: order1 goes from 5.0 to 3.0 -> keeps rank 1!
        let res = queue.replace_order("order1", dec!(3.0), 105).unwrap();
        assert!(res.is_none(), "size decrease should preserve priority");
        assert_eq!(queue.orders().front().unwrap().order_id, "order1");
        assert_eq!(queue.total_volume, dec!(8.0));

        // Size increase: order1 goes from 3.0 to 10.0 -> loses priority, moved to back!
        let res = queue.replace_order("order1", dec!(10.0), 110).unwrap();
        assert!(res.is_some(), "size increase should lose priority");
        assert_eq!(queue.orders().front().unwrap().order_id, "order2");
        assert_eq!(queue.orders().back().unwrap().order_id, "order1");
        assert_eq!(queue.total_volume, dec!(15.0));
    }

    #[test]
    fn test_l3_orderbook_full_lifecycle_and_l2_snapshot() {
        let mut book = L3OrderBook::new("BTC-USDC".into());

        // Add 2 bids
        book.apply_event(L3OrderEvent {
            event_id: 1,
            timestamp_ns: 1000,
            order_id: "bid1".into(),
            is_bid: true,
            price: dec!(60000.0),
            size: dec!(1.5),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        })
        .unwrap();

        book.apply_event(L3OrderEvent {
            event_id: 2,
            timestamp_ns: 1001,
            order_id: "bid2".into(),
            is_bid: true,
            price: dec!(59990.0),
            size: dec!(2.0),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        })
        .unwrap();

        // Add 2 asks
        book.apply_event(L3OrderEvent {
            event_id: 3,
            timestamp_ns: 1002,
            order_id: "ask1".into(),
            is_bid: false,
            price: dec!(60010.0),
            size: dec!(0.5),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        })
        .unwrap();

        book.apply_event(L3OrderEvent {
            event_id: 4,
            timestamp_ns: 1003,
            order_id: "ask2".into(),
            is_bid: false,
            price: dec!(60020.0),
            size: dec!(1.0),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        })
        .unwrap();

        assert_eq!(book.best_bid(), Some((dec!(60000.0), dec!(1.5))));
        assert_eq!(book.best_ask(), Some((dec!(60010.0), dec!(0.5))));
        assert_eq!(book.mid_price(), Some(dec!(60005.0)));

        // Micro-price calculation:
        // V_bid = 1.5, P_ask = 60010; V_ask = 0.5, P_bid = 60000
        // (1.5 * 60010 + 0.5 * 60000) / 2.0 = (90015 + 30000) / 2.0 = 60007.5
        assert_eq!(book.micro_price(), Some(dec!(60007.5)));

        // Convert to L2 OrderBook
        let l2 = book.to_l2_orderbook(VenueId::new("binance"), 5);
        assert_eq!(l2.bids.len(), 2);
        assert_eq!(l2.asks.len(), 2);
        assert_eq!(l2.bids[0].price, dec!(60000.0));
        assert_eq!(l2.bids[0].size, dec!(1.5));
        assert_eq!(l2.asks[0].price, dec!(60010.0));
        assert_eq!(l2.asks[0].size, dec!(0.5));
    }

    #[test]
    fn test_estimate_fill_probability() {
        let mut queue = L3PriceQueue::new(dec!(50.0));
        queue.add_order(L3OrderEntry::new("lead".into(), dec!(50.0), dec!(10.0), 10));
        queue.add_order(L3OrderEntry::new(
            "our_order".into(),
            dec!(50.0),
            dec!(5.0),
            20,
        ));

        let p_short = queue
            .estimate_fill_probability("our_order", 0.5, 2.0, 1.0, 1.5)
            .unwrap();
        let p_long = queue
            .estimate_fill_probability("our_order", 10.0, 2.0, 1.0, 1.5)
            .unwrap();

        assert!(p_long > p_short, "longer horizon yields higher fill prob");
        assert!((0.0..=1.0).contains(&p_short));
        assert!((0.0..=1.0).contains(&p_long));
    }

    #[test]
    fn test_l3_orderbook_evaluate_order_drain_hazard() {
        let mut book = L3OrderBook::new("BTC-USDC".to_string());
        let _ = book.apply_event(L3OrderEvent {
            event_id: 1,
            timestamp_ns: 100,
            order_id: "lead_bid".into(),
            is_bid: true,
            price: dec!(60000.0),
            size: dec!(5.0),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        });
        let _ = book.apply_event(L3OrderEvent {
            event_id: 2,
            timestamp_ns: 110,
            order_id: "our_bid".into(),
            is_bid: true,
            price: dec!(60000.0),
            size: dec!(1.0),
            event_type: OrderEventType::Add,
            new_price: None,
            new_size: None,
        });

        let forecaster = crate::l3_hazard::QueueDrainHazardForecaster::default();
        let (decision, metrics) = book
            .evaluate_order_drain_hazard("our_bid", &forecaster, false)
            .expect("order exists");

        assert_eq!(metrics.volume_ahead, dec!(5.0));
        assert!(matches!(
            decision,
            crate::l3_hazard::QueueHazardDecision::SafeHold { .. }
        ));
    }
}
