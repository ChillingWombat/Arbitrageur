//! Fast Basis Harvesting vs Passive Funding Drip.
//!
//! Evaluates whether an open funding rate carry or spot-perp basis trade should be
//! harvested early rather than held to the end of the 8-hour or 1-hour settlement epoch.
//!
//! When basis compresses rapidly (e.g. >= 80% compressed in the first 15-30 minutes),
//! capital turnover multiplier (IRR) can exceed passive hold returns by 10-15x.

#[derive(Debug, Clone, PartialEq)]
pub struct FastBasisHarvestEvaluator {
    /// Fraction of initial basis required to have converged (e.g. 0.80 for 80% compression).
    pub min_compression_ratio: f64,
    /// Minimum net profit in USD required after all trading friction/fees to trigger early exit.
    pub min_harvest_net_usd: f64,
    /// Fraction of epoch elapsed after which early harvest yields diminish (default 0.75).
    pub max_elapsed_epoch_fraction: f64,
}

impl Default for FastBasisHarvestEvaluator {
    fn default() -> Self {
        Self {
            min_compression_ratio: 0.80,
            min_harvest_net_usd: 1.0,
            max_elapsed_epoch_fraction: 0.75,
        }
    }
}

impl FastBasisHarvestEvaluator {
    pub fn new(
        min_compression_ratio: f64,
        min_harvest_net_usd: f64,
        max_elapsed_epoch_fraction: f64,
    ) -> Self {
        Self {
            min_compression_ratio: min_compression_ratio.clamp(0.50, 0.99),
            min_harvest_net_usd: min_harvest_net_usd.max(0.0),
            max_elapsed_epoch_fraction: max_elapsed_epoch_fraction.clamp(0.10, 0.95),
        }
    }

    /// Evaluates the continuous dynamic stopping boundary alpha*(tau) as a function of remaining epoch time tau.
    /// Early in the epoch (elapsed <= 0.25): strict min_compression_ratio (e.g. 0.80).
    /// Mid-epoch (0.25 < elapsed <= 0.75): relaxed down to 0.60 to maximize annualized capital velocity (IRR).
    /// Late epoch (elapsed > 0.75): 0.60 floor.
    pub fn dynamic_compression_boundary(&self, elapsed_fraction: f64) -> f64 {
        if elapsed_fraction <= 0.25 {
            self.min_compression_ratio
        } else if elapsed_fraction <= 0.75 {
            let progress = (elapsed_fraction - 0.25) / 0.50;
            self.min_compression_ratio - progress * (self.min_compression_ratio - 0.60)
        } else {
            0.60
        }
    }

    /// Determines whether the position has captured its basis compression edge early
    /// and should be closed to free capital.
    #[allow(clippy::too_many_arguments)]
    pub fn should_harvest_early(
        &self,
        entry_basis_bps: f64,
        current_basis_bps: f64,
        elapsed_hours: f64,
        total_epoch_hours: f64,
        accrued_funding_usd: f64,
        unrealized_basis_pnl_usd: f64,
        round_trip_fee_usd: f64,
    ) -> bool {
        if entry_basis_bps <= 0.0 || elapsed_hours <= 0.0 || total_epoch_hours <= 0.0 {
            return false;
        }

        let net_pnl_usd = unrealized_basis_pnl_usd + accrued_funding_usd - round_trip_fee_usd;
        if net_pnl_usd < self.min_harvest_net_usd {
            return false;
        }

        let elapsed_fraction = elapsed_hours / total_epoch_hours;
        if elapsed_fraction > self.max_elapsed_epoch_fraction {
            return false;
        }

        let compression = (entry_basis_bps - current_basis_bps) / entry_basis_bps;
        let boundary = self.dynamic_compression_boundary(elapsed_fraction);
        compression >= boundary
    }

    /// Evaluates whether to harvest early taking into account upcoming heterogeneous funding payment timestamps
    /// (e.g. 1h settlement on Hyperliquid vs 8h settlement on Lighter).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_asymmetric_funding_harvest(
        &self,
        entry_basis_bps: f64,
        current_basis_bps: f64,
        next_settlement_secs_a: u64,
        next_settlement_secs_b: u64,
        upcoming_payment_usd_a: f64,
        upcoming_payment_usd_b: f64,
        net_pnl_usd: f64,
    ) -> AsymmetricHarvestDecision {
        if net_pnl_usd < self.min_harvest_net_usd || entry_basis_bps <= 0.0 {
            return AsymmetricHarvestDecision::Hold;
        }

        let compression = (entry_basis_bps - current_basis_bps) / entry_basis_bps;
        if compression < 0.60 {
            return AsymmetricHarvestDecision::Hold;
        }

        let (imminent_secs, imminent_payment) = if next_settlement_secs_a <= next_settlement_secs_b
        {
            (next_settlement_secs_a, upcoming_payment_usd_a)
        } else {
            (next_settlement_secs_b, upcoming_payment_usd_b)
        };

        if imminent_payment < 0.0 && imminent_secs <= 180 {
            // Upcoming funding payment is a fee: escape before payment timestamp!
            AsymmetricHarvestDecision::AccelerateExitBeforeFee {
                seconds_remaining: imminent_secs,
            }
        } else if imminent_payment > 0.0 && imminent_secs <= 300 {
            // Upcoming payment is income: delay exit to collect cash!
            AsymmetricHarvestDecision::DelayForSettlement {
                delay_seconds: imminent_secs,
                expected_payment_usd: imminent_payment,
            }
        } else {
            AsymmetricHarvestDecision::HarvestImmediately
        }
    }

    /// Evaluates dynamic basis exit using Leung-Li (2015) double optimal stopping boundaries.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_leung_li_exit(
        &self,
        current_basis_bps: f64,
        mean_basis_theta_bps: f64,
        reversion_speed_kappa: f64,
        basis_volatility_sigma_bps: f64,
        borrow_rate_annual: f64,
        funding_rate_annual: f64,
        roundtrip_taker_fee_bps: f64,
        discount_rate_rho: Option<f64>,
    ) -> BasisExitDecision {
        let router = LeungLiBasisStoppingRouter::new(discount_rate_rho.unwrap_or(0.10));
        router.evaluate_basis_exit(
            current_basis_bps,
            mean_basis_theta_bps,
            reversion_speed_kappa,
            basis_volatility_sigma_bps,
            borrow_rate_annual,
            funding_rate_annual,
            roundtrip_taker_fee_bps,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AsymmetricHarvestDecision {
    HarvestImmediately,
    DelayForSettlement {
        delay_seconds: u64,
        expected_payment_usd: f64,
    },
    AccelerateExitBeforeFee {
        seconds_remaining: u64,
    },
    Hold,
}

/// Decision rendered by Leung-Li (2015) double optimal stopping basis exit router.
#[derive(Debug, Clone, PartialEq)]
pub enum BasisExitDecision {
    /// Spread remains within non-intervention continuation region (b_L, b_U). Hold position to capture positive carry.
    HoldCarryHarvest { net_carry_bps_per_day: f64 },
    /// Basis has compressed to or above b_U*: take profit immediately.
    TakeProfitConvergence {
        exit_spread_bps: f64,
        hurdle_b_u_bps: f64,
    },
    /// Basis has widened to or below b_L*: stop loss / roll basis to prevent severe carrying loss.
    StopLossRoll {
        exit_spread_bps: f64,
        hurdle_b_l_bps: f64,
    },
}

/// Leung-Li (2015) Double Optimal Stopping Basis Exit & Roll Engine.
#[derive(Debug, Clone, PartialEq)]
pub struct LeungLiBasisStoppingRouter {
    /// Subjective discount rate rho (annualized, default 0.10 = 10%).
    pub discount_rate_rho: f64,
}

impl Default for LeungLiBasisStoppingRouter {
    fn default() -> Self {
        Self {
            discount_rate_rho: 0.10,
        }
    }
}

impl LeungLiBasisStoppingRouter {
    pub fn new(discount_rate_rho: f64) -> Self {
        Self {
            discount_rate_rho: discount_rate_rho.clamp(0.01, 1.0),
        }
    }

    /// Evaluates optimal stopping boundaries b_L* and b_U* and produces an exit decision.
    ///
    /// # Arguments
    /// - `current_basis_bps`: Observed basis spread in basis points (e.g. 5.0 bps).
    /// - `mean_basis_theta_bps`: Long-run mean basis theta from OU fit (bps).
    /// - `reversion_speed_kappa`: OU mean-reversion speed kappa (annualized).
    /// - `basis_volatility_sigma_bps`: Annualized basis volatility sigma in bps.
    /// - `borrow_rate_annual`: Annual borrow cost (e.g. 0.06 = 6%).
    /// - `funding_rate_annual`: Annual funding rate received (e.g. 0.12 = 12%).
    /// - `roundtrip_taker_fee_bps`: Total round-trip exit fee in bps (e.g. 8.0 bps).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_basis_exit(
        &self,
        current_basis_bps: f64,
        mean_basis_theta_bps: f64,
        reversion_speed_kappa: f64,
        basis_volatility_sigma_bps: f64,
        borrow_rate_annual: f64,
        funding_rate_annual: f64,
        roundtrip_taker_fee_bps: f64,
    ) -> BasisExitDecision {
        let net_carry_rate = borrow_rate_annual - funding_rate_annual; // Positive = cost drag, Negative = net income
        let kappa = reversion_speed_kappa.max(0.1);
        let sigma = basis_volatility_sigma_bps.max(0.1);
        let rho = self.discount_rate_rho;

        // Effective carry cost drag converted to bps of spread hurdle:
        let carry_friction_bps = (net_carry_rate * 10_000.0 / rho).max(0.0);
        let eff_fee_upper = (roundtrip_taker_fee_bps + carry_friction_bps).max(0.1);
        let eff_fee_lower = roundtrip_taker_fee_bps.max(0.1);

        let scale = sigma * (2.0 / kappa).sqrt();
        let log_arg_upper = (sigma / ((2.0 * kappa).sqrt() * eff_fee_upper)).max(1.0001);
        let log_arg_lower = (sigma / ((2.0 * kappa).sqrt() * eff_fee_lower)).max(1.0001);

        let delta_u = scale * log_arg_upper.ln().sqrt();
        let delta_l = scale
            * log_arg_lower.ln().sqrt()
            * (1.0 + (net_carry_rate.abs() * 10_000.0) / (rho * sigma));

        let b_u_star = mean_basis_theta_bps + delta_u;
        let b_l_star = mean_basis_theta_bps - delta_l;

        if current_basis_bps >= b_u_star {
            BasisExitDecision::TakeProfitConvergence {
                exit_spread_bps: current_basis_bps,
                hurdle_b_u_bps: b_u_star,
            }
        } else if current_basis_bps <= b_l_star {
            BasisExitDecision::StopLossRoll {
                exit_spread_bps: current_basis_bps,
                hurdle_b_l_bps: b_l_star,
            }
        } else {
            let daily_carry_bps = -(net_carry_rate / 365.25) * 10_000.0;
            BasisExitDecision::HoldCarryHarvest {
                net_carry_bps_per_day: daily_carry_bps,
            }
        }
    }
}

/// Detailed decomposition of lead-lag price discovery across Spot, Perp CEX, and Perp DEX.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceDiscoveryReport {
    /// Normalized cointegrating vectors beta: [p_s - p_c, p_c - p_d]
    pub cointegrating_beta: [f64; 2],
    /// Speed of adjustment vector alpha: [alpha_spot, alpha_perp_cex, alpha_perp_dex]
    pub speed_of_adjustment_alpha: [f64; 3],
    /// Gonzalo-Granger (1995) permanent price discovery component shares (sum to 1.0)
    pub gonzalo_granger_shares: [f64; 3],
    /// Hasbrouck (1995) Information Shares midpoint (sum to 1.0)
    pub hasbrouck_info_shares: [f64; 3],
    /// Index of the dominant price discovery leader venue: 0 = Spot, 1 = Perp CEX, 2 = Perp DEX
    pub dominant_leader_idx: usize,
}

/// Cointegrated Vector Error Correction Model (VECM) for Tri-Venue Spot-Perp Basis Discovery.
#[derive(Debug, Clone)]
pub struct VecmBasisAnalyzer {
    pub min_samples: usize,
}

impl Default for VecmBasisAnalyzer {
    fn default() -> Self {
        Self { min_samples: 50 }
    }
}

impl VecmBasisAnalyzer {
    pub fn new(min_samples: usize) -> Self {
        Self {
            min_samples: min_samples.max(35),
        }
    }

    /// Evaluates high-frequency tri-venue prices and extracts Gonzalo-Granger and Hasbrouck price discovery shares.
    pub fn analyze_price_discovery(
        &self,
        spot_prices: &[f64],
        perp_cex_prices: &[f64],
        perp_dex_prices: &[f64],
    ) -> Option<PriceDiscoveryReport> {
        let n = spot_prices.len();
        if n < self.min_samples || n != perp_cex_prices.len() || n != perp_dex_prices.len() {
            return None;
        }

        // Compute log prices and first differences
        let mut d_y = Vec::with_capacity(n - 1);
        let mut y_lag = Vec::with_capacity(n - 1);
        for t in 0..n - 1 {
            let p_s = spot_prices[t].ln();
            let p_c = perp_cex_prices[t].ln();
            let p_d = perp_dex_prices[t].ln();
            if !p_s.is_finite() || !p_c.is_finite() || !p_d.is_finite() {
                return None;
            }
            y_lag.push([p_s, p_c, p_d]);

            let dp_s = spot_prices[t + 1].ln() - p_s;
            let dp_c = perp_cex_prices[t + 1].ln() - p_c;
            let dp_d = perp_dex_prices[t + 1].ln() - p_d;
            if !dp_s.is_finite() || !dp_c.is_finite() || !dp_d.is_finite() {
                return None;
            }
            d_y.push([dp_s, dp_c, dp_d]);
        }

        let m = n - 1;
        let mut mean_z1 = 0.0;
        let mut mean_z2 = 0.0;
        let mut mean_dy = [0.0; 3];
        for t in 0..m {
            let z1 = y_lag[t][0] - y_lag[t][1];
            let z2 = y_lag[t][1] - y_lag[t][2];
            mean_z1 += z1;
            mean_z2 += z2;
            for i in 0..3 {
                mean_dy[i] += d_y[t][i];
            }
        }
        let m_f = (m as f64).max(1.0);
        mean_z1 /= m_f;
        mean_z2 /= m_f;
        for val in &mut mean_dy {
            *val /= m_f;
        }

        // Structural cointegrating relations centered:
        let mut sum_z1_sq = 0.0;
        let mut sum_z2_sq = 0.0;
        let mut sum_z1_z2 = 0.0;
        let mut alpha_cross_z1 = [0.0; 3];
        let mut alpha_cross_z2 = [0.0; 3];

        for t in 0..m {
            let z1 = (y_lag[t][0] - y_lag[t][1]) - mean_z1;
            let z2 = (y_lag[t][1] - y_lag[t][2]) - mean_z2;
            sum_z1_sq += z1 * z1;
            sum_z2_sq += z2 * z2;
            sum_z1_z2 += z1 * z2;

            for i in 0..3 {
                let dy_c = d_y[t][i] - mean_dy[i];
                alpha_cross_z1[i] += dy_c * z1;
                alpha_cross_z2[i] += dy_c * z2;
            }
        }

        let det_z = sum_z1_sq * sum_z2_sq - sum_z1_z2 * sum_z1_z2;
        let mut a_col1 = [0.0; 3];
        let mut a_col2 = [0.0; 3];
        let mut alpha = [0.0; 3];
        if det_z.abs() > 1e-12 {
            for i in 0..3 {
                a_col1[i] = (alpha_cross_z1[i] * sum_z2_sq - alpha_cross_z2[i] * sum_z1_z2) / det_z;
                a_col2[i] = (alpha_cross_z2[i] * sum_z1_sq - alpha_cross_z1[i] * sum_z1_z2) / det_z;
                alpha[i] = 0.5 * (a_col1[i] + a_col2[i]);
            }
        } else {
            for i in 0..3 {
                a_col1[i] = alpha_cross_z1[i] / sum_z1_sq.max(1e-12);
                a_col2[i] = alpha_cross_z2[i] / sum_z2_sq.max(1e-12);
                alpha[i] = 0.5 * (a_col1[i] + a_col2[i]);
            }
        }

        // Intercepts c_i = mean_dy[i] - (a_col1[i]*mean_z1 + a_col2[i]*mean_z2)
        let mut intercept_c = [0.0; 3];
        for i in 0..3 {
            intercept_c[i] = mean_dy[i] - (a_col1[i] * mean_z1 + a_col2[i] * mean_z2);
        }

        // Gonzalo-Granger orthogonal weights gamma = alpha_perp:
        // In R^3 with 2 cointegrating vectors, alpha_perp is given by the cross product a_col1 x a_col2.
        let c0 = a_col1[1] * a_col2[2] - a_col1[2] * a_col2[1];
        let c1 = a_col1[2] * a_col2[0] - a_col1[0] * a_col2[2];
        let c2 = a_col1[0] * a_col2[1] - a_col1[1] * a_col2[0];
        let perp_raw = [c0.abs(), c1.abs(), c2.abs()];
        let sum_perp = perp_raw[0] + perp_raw[1] + perp_raw[2];
        let gg_shares = if sum_perp > 1e-12 {
            [
                perp_raw[0] / sum_perp,
                perp_raw[1] / sum_perp,
                perp_raw[2] / sum_perp,
            ]
        } else {
            let inv_alpha: Vec<f64> = alpha.iter().map(|&a| 1.0 / (a.abs().max(1e-4))).collect();
            let s: f64 = inv_alpha.iter().sum();
            [inv_alpha[0] / s, inv_alpha[1] / s, inv_alpha[2] / s]
        };

        // Innovation covariance matrix Omega = (1 / m) * sum (e_t * e_t^T)
        let mut omega = [[0.0; 3]; 3];
        for t in 0..m {
            let z1 = y_lag[t][0] - y_lag[t][1];
            let z2 = y_lag[t][1] - y_lag[t][2];
            let mut res = [0.0; 3];
            for i in 0..3 {
                let predicted = intercept_c[i] + a_col1[i] * z1 + a_col2[i] * z2;
                res[i] = d_y[t][i] - predicted;
            }
            for i in 0..3 {
                for j in 0..3 {
                    omega[i][j] += res[i] * res[j];
                }
            }
        }
        for row in &mut omega {
            for val in row {
                *val /= m_f;
            }
        }

        // Cholesky decomposition of Omega for Hasbrouck Information Share:
        let mut l = [[0.0; 3]; 3];
        let l00 = omega[0][0].max(1e-12).sqrt();
        l[0][0] = l00;
        l[1][0] = omega[1][0] / l00;
        l[2][0] = omega[2][0] / l00;

        let l11_sq = (omega[1][1] - l[1][0] * l[1][0]).max(1e-12);
        let l11 = l11_sq.sqrt();
        l[1][1] = l11;
        l[2][1] = (omega[2][1] - l[2][0] * l[1][0]) / l11;

        let l22_sq = (omega[2][2] - l[2][0] * l[2][0] - l[2][1] * l[2][1]).max(1e-12);
        l[2][2] = l22_sq.sqrt();

        // gamma^T * L:
        let mut gamma_l = [0.0; 3];
        for j in 0..3 {
            for i in 0..3 {
                gamma_l[j] += gg_shares[i] * l[i][j];
            }
        }
        let denom_is = gamma_l.iter().map(|&x| x * x).sum::<f64>().max(1e-12);
        let is_shares = [
            (gamma_l[0] * gamma_l[0]) / denom_is,
            (gamma_l[1] * gamma_l[1]) / denom_is,
            (gamma_l[2] * gamma_l[2]) / denom_is,
        ];

        let dominant_leader_idx = is_shares
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(1);

        Some(PriceDiscoveryReport {
            cointegrating_beta: [1.0, -1.0],
            speed_of_adjustment_alpha: alpha,
            gonzalo_granger_shares: gg_shares,
            hasbrouck_info_shares: is_shares,
            dominant_leader_idx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_basis_harvest_trigger() {
        let evaluator = FastBasisHarvestEvaluator::default();

        // Entry basis 15 bps, compressed to 2 bps (86.7% compression) in 0.5 hours of an 8h epoch
        let harvest = evaluator.should_harvest_early(
            15.0, 2.0, 0.5, 8.0, 0.20, 5.00,
            1.50, // Net PnL = 5.00 + 0.20 - 1.50 = $3.70 > $1.00 min
        );
        assert!(
            harvest,
            "Should harvest when 86.7% basis compressed in 30min"
        );

        // Insufficient compression: 15 bps -> 10 bps (33.3% compression)
        let harvest_low = evaluator.should_harvest_early(15.0, 10.0, 0.5, 8.0, 0.20, 1.50, 1.50);
        assert!(!harvest_low, "Should not harvest on low compression");
    }

    #[test]
    fn test_asymmetric_funding_harvest_decision() {
        let evaluator = FastBasisHarvestEvaluator::default();

        // 1. Imminent positive income in 120s (e.g. Hyperliquid 1h payment): Delay for settlement
        let decision_delay = evaluator.evaluate_asymmetric_funding_harvest(
            20.0, 3.0, // 85% compression
            120, 18000, // Venue A pays in 120s, Venue B in 5h
            2.50, -1.00, // Upcoming payment on A is +$2.50
            10.0,
        );
        assert_eq!(
            decision_delay,
            AsymmetricHarvestDecision::DelayForSettlement {
                delay_seconds: 120,
                expected_payment_usd: 2.50
            }
        );

        // 2. Imminent fee in 90s: Accelerate exit before payment
        let decision_accel = evaluator.evaluate_asymmetric_funding_harvest(
            20.0, 3.0, 90, 18000, -3.00, 1.00, // Upcoming fee of -$3.00
            10.0,
        );
        assert_eq!(
            decision_accel,
            AsymmetricHarvestDecision::AccelerateExitBeforeFee {
                seconds_remaining: 90
            }
        );

        // 3. Imminent settlement far away (> 300s): Harvest immediately
        let decision_immediate =
            evaluator.evaluate_asymmetric_funding_harvest(20.0, 3.0, 3600, 14400, 1.50, 1.50, 10.0);
        assert_eq!(
            decision_immediate,
            AsymmetricHarvestDecision::HarvestImmediately
        );
    }

    #[test]
    fn test_dynamic_compression_boundary() {
        let evaluator = FastBasisHarvestEvaluator::default(); // min_compression_ratio = 0.80

        // Early in epoch (<= 0.25): strict 0.80
        assert_eq!(evaluator.dynamic_compression_boundary(0.10), 0.80);
        assert_eq!(evaluator.dynamic_compression_boundary(0.25), 0.80);

        // Mid-epoch (0.50): halfway between 0.80 and 0.60 -> 0.70
        let mid_boundary = evaluator.dynamic_compression_boundary(0.50);
        assert!((mid_boundary - 0.70).abs() < 1e-6);

        // Late epoch (>= 0.75): 0.60
        assert_eq!(evaluator.dynamic_compression_boundary(0.75), 0.60);
        assert_eq!(evaluator.dynamic_compression_boundary(0.90), 0.60);

        // At 4h of 8h epoch (elapsed_fraction = 0.50), a 72% compression should harvest
        // (15 bps -> 4.2 bps = 72% compression)
        let harvest_mid = evaluator.should_harvest_early(15.0, 4.2, 4.0, 8.0, 0.50, 4.00, 1.50);
        assert!(
            harvest_mid,
            "72% compression should harvest at mid-epoch where boundary is 70%"
        );
    }

    #[test]
    fn test_leung_li_basis_stopping_decisions() {
        let router = LeungLiBasisStoppingRouter::new(0.10);

        // Parameters:
        // theta = 5.0 bps, kappa = 2.0, sigma = 12.0 bps
        // borrow = 6%, funding = 16% (net carry = -10% = 10% positive yield)
        // roundtrip taker fee = 8.0 bps
        let decision_hold = router.evaluate_basis_exit(
            5.0,  // current basis = mean
            5.0,  // mean basis theta
            2.0,  // reversion speed kappa
            12.0, // basis volatility sigma
            0.06, // borrow rate
            0.16, // funding rate (positive yield)
            8.0,  // taker fee bps
        );
        match decision_hold {
            BasisExitDecision::HoldCarryHarvest {
                net_carry_bps_per_day,
            } => {
                assert!(
                    net_carry_bps_per_day > 0.0,
                    "Carry should be positive yield"
                );
            }
            other => panic!("Expected HoldCarryHarvest at mean, got {other:?}"),
        }

        // Test take profit when basis converges/compresses significantly upwards to hurdle
        let decision_take_profit = router.evaluate_basis_exit(
            35.0, // current basis is high (e.g. 35 bps > hurdle b_U)
            5.0, 2.0, 12.0, 0.06, 0.16, 8.0,
        );
        match decision_take_profit {
            BasisExitDecision::TakeProfitConvergence {
                exit_spread_bps,
                hurdle_b_u_bps,
            } => {
                assert_eq!(exit_spread_bps, 35.0);
                assert!(hurdle_b_u_bps < 35.0);
            }
            other => panic!("Expected TakeProfitConvergence, got {other:?}"),
        }

        // Test stop loss when basis expands severely negatively below b_L*
        let decision_stop_loss =
            router.evaluate_basis_exit(-150.0, 5.0, 2.0, 12.0, 0.06, 0.16, 8.0);
        match decision_stop_loss {
            BasisExitDecision::StopLossRoll {
                exit_spread_bps,
                hurdle_b_l_bps,
            } => {
                assert_eq!(exit_spread_bps, -150.0);
                assert!(hurdle_b_l_bps > -150.0);
            }
            other => panic!("Expected StopLossRoll, got {other:?}"),
        }
    }

    #[test]
    fn test_vecm_basis_price_discovery() {
        let analyzer = VecmBasisAnalyzer::new(40);

        // Simulate 60 time points where Perp CEX (index 1) leads price discovery:
        let mut spot = Vec::with_capacity(60);
        let mut perp_cex = Vec::with_capacity(60);
        let mut perp_dex = Vec::with_capacity(60);

        let mut p_s = 60000.0;
        let mut p_c = 60000.0;
        let mut p_d = 60000.0;

        spot.push(p_s);
        perp_cex.push(p_c);
        perp_dex.push(p_d);

        let mut seed = 123456789u64;
        let mut next_noise = || -> f64 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let val = ((seed >> 33) as f64) / (0x7FFF_FFFF as f64);
            (val - 0.5) * 2.0 // [-1.0, 1.0]
        };

        for _ in 1..80 {
            // Perp CEX moves as pure martingale random walk (unforecastable exogenous leader)
            let d_pc = next_noise() * 20.0;
            p_c += d_pc;

            let prev_s = *spot.last().unwrap();
            let prev_c = *perp_cex.last().unwrap();
            let prev_d = *perp_dex.last().unwrap();

            // Spot and Perp DEX error-correct towards lagged Perp CEX
            p_s = prev_s + 0.65 * (prev_c - prev_s) + (next_noise() * 1.5);
            p_d = prev_d + 0.55 * (prev_c - prev_d) + (next_noise() * 1.5);

            spot.push(p_s);
            perp_cex.push(p_c);
            perp_dex.push(p_d);
        }

        let report = analyzer
            .analyze_price_discovery(&spot, &perp_cex, &perp_dex)
            .expect("VECM analysis should succeed on 60 samples");

        // The shares must sum to 1.0 (within float epsilon)
        let gg_sum: f64 = report.gonzalo_granger_shares.iter().sum();
        let is_sum: f64 = report.hasbrouck_info_shares.iter().sum();
        assert!(
            (gg_sum - 1.0).abs() < 1e-4,
            "GG shares must sum to 1: {gg_sum}"
        );
        assert!(
            (is_sum - 1.0).abs() < 1e-4,
            "IS shares must sum to 1: {is_sum}"
        );

        // Perp CEX (index 1) should be the price discovery leader
        assert_eq!(
            report.dominant_leader_idx, 1,
            "Perp CEX should be dominant leader"
        );
    }
}
