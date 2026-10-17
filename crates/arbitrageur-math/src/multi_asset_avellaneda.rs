//! Multi-Asset Avellaneda-Stoikov-Guéant (ASG) Optimal Inventory Reservation Engine.
//!
//! Extends classical market-making and spatial arbitrage inventory models to:
//! 1. Arbitrary $K$-dimensional portfolio inventory vectors $\mathbf{q} \in \mathbb{R}^K$.
//! 2. Full asset covariance matrix $\mathbf{\Sigma} \in \mathbb{R}^{K \times K}$ (cross-asset covariance coupling).
//! 3. Continuous perpetual funding carry drift $\mathbf{\Phi}_{\text{carry}}(\mathbf{q}, \tau)$.
//! 4. Non-linear interior log-barrier margin exhaustion penalties $\gamma_{\text{eff}}(U)$.
//! 5. Trade variance delta scoring ($\Delta \text{Var}$) for dynamic entry hurdle expansion/discounting.

use serde::{Deserialize, Serialize};

/// Recommendation for multi-asset quoting and inventory reservation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiAssetReservationQuote {
    pub reservation_prices: Vec<f64>,
    pub optimal_bids: Vec<f64>,
    pub optimal_asks: Vec<f64>,
    pub bid_half_spreads_bps: Vec<f64>,
    pub ask_half_spreads_bps: Vec<f64>,
    pub margin_utilization: f64,
    pub effective_risk_aversion: f64,
    pub quote_allowed_bid: Vec<bool>,
    pub quote_allowed_ask: Vec<bool>,
}

/// Evaluation result for a candidate trade on asset $i$.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CandidateTradeReservationEvaluation {
    pub asset_index: usize,
    pub trade_size: f64,
    pub variance_delta_usd: f64,
    pub is_risk_reducing_rebalance: bool,
    pub base_hurdle_bps: f64,
    pub adjusted_hurdle_bps: f64,
    pub post_trade_margin_utilization: f64,
    pub trade_allowed: bool,
}

/// Configuration parameters for the multi-asset Avellaneda-Stoikov-Guéant engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAssetAvellanedaConfig {
    /// Baseline CARA risk aversion parameter $\gamma > 0$.
    pub risk_aversion_gamma: f64,
    /// Margin barrier parameter $\theta_{\text{margin}}$.
    pub margin_barrier_theta: f64,
    /// Maximum allowable margin utilization before hard quote suppression (e.g. 0.85 = 85%).
    pub max_margin_utilization: f64,
    /// Minimum friction hurdle floor (bps).
    pub min_friction_floor_bps: f64,
    /// Funding carry scaling factor.
    pub funding_carry_weight: f64,
    /// Default order book liquidity parameter $\kappa_i$.
    pub default_liquidity_kappa: f64,
}

impl Default for MultiAssetAvellanedaConfig {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 1e-4,
            margin_barrier_theta: 0.25,
            max_margin_utilization: 0.85,
            min_friction_floor_bps: 2.0,
            funding_carry_weight: 1.0,
            default_liquidity_kappa: 1.50,
        }
    }
}

/// Multi-Asset Avellaneda-Stoikov-Guéant (ASG) Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAssetAvellanedaEngine {
    pub config: MultiAssetAvellanedaConfig,
    pub asset_symbols: Vec<String>,
    pub covariance_matrix: Vec<Vec<f64>>,
    pub mmr_ratios: Vec<f64>,
    pub liquidity_kappas: Vec<f64>,
}

impl MultiAssetAvellanedaEngine {
    /// Creates a new MultiAssetAvellanedaEngine.
    pub fn new(
        asset_symbols: Vec<String>,
        covariance_matrix: Vec<Vec<f64>>,
        mmr_ratios: Vec<f64>,
        config: MultiAssetAvellanedaConfig,
    ) -> Self {
        let n = asset_symbols.len();
        assert_eq!(
            covariance_matrix.len(),
            n,
            "Covariance rows must match asset count"
        );
        assert_eq!(mmr_ratios.len(), n, "MMR ratios must match asset count");

        let liquidity_kappas = vec![config.default_liquidity_kappa; n];
        Self {
            config,
            asset_symbols,
            covariance_matrix,
            mmr_ratios,
            liquidity_kappas,
        }
    }

    /// Evaluates current portfolio maintenance margin utilization $U(\mathbf{q}) \in [0, 1]$.
    pub fn margin_utilization(
        &self,
        inventory_q: &[f64],
        spot_prices: &[f64],
        available_equity_usd: f64,
    ) -> f64 {
        let equity = available_equity_usd.max(1.0);
        let mut total_mmr = 0.0;
        for i in 0..self.asset_symbols.len() {
            let q = if i < inventory_q.len() {
                inventory_q[i]
            } else {
                0.0
            };
            let s = if i < spot_prices.len() {
                spot_prices[i]
            } else {
                0.0
            };
            let mmr = if i < self.mmr_ratios.len() {
                self.mmr_ratios[i]
            } else {
                0.05
            };
            total_mmr += q.abs() * s * mmr;
        }
        (total_mmr / equity).clamp(0.0, 1.0)
    }

    /// Evaluates effective risk aversion $\gamma_{\text{eff}}$ under the interior margin barrier.
    pub fn effective_risk_aversion(&self, utilization: f64) -> f64 {
        let u = utilization.clamp(0.0, 0.999);
        self.config.risk_aversion_gamma * (1.0 + self.config.margin_barrier_theta / (1.0 - u))
    }

    /// Computes the portfolio marginal risk vector $\mathbf{\Sigma} \mathbf{q}$.
    #[allow(clippy::needless_range_loop)]
    pub fn compute_sigma_q(&self, inventory_q: &[f64]) -> Vec<f64> {
        let n = self.asset_symbols.len();
        let mut sigma_q = vec![0.0; n];
        for i in 0..n {
            for j in 0..n {
                let q_j = if j < inventory_q.len() {
                    inventory_q[j]
                } else {
                    0.0
                };
                sigma_q[i] += self.covariance_matrix[i][j] * q_j;
            }
        }
        sigma_q
    }

    /// Evaluates multi-asset reservation prices and optimal bid/ask quoting spreads.
    pub fn evaluate_reservation_quotes(
        &self,
        inventory_q: &[f64],
        spot_prices: &[f64],
        annualized_funding_rates: &[f64],
        horizon_sec: f64,
        available_equity_usd: f64,
    ) -> MultiAssetReservationQuote {
        let n = self.asset_symbols.len();
        let tau = horizon_sec.max(0.10);
        let u = self.margin_utilization(inventory_q, spot_prices, available_equity_usd);
        let gamma_eff = self.effective_risk_aversion(u);
        let sigma_q = self.compute_sigma_q(inventory_q);

        let mut reservation_prices = vec![0.0; n];
        let mut optimal_bids = vec![0.0; n];
        let mut optimal_asks = vec![0.0; n];
        let mut bid_half_spreads_bps = vec![0.0; n];
        let mut ask_half_spreads_bps = vec![0.0; n];
        let mut quote_allowed_bid = vec![true; n];
        let mut quote_allowed_ask = vec![true; n];

        for i in 0..n {
            let s_i = if i < spot_prices.len() {
                spot_prices[i]
            } else {
                1.0
            };
            let q_i = if i < inventory_q.len() {
                inventory_q[i]
            } else {
                0.0
            };
            let f_i = if i < annualized_funding_rates.len() {
                annualized_funding_rates[i]
            } else {
                0.0
            };
            let kappa_i = self.liquidity_kappas[i].max(0.10);

            // Perpetual funding carry drift: positive carry (earning) dampens inventory aversion
            let denom = 1.0 + gamma_eff * sigma_q[i].abs();
            let phi_carry_i =
                -(f_i * s_i * (tau / 86400.0) * self.config.funding_carry_weight) / denom;

            // Reservation price: s_i - gamma_eff * (Sigma * q)_i * tau + phi_carry_i
            let r_i = s_i - gamma_eff * sigma_q[i] * tau + phi_carry_i;
            reservation_prices[i] = r_i;

            let half_spread_base =
                (1.0 / kappa_i) * (1.0 + (gamma_eff / kappa_i)).ln().max(0.001 * s_i);
            let inv_skew = 0.5 * gamma_eff * sigma_q[i] * tau;

            let delta_a = (half_spread_base - inv_skew - phi_carry_i).max(0.0001 * s_i);
            let delta_b = (half_spread_base + inv_skew + phi_carry_i).max(0.0001 * s_i);

            let bid_px = s_i - delta_b;
            let ask_px = s_i + delta_a;

            optimal_bids[i] = bid_px;
            optimal_asks[i] = ask_px;
            bid_half_spreads_bps[i] = (delta_b / s_i) * 10_000.0;
            ask_half_spreads_bps[i] = (delta_a / s_i) * 10_000.0;

            // Hard margin quota gate
            if u >= self.config.max_margin_utilization {
                if q_i >= 0.0 {
                    quote_allowed_bid[i] = false; // Disallow expanding long
                }
                if q_i <= 0.0 {
                    quote_allowed_ask[i] = false; // Disallow expanding short
                }
            }
        }

        MultiAssetReservationQuote {
            reservation_prices,
            optimal_bids,
            optimal_asks,
            bid_half_spreads_bps,
            ask_half_spreads_bps,
            margin_utilization: u,
            effective_risk_aversion: gamma_eff,
            quote_allowed_bid,
            quote_allowed_ask,
        }
    }

    /// Evaluates a candidate trade $\Delta q_i$ on asset $i$, computing variance delta $\Delta \text{Var}$
    /// and dynamic hurdle adjustment.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_candidate_trade(
        &self,
        asset_index: usize,
        trade_size: f64, // signed: > 0 for Buy, < 0 for Sell
        base_hurdle_bps: f64,
        inventory_q: &[f64],
        spot_prices: &[f64],
        horizon_sec: f64,
        available_equity_usd: f64,
    ) -> CandidateTradeReservationEvaluation {
        let n = self.asset_symbols.len();
        if asset_index >= n {
            return CandidateTradeReservationEvaluation {
                asset_index,
                trade_size,
                variance_delta_usd: 0.0,
                is_risk_reducing_rebalance: false,
                base_hurdle_bps,
                adjusted_hurdle_bps: base_hurdle_bps,
                post_trade_margin_utilization: 1.0,
                trade_allowed: false,
            };
        }

        let tau = horizon_sec.max(0.10);
        let u_pre = self.margin_utilization(inventory_q, spot_prices, available_equity_usd);
        let gamma_eff = self.effective_risk_aversion(u_pre);

        // Pre-trade variance: q^T Sigma q
        let sigma_q = self.compute_sigma_q(inventory_q);
        let var_pre: f64 = inventory_q
            .iter()
            .zip(sigma_q.iter())
            .map(|(q, sq)| q * sq)
            .sum();

        // Post-trade inventory
        let mut post_q = inventory_q.to_vec();
        while post_q.len() < n {
            post_q.push(0.0);
        }
        post_q[asset_index] += trade_size;

        // Post-trade variance
        let post_sigma_q = self.compute_sigma_q(&post_q);
        let var_post: f64 = post_q
            .iter()
            .zip(post_sigma_q.iter())
            .map(|(q, sq)| q * sq)
            .sum();

        let variance_delta = var_post - var_pre;
        let is_risk_reducing = variance_delta < -1e-6;

        let u_post = self.margin_utilization(&post_q, spot_prices, available_equity_usd);

        // Compute hurdle adjustment
        let s_i = if asset_index < spot_prices.len() {
            spot_prices[asset_index]
        } else {
            1.0
        };
        let notional_usd = (trade_size.abs() * s_i).max(1.0);

        // Marginal penalty in bps: gamma_eff * Delta Var * tau scaled to notional
        let penalty_bps = (gamma_eff * variance_delta.abs() * tau / notional_usd) * 10_000.0;

        let adjusted_hurdle_bps = if is_risk_reducing {
            // Rebalancing discount down to floor
            (base_hurdle_bps - penalty_bps).max(self.config.min_friction_floor_bps)
        } else {
            // Risk expansion penalty
            base_hurdle_bps + penalty_bps
        };

        // Trade allowed check
        let trade_allowed = u_post <= self.config.max_margin_utilization || is_risk_reducing;

        CandidateTradeReservationEvaluation {
            asset_index,
            trade_size,
            variance_delta_usd: variance_delta,
            is_risk_reducing_rebalance: is_risk_reducing,
            base_hurdle_bps,
            adjusted_hurdle_bps,
            post_trade_margin_utilization: u_post,
            trade_allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_asg_engine() -> MultiAssetAvellanedaEngine {
        let assets = vec!["BTC".to_string(), "ETH".to_string()];
        // High positive correlation between BTC and ETH: Cov(BTC, ETH) > 0
        let cov = vec![
            vec![16.0, 12.0], // BTC var = 16.0, Cov(BTC, ETH) = 12.0
            vec![12.0, 25.0], // ETH var = 25.0
        ];
        let mmr = vec![0.05, 0.05]; // 5% MMR = 20x leverage cap

        let cfg = MultiAssetAvellanedaConfig {
            risk_aversion_gamma: 0.01,
            margin_barrier_theta: 0.20,
            max_margin_utilization: 0.85,
            min_friction_floor_bps: 2.0,
            funding_carry_weight: 1.0,
            default_liquidity_kappa: 1.5,
        };

        MultiAssetAvellanedaEngine::new(assets, cov, mmr, cfg)
    }

    #[test]
    fn test_cross_asset_inventory_coupling() {
        let engine = make_test_asg_engine();
        // Portfolio is long 10 BTC, 0 ETH
        let inv = vec![10.0, 0.0];
        let spots = vec![60_000.0, 3_000.0];
        let fundings = vec![0.0, 0.0];
        let equity = 500_000.0;

        let quotes = engine.evaluate_reservation_quotes(&inv, &spots, &fundings, 10.0, equity);

        // Because BTC is long (10.0) and Cov(BTC, ETH) = 12.0 > 0:
        // Even though ETH inventory is 0.0, ETH reservation price should be depressed (below spot)
        // to disincentivize buying more correlated ETH and incentivize selling ETH to hedge!
        let eth_res = quotes.reservation_prices[1];
        assert!(
            eth_res < spots[1],
            "ETH reservation price ({}) should be below spot ({}) due to long BTC inventory coupling!",
            eth_res,
            spots[1]
        );

        // And ETH ask spread should be tighter than bid spread to attract buyers
        assert!(
            quotes.ask_half_spreads_bps[1] < quotes.bid_half_spreads_bps[1],
            "ETH ask spread should be tighter than bid spread to offload correlated directional risk"
        );
    }

    #[test]
    fn test_candidate_trade_rebalance_vs_risk_expansion() {
        let engine = make_test_asg_engine();
        // Long 10 BTC, Long 20 ETH
        let inv = vec![10.0, 20.0];
        let spots = vec![60_000.0, 3_000.0];
        let equity = 1_000_000.0;

        // Candidate 1: Buy 2 more BTC (expands variance)
        let eval_expand = engine.evaluate_candidate_trade(0, 2.0, 10.0, &inv, &spots, 5.0, equity);
        assert!(!eval_expand.is_risk_reducing_rebalance);
        assert!(eval_expand.variance_delta_usd > 0.0);
        assert!(
            eval_expand.adjusted_hurdle_bps > eval_expand.base_hurdle_bps,
            "Expanding risk must increase hurdle above base: {} > 10.0",
            eval_expand.adjusted_hurdle_bps
        );

        // Candidate 2: Sell 5 BTC (rebalancing / risk-reducing)
        let eval_hedge = engine.evaluate_candidate_trade(0, -5.0, 10.0, &inv, &spots, 5.0, equity);
        assert!(eval_hedge.is_risk_reducing_rebalance);
        assert!(eval_hedge.variance_delta_usd < 0.0);
        assert!(
            eval_hedge.adjusted_hurdle_bps < eval_hedge.base_hurdle_bps,
            "Rebalancing must discount hurdle below base: {} < 10.0",
            eval_hedge.adjusted_hurdle_bps
        );
    }

    #[test]
    fn test_margin_barrier_shutoff_on_limit_breach() {
        let engine = make_test_asg_engine();
        // Extreme inventory: 15 BTC at $60,000 = $900k notional -> MMR = $45k
        // On $50k equity, utilization = 45k / 50k = 90% (>= 85% limit)
        let inv = vec![15.0, 0.0];
        let spots = vec![60_000.0, 3_000.0];
        let fundings = vec![0.0, 0.0];
        let equity = 50_000.0;

        let quotes = engine.evaluate_reservation_quotes(&inv, &spots, &fundings, 1.0, equity);
        assert!(quotes.margin_utilization >= 0.85);

        // Since inventory is long BTC (15.0), expanding bids must be disallowed!
        assert!(
            !quotes.quote_allowed_bid[0],
            "Bids must be disabled when margin >= 85% and inventory is long"
        );
        assert!(
            quotes.quote_allowed_ask[0],
            "Asks must remain enabled to permit unwinding long inventory"
        );
    }
}
