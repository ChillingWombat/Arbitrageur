use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CycleStatus {
    Open,
    Closed,
    StoppedOut,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageCycleLot {
    pub id: String,
    pub strategy_id: String,
    pub symbol: String,
    /// Trading environment the cycle ran under (`"testnet"`/`"live"`;
    /// `"unknown"` for pre-v33 persisted lots). Metrics and exposure
    /// aggregation filter on this — a lot is only attributed to the
    /// environment that created it, never blended across.
    #[serde(default = "default_unknown_environment")]
    pub environment: String,
    pub venue_long: String,
    pub venue_short: String,
    pub entry_time: DateTime<Utc>,
    pub entry_spread_bps: f64,
    pub long_qty: f64,
    pub long_entry_price: f64,
    pub short_qty: f64,
    pub short_entry_price: f64,
    pub fees_paid_usd: f64,
    pub status: CycleStatus,
    pub close_time: Option<DateTime<Utc>>,
    pub close_spread_bps: Option<f64>,
    pub realized_pnl_usd: Option<f64>,
    pub realized_pnl_bps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyMetrics {
    pub strategy_id: String,
    pub total_cycles_initiated: usize,
    pub total_cycles_completed: usize,
    pub winning_cycles: usize,
    pub losing_cycles: usize,
    pub win_rate_pct: f64,
    pub total_pnl_usd: f64,
    pub avg_pnl_usd: f64,
    pub avg_pnl_bps: f64,
    pub max_drawdown_usd: f64,
    pub max_drawdown_pct: f64,
    pub sharpe_ratio: f64,
    pub annualized_apr_pct: f64,
    pub annualized_apy_pct: f64,
}

#[derive(Debug, Default)]
pub struct VirtualStrategyLedger {
    pub lots: HashMap<String, ArbitrageCycleLot>,
    /// Secondary index mapping environment -> symbol -> venue -> open lot IDs for O(1) zero-allocation lookups
    open_lots_by_venue: HashMap<String, HashMap<String, HashMap<String, HashSet<String>>>>,
}

fn default_unknown_environment() -> String {
    "unknown".to_string()
}

impl VirtualStrategyLedger {
    pub fn new() -> Self {
        Self {
            lots: HashMap::new(),
            open_lots_by_venue: HashMap::new(),
        }
    }

    /// Rebuilds the secondary open lots index across all currently open lots.
    pub fn rebuild_open_index(&mut self) {
        self.open_lots_by_venue.clear();
        for lot in self.lots.values() {
            if lot.status == CycleStatus::Open {
                Self::insert_open_index(
                    &mut self.open_lots_by_venue,
                    &lot.environment,
                    &lot.symbol,
                    &lot.venue_long,
                    &lot.id,
                );
                Self::insert_open_index(
                    &mut self.open_lots_by_venue,
                    &lot.environment,
                    &lot.symbol,
                    &lot.venue_short,
                    &lot.id,
                );
            }
        }
    }

    /// Sets the entire lots table (e.g. during state hydration on startup) and re-indexes open lots.
    pub fn set_lots(&mut self, lots: HashMap<String, ArbitrageCycleLot>) {
        self.lots = lots;
        self.rebuild_open_index();
    }

    /// Retains lots matching the predicate and synchronizes the open lot index.
    pub fn retain_lots<F>(&mut self, mut f: F)
    where
        F: FnMut(&str, &ArbitrageCycleLot) -> bool,
    {
        self.lots.retain(|k, v| f(k, v));
        self.rebuild_open_index();
    }

    fn insert_open_index(
        index: &mut HashMap<String, HashMap<String, HashMap<String, HashSet<String>>>>,
        env: &str,
        sym: &str,
        venue: &str,
        id: &str,
    ) {
        index
            .entry(env.to_string())
            .or_default()
            .entry(sym.to_string())
            .or_default()
            .entry(venue.to_string())
            .or_default()
            .insert(id.to_string());
    }

    fn remove_open_index(
        index: &mut HashMap<String, HashMap<String, HashMap<String, HashSet<String>>>>,
        env: &str,
        sym: &str,
        venue: &str,
        id: &str,
    ) {
        if let Some(set) = index
            .get_mut(env)
            .and_then(|s| s.get_mut(sym))
            .and_then(|v| v.get_mut(venue))
        {
            set.remove(id);
        }
    }

    /// Open a new arbitrage cycle lot attributed to a specific strategy
    #[allow(clippy::too_many_arguments)]
    pub fn open_cycle(
        &mut self,
        id: String,
        strategy_id: String,
        symbol: String,
        venue_long: String,
        venue_short: String,
        entry_spread_bps: f64,
        long_qty: f64,
        long_entry_price: f64,
        short_qty: f64,
        short_entry_price: f64,
        entry_fees_usd: f64,
        environment: String,
    ) {
        let lot = ArbitrageCycleLot {
            id: id.clone(),
            strategy_id,
            symbol: symbol.clone(),
            environment: environment.clone(),
            venue_long: venue_long.clone(),
            venue_short: venue_short.clone(),
            entry_time: Utc::now(),
            entry_spread_bps,
            long_qty,
            long_entry_price,
            short_qty,
            short_entry_price,
            fees_paid_usd: entry_fees_usd,
            status: CycleStatus::Open,
            close_time: None,
            close_spread_bps: None,
            realized_pnl_usd: None,
            realized_pnl_bps: None,
        };
        Self::insert_open_index(
            &mut self.open_lots_by_venue,
            &environment,
            &symbol,
            &venue_long,
            &id,
        );
        Self::insert_open_index(
            &mut self.open_lots_by_venue,
            &environment,
            &symbol,
            &venue_short,
            &id,
        );
        self.lots.insert(id, lot);
    }

    /// Close an existing arbitrage cycle lot and calculate realized PnL in USD and BPS
    pub fn close_cycle(
        &mut self,
        id: &str,
        long_exit_price: f64,
        short_exit_price: f64,
        exit_fees_usd: f64,
        status: CycleStatus,
    ) -> Option<&ArbitrageCycleLot> {
        let lot = self.lots.get_mut(id)?;

        Self::remove_open_index(
            &mut self.open_lots_by_venue,
            &lot.environment,
            &lot.symbol,
            &lot.venue_long,
            id,
        );
        Self::remove_open_index(
            &mut self.open_lots_by_venue,
            &lot.environment,
            &lot.symbol,
            &lot.venue_short,
            id,
        );

        // Long leg PnL: (exit - entry) * qty
        let long_pnl = (long_exit_price - lot.long_entry_price) * lot.long_qty;
        // Short leg PnL: (entry - exit) * qty
        let short_pnl = (lot.short_entry_price - short_exit_price) * lot.short_qty;

        let total_fees = lot.fees_paid_usd + exit_fees_usd;
        let net_pnl_usd = long_pnl + short_pnl - total_fees;

        let total_notional =
            (lot.long_entry_price * lot.long_qty + lot.short_entry_price * lot.short_qty) / 2.0;
        let net_pnl_bps = if total_notional > 0.0 {
            (net_pnl_usd / total_notional) * 10000.0
        } else {
            0.0
        };

        lot.status = status;
        lot.close_time = Some(Utc::now());
        lot.fees_paid_usd = total_fees;
        lot.realized_pnl_usd = Some(net_pnl_usd);
        lot.realized_pnl_bps = Some(net_pnl_bps);

        let close_spread = if short_exit_price > 0.0 {
            ((short_exit_price - long_exit_price) / short_exit_price) * 10000.0
        } else {
            0.0
        };
        lot.close_spread_bps = Some(close_spread);

        Some(lot)
    }

    /// Close an existing arbitrage cycle lot directly with realized PnL in USD and BPS
    pub fn close_cycle_with_pnl(
        &mut self,
        id: &str,
        realized_pnl_usd: f64,
        realized_pnl_bps: f64,
        status: CycleStatus,
    ) -> Option<&ArbitrageCycleLot> {
        let lot = self.lots.get_mut(id)?;

        Self::remove_open_index(
            &mut self.open_lots_by_venue,
            &lot.environment,
            &lot.symbol,
            &lot.venue_long,
            id,
        );
        Self::remove_open_index(
            &mut self.open_lots_by_venue,
            &lot.environment,
            &lot.symbol,
            &lot.venue_short,
            id,
        );

        lot.status = status;
        lot.close_time = Some(Utc::now());
        lot.realized_pnl_usd = Some(realized_pnl_usd);
        lot.realized_pnl_bps = Some(realized_pnl_bps);
        Some(lot)
    }

    /// Computes the net physical orderbook exposure on a specific venue for a symbol.
    /// Aggregates across active/open strategy lots in `environment` in O(k) indexed time without allocations:
    /// Long positions on this venue add positive exposure, Short positions subtract exposure.
    /// Returns net_qty: positive means net long, negative means net short.
    pub fn compute_venue_net_exposure(&self, venue: &str, symbol: &str, environment: &str) -> f64 {
        let lot_ids_opt = self
            .open_lots_by_venue
            .get(environment)
            .and_then(|by_symbol| by_symbol.get(symbol))
            .and_then(|by_venue| by_venue.get(venue));

        let Some(lot_ids) = lot_ids_opt else {
            // Fail-closed safety guard: if index was unpopulated (e.g. unindexed direct lots insertion in tests),
            // fallback to direct scan so exposure is never falsely reported as 0.0.
            if self.open_lots_by_venue.is_empty() && !self.lots.is_empty() {
                let mut net_qty = 0.0;
                for lot in self.lots.values() {
                    if lot.status == CycleStatus::Open
                        && lot.symbol == symbol
                        && lot.environment == environment
                    {
                        if lot.venue_long == venue {
                            net_qty += lot.long_qty;
                        }
                        if lot.venue_short == venue {
                            net_qty -= lot.short_qty;
                        }
                    }
                }
                return net_qty;
            }
            return 0.0;
        };

        let mut net_qty = 0.0;
        for id in lot_ids {
            if let Some(lot) = self.lots.get(id) {
                if lot.status == CycleStatus::Open
                    && lot.symbol == symbol
                    && lot.environment == environment
                {
                    if lot.venue_long == venue {
                        net_qty += lot.long_qty;
                    }
                    if lot.venue_short == venue {
                        net_qty -= lot.short_qty;
                    }
                }
            }
        }
        net_qty
    }

    /// Computes comprehensive strategy performance metrics:
    /// cycles completed, win rate, avg/total PnL (BPS & USD), Max Drawdown, Sharpe, and APR/APY.
    /// `environment` scopes the aggregation — lots only count toward the
    /// environment that created them; `"unknown"` legacy lots are excluded
    /// from every environment's metrics rather than guessed into one.
    pub fn compute_strategy_metrics(
        &self,
        strategy_id: &str,
        environment: &str,
    ) -> StrategyMetrics {
        let mut total_initiated = 0;
        let mut completed_lots = Vec::new();

        for lot in self.lots.values() {
            if lot.strategy_id == strategy_id && lot.environment == environment {
                total_initiated += 1;
                if lot.status != CycleStatus::Open && lot.realized_pnl_usd.is_some() {
                    completed_lots.push(lot);
                }
            }
        }

        // Sort by close_time ascending for drawdown calculation
        completed_lots.sort_by_key(|l| l.close_time.unwrap_or(l.entry_time));

        let completed_count = completed_lots.len();
        if completed_count == 0 {
            return StrategyMetrics {
                strategy_id: strategy_id.to_string(),
                total_cycles_initiated: total_initiated,
                ..Default::default()
            };
        }

        let mut winning = 0;
        let mut losing = 0;
        let mut total_usd = 0.0;
        let mut total_bps = 0.0;
        let mut pnl_series = Vec::with_capacity(completed_count);

        let mut cumulative_equity = 0.0;
        let mut peak_equity = 0.0;
        let mut max_dd_usd = 0.0;

        for lot in &completed_lots {
            let pnl_usd = lot.realized_pnl_usd.unwrap_or(0.0);
            let pnl_bps = lot.realized_pnl_bps.unwrap_or(0.0);

            if pnl_usd > 0.0 {
                winning += 1;
            } else if pnl_usd < 0.0 {
                losing += 1;
            }

            total_usd += pnl_usd;
            total_bps += pnl_bps;
            pnl_series.push(pnl_usd);

            cumulative_equity += pnl_usd;
            if cumulative_equity > peak_equity {
                peak_equity = cumulative_equity;
            }
            let dd = peak_equity - cumulative_equity;
            if dd > max_dd_usd {
                max_dd_usd = dd;
            }
        }

        let win_rate = if completed_count > 0 {
            (winning as f64 / completed_count as f64) * 100.0
        } else {
            0.0
        };

        let avg_usd = total_usd / completed_count as f64;
        let avg_bps = total_bps / completed_count as f64;

        // Sharpe ratio: mean / std_dev of trade PnL
        let mean = avg_usd;
        let variance: f64 = if completed_count > 1 {
            pnl_series.iter().map(|&p| (p - mean).powi(2)).sum::<f64>()
                / (completed_count - 1) as f64
        } else {
            0.0
        };
        let std_dev = variance.sqrt();
        let sharpe = if std_dev > 1e-6 {
            (mean / std_dev) * (252.0_f64).sqrt()
        } else if mean > 0.0 {
            10.0
        } else {
            0.0
        };

        let base_capital = if peak_equity > 1000.0 {
            peak_equity
        } else {
            10000.0
        };
        let max_dd_pct = (max_dd_usd / base_capital) * 100.0;

        let raw_return_rate = total_usd / base_capital;
        let apr_pct = raw_return_rate * 365.0 / 30.0 * 100.0;
        let apy_pct = ((1.0 + (raw_return_rate.max(-0.99) / 365.0)).powf(365.0) - 1.0) * 100.0;

        StrategyMetrics {
            strategy_id: strategy_id.to_string(),
            total_cycles_initiated: total_initiated,
            total_cycles_completed: completed_count,
            winning_cycles: winning,
            losing_cycles: losing,
            win_rate_pct: (win_rate * 100.0).round() / 100.0,
            total_pnl_usd: (total_usd * 100.0).round() / 100.0,
            avg_pnl_usd: (avg_usd * 100.0).round() / 100.0,
            avg_pnl_bps: (avg_bps * 100.0).round() / 100.0,
            max_drawdown_usd: (max_dd_usd * 100.0).round() / 100.0,
            max_drawdown_pct: (max_dd_pct * 100.0).round() / 100.0,
            sharpe_ratio: (sharpe * 100.0).round() / 100.0,
            annualized_apr_pct: (apr_pct * 100.0).round() / 100.0,
            annualized_apy_pct: (apy_pct * 100.0).round() / 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_ledger_cycle_and_netting() {
        let mut ledger = VirtualStrategyLedger::new();

        // Strategy A opens Long 1.0 BTC on Hyperliquid, Short 1.0 BTC on Lighter
        ledger.open_cycle(
            "lot-1".into(),
            "spatial_spread".into(),
            "BTC-USDC".into(),
            "Hyperliquid".into(),
            "Lighter".into(),
            20.0,
            1.0,
            60000.0,
            1.0,
            60120.0,
            5.0,
            "live".into(),
        );

        // Strategy B opens Long 0.4 BTC on Lighter, Short 0.4 BTC on Hyperliquid
        ledger.open_cycle(
            "lot-2".into(),
            "stat_arb".into(),
            "BTC-USDC".into(),
            "Lighter".into(),
            "Hyperliquid".into(),
            15.0,
            0.4,
            60050.0,
            0.4,
            60140.0,
            2.0,
            "live".into(),
        );

        // Verify physical orderbook net exposure:
        // Hyperliquid: +1.0 (from A) - 0.4 (from B) = +0.6 BTC
        let hl_net = ledger.compute_venue_net_exposure("Hyperliquid", "BTC-USDC", "live");
        assert!((hl_net - 0.6).abs() < 1e-5);

        // Lighter: -1.0 (from A) + 0.4 (from B) = -0.6 BTC
        let lgt_net = ledger.compute_venue_net_exposure("Lighter", "BTC-USDC", "live");
        assert!((lgt_net - (-0.6)).abs() < 1e-5);

        // Close lot-1 with profit:
        // Long exit: 60100 (+$100), Short exit: 60050 (+$70), Exit fees: $5
        // Net USD: +$165 ($170 gross - $10 total fees = $160)
        let closed = ledger.close_cycle("lot-1", 60100.0, 60050.0, 5.0, CycleStatus::Closed);
        assert!(closed.is_some());
        let lot1 = closed.unwrap();
        assert_eq!(lot1.status, CycleStatus::Closed);
        assert!((lot1.realized_pnl_usd.unwrap() - 160.0).abs() < 1e-5);

        // Now Hyperliquid net exposure is only from lot-2: -0.4 BTC
        let hl_net_after = ledger.compute_venue_net_exposure("Hyperliquid", "BTC-USDC", "live");
        assert!((hl_net_after - (-0.4)).abs() < 1e-5);

        // Compute metrics for spatial_spread
        let metrics = ledger.compute_strategy_metrics("spatial_spread", "live");
        assert_eq!(metrics.total_cycles_completed, 1);
        assert_eq!(metrics.winning_cycles, 1);
        assert_eq!(metrics.win_rate_pct, 100.0);
        assert!((metrics.total_pnl_usd - 160.0).abs() < 1e-2);

        // Test hydration via set_lots (rebuilding secondary index on app restart)
        let mut rehydrated = VirtualStrategyLedger::new();
        rehydrated.set_lots(ledger.lots.clone());
        let hl_rehydrated =
            rehydrated.compute_venue_net_exposure("Hyperliquid", "BTC-USDC", "live");
        assert_eq!(hl_rehydrated, hl_net_after);

        // Test retain_lots filtering
        rehydrated.retain_lots(|_, l| l.environment != "live");
        assert_eq!(
            rehydrated.compute_venue_net_exposure("Hyperliquid", "BTC-USDC", "live"),
            0.0
        );

        // Test unindexed fallback safety guard: direct insertion into .lots without index update
        let mut raw_ledger = VirtualStrategyLedger::new();
        raw_ledger.lots.insert(
            "lot-raw".into(),
            ArbitrageCycleLot {
                id: "lot-raw".into(),
                strategy_id: "test".into(),
                symbol: "ETH-USDC".into(),
                environment: "live".into(),
                venue_long: "Binance".into(),
                venue_short: "OKX".into(),
                entry_time: Utc::now(),
                entry_spread_bps: 10.0,
                long_qty: 2.5,
                long_entry_price: 3000.0,
                short_qty: 2.5,
                short_entry_price: 3010.0,
                fees_paid_usd: 1.0,
                status: CycleStatus::Open,
                close_time: None,
                close_spread_bps: None,
                realized_pnl_usd: None,
                realized_pnl_bps: None,
            },
        );
        // Even without index, fallback must find exposure correctly
        let binance_net = raw_ledger.compute_venue_net_exposure("Binance", "ETH-USDC", "live");
        assert_eq!(binance_net, 2.5);
    }
}
