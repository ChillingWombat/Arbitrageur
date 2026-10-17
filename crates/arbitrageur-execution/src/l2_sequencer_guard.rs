//! L2 Optimistic Sequencer Pre-Confirmation Latency & Re-Ordering Hazard Guard.
//!
//! Provides deterministic microstructural protection against Layer-2 sequencer stalls,
//! block boundary re-ordering races, and toxic MEV frontrunning on Arbitrum, Base, and Optimism.
//!
//! Enforces:
//! 1. Sequencer block cadence monitoring and stall detection ($\Delta t_{\text{stall}} > 3 T_{\text{block}}$).
//! 2. Pre-flight block cushion checks ($\Delta t_{\text{rem}} > \mu_{\text{prop}} + 2.5 \sigma_{\text{jitter}}$).
//! 3. Closed-form Pareto-optimal dynamic EIP-1559 priority tip escalation.
//! 4. Institutional transaction cost invariant: $C_{\text{gas}}(g^*) \le 0.35 \cdot \Pi_{\text{gross}}$.
//! 5. Speedup replacement calculation ($\ge 15\%$ tip bump) with nonce reuse.

use serde::{Deserialize, Serialize};

/// Error function approximation (Abramowitz and Stegun 7.1.26).
fn erf_approx(x: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let abs_x = x.abs();

    let t = 1.0 / (1.0 + p * abs_x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-abs_x * abs_x).exp();

    sign * y
}

/// Health status of an L2 optimistic sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SequencerHealth {
    /// Normal block cadence; turnaround times within nominal bounds.
    Healthy,
    /// Mild delay or queuing jitter ($1.5 T_{\text{block}} < \Delta t \le 3.0 T_{\text{block}}$).
    DegradedJitter,
    /// Sequencer feed stalled ($\Delta t > 3.0 T_{\text{block}}$); all new entries halted.
    SequencerStallAbort,
}

/// Recommendation result for dynamic EIP-1559 priority fee escalation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct L2PriorityTipRecommendation {
    pub priority_fee_gwei: f64,
    pub max_fee_per_gas_gwei: f64,
    pub total_gas_cost_usd: f64,
    pub inclusion_probability: f64,
    pub expected_net_pnl_usd: f64,
    pub gas_edge_share: f64,
    pub dispatch_allowed: bool,
}

/// Configuration parameters for an L2 chain sequencer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2SequencerGuardConfig {
    pub chain_id: u64,
    pub block_time_ms: f64,
    pub mean_propagation_ms: f64,
    pub std_jitter_ms: f64,
    pub max_stall_multiplier: f64,
    pub hazard_rate_per_ms: f64,
    pub min_priority_fee_gwei: f64,
    pub tip_dist_mu: f64,
    pub tip_dist_sigma: f64,
    pub max_gas_edge_ratio: f64,
}

impl L2SequencerGuardConfig {
    /// Arbitrum One (Nitro sequencer: 250ms micro-blocks, FIFO + time-boost).
    pub fn arbitrum_one() -> Self {
        Self {
            chain_id: 42161,
            block_time_ms: 250.0,
            mean_propagation_ms: 18.0,
            std_jitter_ms: 12.0,
            max_stall_multiplier: 3.0,
            hazard_rate_per_ms: 0.008,
            min_priority_fee_gwei: 0.05,
            tip_dist_mu: 1.20,
            tip_dist_sigma: 0.60,
            max_gas_edge_ratio: 0.35, // Institutional invariant: <= 35%
        }
    }

    /// Base (OP Stack: 2000ms block cadence, priority tip auction).
    pub fn base() -> Self {
        Self {
            chain_id: 8453,
            block_time_ms: 2000.0,
            mean_propagation_ms: 25.0,
            std_jitter_ms: 20.0,
            max_stall_multiplier: 3.0,
            hazard_rate_per_ms: 0.002,
            min_priority_fee_gwei: 0.005,
            tip_dist_mu: 0.80,
            tip_dist_sigma: 0.50,
            max_gas_edge_ratio: 0.35,
        }
    }
}

/// L2 Sequencer Latency & Pre-Confirmation Guard Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2SequencerLatencyGuard {
    pub config: L2SequencerGuardConfig,
    pub last_preconf_timestamp_ms: u64,
}

impl L2SequencerLatencyGuard {
    pub fn new(config: L2SequencerGuardConfig, now_ms: u64) -> Self {
        Self {
            config,
            last_preconf_timestamp_ms: now_ms,
        }
    }

    /// Records receipt of a pre-confirmed block or sequencer state root.
    pub fn record_block_receipt(&mut self, timestamp_ms: u64) {
        if timestamp_ms > self.last_preconf_timestamp_ms {
            self.last_preconf_timestamp_ms = timestamp_ms;
        }
    }

    /// Evaluates current health status of the sequencer.
    pub fn check_sequencer_health(&self, now_ms: u64) -> (SequencerHealth, f64) {
        let stall_dt_ms = if now_ms >= self.last_preconf_timestamp_ms {
            (now_ms - self.last_preconf_timestamp_ms) as f64
        } else {
            0.0
        };

        let t_block = self.config.block_time_ms;
        let health = if stall_dt_ms <= 1.5 * t_block {
            SequencerHealth::Healthy
        } else if stall_dt_ms <= self.config.max_stall_multiplier * t_block {
            SequencerHealth::DegradedJitter
        } else {
            SequencerHealth::SequencerStallAbort
        };

        (health, stall_dt_ms)
    }

    /// Evaluates pre-flight block cushion.
    /// Aborts entry if remaining block time $\Delta t_{\text{rem}} \le \mu_{\text{prop}} + 2.5 \sigma_{\text{jitter}}$.
    pub fn evaluate_preflight_block_cushion(&self, remaining_block_time_ms: f64) -> bool {
        let critical_cushion_ms = self.config.mean_propagation_ms + 2.5 * self.config.std_jitter_ms;
        remaining_block_time_ms > critical_cushion_ms
    }

    /// Evaluates inclusion probability $P(\text{Included} \mid \Delta t, g)$.
    pub fn preconf_inclusion_probability(&self, arrival_delay_ms: f64, tip_gwei: f64) -> f64 {
        if arrival_delay_ms >= self.config.block_time_ms {
            return 0.0;
        }

        let g = tip_gwei.max(self.config.min_priority_fee_gwei);
        let z = ((g / self.config.min_priority_fee_gwei).ln() - self.config.tip_dist_mu)
            / self.config.tip_dist_sigma;
        let phi_tip = 0.5 * (1.0 + erf_approx(z / std::f64::consts::SQRT_2));

        let time_decay = (-self.config.hazard_rate_per_ms * arrival_delay_ms.max(0.0)).exp();
        (phi_tip * time_decay).clamp(0.0, 1.0)
    }

    /// Computes total gas cost in USD.
    #[inline]
    pub fn compute_gas_cost_usd(
        gas_units: f64,
        effective_gas_price_gwei: f64,
        eth_price_usd: f64,
    ) -> f64 {
        gas_units * effective_gas_price_gwei * 1e-9 * eth_price_usd
    }

    /// Solves for Pareto-optimal EIP-1559 priority tip maximizing expected net PnL.
    pub fn optimize_priority_fee(
        &self,
        gross_profit_usd: f64,
        gas_units: f64,
        base_fee_gwei: f64,
        eth_price_usd: f64,
        arrival_delay_ms: f64,
        is_private_revert_protected: bool,
    ) -> L2PriorityTipRecommendation {
        if gross_profit_usd <= 0.0 || arrival_delay_ms >= self.config.block_time_ms {
            return L2PriorityTipRecommendation {
                priority_fee_gwei: self.config.min_priority_fee_gwei,
                max_fee_per_gas_gwei: base_fee_gwei * 1.25 + self.config.min_priority_fee_gwei,
                total_gas_cost_usd: 0.0,
                inclusion_probability: 0.0,
                expected_net_pnl_usd: 0.0,
                gas_edge_share: 0.0,
                dispatch_allowed: false,
            };
        }

        // Standing institutional ceiling: gas cost must never exceed 35% of gross profit
        let max_allowed_gas_usd = gross_profit_usd * self.config.max_gas_edge_ratio;
        let max_tip_gwei = ((max_allowed_gas_usd / (gas_units * 1e-9 * eth_price_usd))
            - base_fee_gwei)
            .max(self.config.min_priority_fee_gwei);

        let mut best_tip = self.config.min_priority_fee_gwei;
        let mut best_expected_pnl = -f64::INFINITY;
        let mut best_prob = 0.0;
        let mut best_cost_usd = 0.0;

        let steps = 50;
        let step_size = (max_tip_gwei - self.config.min_priority_fee_gwei) / (steps as f64);

        for step in 0..=steps {
            let tip = self.config.min_priority_fee_gwei + (step as f64) * step_size;
            let prob = self.preconf_inclusion_probability(arrival_delay_ms, tip);
            let cost_success =
                Self::compute_gas_cost_usd(gas_units, base_fee_gwei + tip, eth_price_usd);
            let cost_fail = if is_private_revert_protected {
                0.0
            } else {
                0.25 * cost_success
            };

            let expected_pnl = prob * (gross_profit_usd - cost_success) - (1.0 - prob) * cost_fail;

            if expected_pnl > best_expected_pnl {
                best_expected_pnl = expected_pnl;
                best_tip = tip;
                best_prob = prob;
                best_cost_usd = cost_success;
            }
        }

        let max_fee_per_gas = base_fee_gwei * 1.25 + best_tip;
        let gas_edge_share = best_cost_usd / gross_profit_usd;
        let dispatch_allowed =
            best_expected_pnl > 0.0 && gas_edge_share <= self.config.max_gas_edge_ratio;

        L2PriorityTipRecommendation {
            priority_fee_gwei: best_tip,
            max_fee_per_gas_gwei: max_fee_per_gas,
            total_gas_cost_usd: best_cost_usd,
            inclusion_probability: best_prob,
            expected_net_pnl_usd: best_expected_pnl,
            gas_edge_share,
            dispatch_allowed,
        }
    }

    /// Computes speedup replacement priority fee ($\ge 15\%$ bump) for stalled transactions.
    pub fn compute_speedup_replacement_tip(
        &self,
        old_priority_fee_gwei: f64,
        gross_profit_usd: f64,
        gas_units: f64,
        base_fee_gwei: f64,
        eth_price_usd: f64,
    ) -> L2PriorityTipRecommendation {
        let min_speedup_tip = (old_priority_fee_gwei * 1.15).max(old_priority_fee_gwei + 0.10);
        let mut rec = self.optimize_priority_fee(
            gross_profit_usd,
            gas_units,
            base_fee_gwei,
            eth_price_usd,
            0.0,
            true,
        );

        if rec.priority_fee_gwei < min_speedup_tip {
            rec.priority_fee_gwei = min_speedup_tip;
            rec.max_fee_per_gas_gwei = base_fee_gwei * 1.25 + min_speedup_tip;
            rec.total_gas_cost_usd = Self::compute_gas_cost_usd(
                gas_units,
                base_fee_gwei + min_speedup_tip,
                eth_price_usd,
            );
            rec.gas_edge_share = rec.total_gas_cost_usd / gross_profit_usd;
            rec.dispatch_allowed = rec.gas_edge_share <= self.config.max_gas_edge_ratio;
        }

        rec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequencer_health_stall_detection() {
        let config = L2SequencerGuardConfig::arbitrum_one(); // 250ms block
        let mut guard = L2SequencerLatencyGuard::new(config, 1_000_000);

        // Within 1.5 * 250ms = 375ms -> Healthy
        let (h1, _) = guard.check_sequencer_health(1_000_200);
        assert_eq!(h1, SequencerHealth::Healthy);

        // Between 375ms and 750ms -> DegradedJitter
        let (h2, _) = guard.check_sequencer_health(1_000_500);
        assert_eq!(h2, SequencerHealth::DegradedJitter);

        // Past 3 * 250ms = 750ms -> SequencerStallAbort
        let (h3, _) = guard.check_sequencer_health(1_001_000);
        assert_eq!(h3, SequencerHealth::SequencerStallAbort);

        // Update block receipt -> restores to Healthy
        guard.record_block_receipt(1_000_950);
        let (h4, _) = guard.check_sequencer_health(1_001_000);
        assert_eq!(h4, SequencerHealth::Healthy);
    }

    #[test]
    fn test_preflight_block_cushion() {
        let config = L2SequencerGuardConfig::arbitrum_one();
        // critical cushion = 18.0 + 2.5 * 12.0 = 48.0 ms
        let guard = L2SequencerLatencyGuard::new(config, 1_000_000);

        assert!(
            guard.evaluate_preflight_block_cushion(60.0),
            "60ms remaining > 48ms cushion -> allowed"
        );
        assert!(
            !guard.evaluate_preflight_block_cushion(35.0),
            "35ms remaining < 48ms cushion -> abort pre-flight"
        );
    }

    #[test]
    fn test_priority_fee_optimization_respects_35_percent_gas_ceiling() {
        let config = L2SequencerGuardConfig::arbitrum_one();
        let guard = L2SequencerLatencyGuard::new(config, 1_000_000);

        let gross_edge = 25.0; // $25 USD edge
        let gas_units = 180_000.0;
        let base_fee = 0.10; // Gwei
        let eth_price = 3_000.0;

        let rec =
            guard.optimize_priority_fee(gross_edge, gas_units, base_fee, eth_price, 10.0, true);
        assert!(rec.dispatch_allowed);
        assert!(
            rec.gas_edge_share <= 0.35,
            "Gas edge share must not exceed 35%: {}",
            rec.gas_edge_share
        );
        assert!(rec.expected_net_pnl_usd > 0.0);
    }

    #[test]
    fn test_speedup_replacement_bumps_priority_fee_at_least_15_percent() {
        let config = L2SequencerGuardConfig::arbitrum_one();
        let guard = L2SequencerLatencyGuard::new(config, 1_000_000);

        let old_tip = 1.0; // Gwei
        let gross_edge = 50.0;
        let gas_units = 150_000.0;
        let base_fee = 0.10;
        let eth_price = 3_000.0;

        let rec = guard
            .compute_speedup_replacement_tip(old_tip, gross_edge, gas_units, base_fee, eth_price);
        assert!(
            rec.priority_fee_gwei >= 1.15 * old_tip,
            "Speedup must bump tip by >= 15%: {} vs {}",
            rec.priority_fee_gwei,
            old_tip
        );
    }
}
