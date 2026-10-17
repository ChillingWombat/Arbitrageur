//! Multi-Horizon High-Frequency Funding Rate Term Structure & Monotone Cubic Spline Engine.
//!
//! Models perpetual funding rates across heterogeneous settlement epochs (8h Binance/OKX/Lighter,
//! 1h Hyperliquid, continuous streaming dYdX/Vertex) using Fritsch-Carlson monotone cubic Hermite
//! splines, diurnal Fourier harmonic series, and analytical optimal rollover timing criteria.

use serde::{Deserialize, Serialize};

/// Shape-preserving C^1 monotone cubic Hermite spline fitting arbitrary coordinate grids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FritschCarlsonMonotoneSpline {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    pub ms: Vec<f64>,
}

impl FritschCarlsonMonotoneSpline {
    /// Fits a monotone cubic spline to strictly increasing knots (xs, ys).
    pub fn fit(xs: &[f64], ys: &[f64]) -> Option<Self> {
        let n = xs.len();
        if n < 2 || ys.len() != n {
            return None;
        }
        for i in 1..n {
            if xs[i] <= xs[i - 1] {
                return None;
            }
        }
        let mut deltas = vec![0.0; n - 1];
        for k in 0..n - 1 {
            deltas[k] = (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k]);
        }
        let mut ms = vec![0.0; n];
        ms[0] = deltas[0];
        ms[n - 1] = deltas[n - 2];
        for k in 1..n - 1 {
            ms[k] = 0.5 * (deltas[k - 1] + deltas[k]);
        }
        for k in 0..n - 1 {
            if deltas[k].abs() < 1e-12 {
                ms[k] = 0.0;
                ms[k + 1] = 0.0;
            } else {
                let alpha = ms[k] / deltas[k];
                let beta = ms[k + 1] / deltas[k];
                if alpha < 0.0 {
                    ms[k] = 0.0;
                }
                if beta < 0.0 {
                    ms[k + 1] = 0.0;
                }
                let norm_sq = alpha * alpha + beta * beta;
                if norm_sq > 9.0 {
                    let tau = 3.0 / norm_sq.sqrt();
                    ms[k] = tau * alpha * deltas[k];
                    ms[k + 1] = tau * beta * deltas[k];
                }
            }
        }
        Some(Self {
            xs: xs.to_vec(),
            ys: ys.to_vec(),
            ms,
        })
    }

    /// Evaluates the spline at query point `x`.
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let mut k = 0;
        while k < n - 2 && self.xs[k + 1] < x {
            k += 1;
        }
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let h00 = 2.0 * t.powi(3) - 3.0 * t.powi(2) + 1.0;
        let h10 = t.powi(3) - 2.0 * t.powi(2) + t;
        let h01 = -2.0 * t.powi(3) + 3.0 * t.powi(2);
        let h11 = t.powi(3) - t.powi(2);
        self.ys[k] * h00 + h * self.ms[k] * h10 + self.ys[k + 1] * h01 + h * self.ms[k + 1] * h11
    }
}

/// Decision output for early fast basis harvesting upon rapid spread compression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FastHarvestDecision {
    pub should_harvest_early: bool,
    pub irr_velocity_multiplier: f64,
    pub net_realized_pnl_bps: f64,
    pub annualized_irr_bps: f64,
    pub message: String,
}

/// Dynamic Fast Basis Harvest Engine for early capital recycling upon rapid basis compression.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DynamicFastBasisHarvestEngine {
    pub min_irr_multiplier: f64,
    pub min_compression_ratio: f64,
    pub min_net_pnl_bps: f64,
}

impl DynamicFastBasisHarvestEngine {
    pub fn new(min_irr_multiplier: f64, min_compression_ratio: f64, min_net_pnl_bps: f64) -> Self {
        Self {
            min_irr_multiplier,
            min_compression_ratio,
            min_net_pnl_bps,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_harvest(
        &self,
        entry_basis_bps: f64,
        current_basis_bps: f64,
        elapsed_hours: f64,
        total_epoch_hours: f64,
        accrued_funding_bps: f64,
        round_trip_fee_bps: f64,
        passive_target_pnl_bps: f64,
        _notional_usd: f64,
    ) -> FastHarvestDecision {
        let basis_gain_bps = entry_basis_bps - current_basis_bps;
        let net_pnl_bps = basis_gain_bps + accrued_funding_bps - round_trip_fee_bps;
        let compression_ratio = if entry_basis_bps.abs() > 1e-6 {
            basis_gain_bps / entry_basis_bps
        } else {
            0.0
        };

        let time_ratio = if elapsed_hours > 0.0 {
            (total_epoch_hours / elapsed_hours).max(1.0)
        } else {
            1.0
        };

        let irr_multiplier = if passive_target_pnl_bps > 0.0 {
            (net_pnl_bps / passive_target_pnl_bps) * time_ratio
        } else {
            1.0
        };

        let annualized_irr_bps = if elapsed_hours > 0.0 {
            net_pnl_bps * (8760.0 / elapsed_hours)
        } else {
            0.0
        };

        let should_harvest = net_pnl_bps >= self.min_net_pnl_bps
            && compression_ratio >= self.min_compression_ratio
            && irr_multiplier >= self.min_irr_multiplier;

        let message = if should_harvest {
            format!(
                "Fast Basis Harvest Triggered: net PnL {:.2} bps (IRR multiplier {:.2}x >= {:.2}x, compression {:.1}%, elapsed {:.1}h)",
                net_pnl_bps, irr_multiplier, self.min_irr_multiplier, compression_ratio * 100.0, elapsed_hours
            )
        } else {
            format!(
                "Hold: net PnL {:.2} bps, IRR multiplier {:.2}x, elapsed {:.1}h",
                net_pnl_bps, irr_multiplier, elapsed_hours
            )
        };

        FastHarvestDecision {
            should_harvest_early: should_harvest,
            irr_velocity_multiplier: irr_multiplier,
            net_realized_pnl_bps: net_pnl_bps,
            annualized_irr_bps,
            message,
        }
    }
}

/// Rollover decision verdict for rolling a funding basis trade between tenors/venues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundingRolloverDecision {
    /// Current annualized funding yield in basis points.
    pub current_funding_yield_bps: f64,
    /// Next tenor/venue annualized funding yield in basis points.
    pub next_tenor_yield_bps: f64,
    /// Net annualized yield spread (next - current) in basis points.
    pub yield_spread_bps: f64,
    /// Transaction fee friction hurdle amortized over remaining horizon in basis points.
    pub rollover_cost_bps: f64,
    /// Optimal rollover execution horizon in seconds.
    pub optimal_rollover_seconds: f64,
    /// Whether rolling over yields positive risk-adjusted carry net of friction.
    pub should_execute_rollover: bool,
}

/// Diurnal Fourier harmonic series for intraday funding rate seasonality.
/// Captures 24h diurnal macro, 12h tidal, and 8h CEX settlement cycles.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DiurnalFundingFourierSeries {
    pub mean_level: f64,
    pub cos_coefficients: [f64; 3], // 24h, 12h, 8h harmonics
    pub sin_coefficients: [f64; 3],
}

impl Default for DiurnalFundingFourierSeries {
    fn default() -> Self {
        Self {
            mean_level: 0.0001, // 1 bp baseline 8h rate
            cos_coefficients: [0.00005, 0.00002, 0.00004],
            sin_coefficients: [0.00003, 0.00001, 0.00005],
        }
    }
}

impl DiurnalFundingFourierSeries {
    /// Evaluates the diurnal adjustment at time `t_hours` (UTC hour [0, 24)).
    pub fn evaluate_diurnal_rate(&self, t_hours: f64) -> f64 {
        let mut rate = self.mean_level;
        for k in 1..=3 {
            let omega = 2.0 * std::f64::consts::PI * (k as f64) * t_hours / 24.0;
            rate += self.cos_coefficients[k - 1] * omega.cos()
                + self.sin_coefficients[k - 1] * omega.sin();
        }
        rate
    }
}

/// Multi-Horizon Continuous Funding Term Structure Engine using Fritsch-Carlson
/// monotone cubic Hermite splines to guarantee shape preservation and no Runge oscillations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundingTermStructureSplineEngine {
    /// Tenor grid in hours (strictly ascending, e.g. [1.0, 4.0, 8.0, 24.0, 168.0]).
    pub tenor_hours: Vec<f64>,
    /// Annualized basis yields at each tenor knot.
    pub basis_yields_annual: Vec<f64>,
    /// Hermite tangents computed at each tenor knot.
    pub spline_tangents: Vec<f64>,
    /// Diurnal Fourier series for intraday rate fluctuations.
    pub diurnal_series: DiurnalFundingFourierSeries,
}

impl FundingTermStructureSplineEngine {
    /// Constructs a new monotone cubic Hermite spline term structure engine.
    /// Returns None if inputs have less than 3 knots, lengths mismatch, or tenors are not strictly increasing.
    pub fn new(tenors: Vec<f64>, yields: Vec<f64>) -> Option<Self> {
        let n = tenors.len();
        if n < 3 || yields.len() != n {
            return None;
        }

        for i in 1..n {
            if tenors[i] <= tenors[i - 1] {
                return None;
            }
        }

        // 1. Calculate secant slopes: Delta_k = (y_{k+1} - y_k) / (x_{k+1} - x_k)
        let mut deltas = vec![0.0; n - 1];
        for k in 0..n - 1 {
            let dx = tenors[k + 1] - tenors[k];
            deltas[k] = (yields[k + 1] - yields[k]) / dx;
        }

        // 2. Initialize tangents using arithmetic averages (one-sided for endpoints)
        let mut ds = vec![0.0; n];
        ds[0] = deltas[0];
        ds[n - 1] = deltas[n - 2];
        for k in 1..n - 1 {
            ds[k] = 0.5 * (deltas[k - 1] + deltas[k]);
        }

        // 3. Fritsch-Carlson monotonicity check & adjustment
        for k in 0..n - 1 {
            if deltas[k].abs() < 1e-12 {
                ds[k] = 0.0;
                ds[k + 1] = 0.0;
            } else {
                let alpha = ds[k] / deltas[k];
                let beta = ds[k + 1] / deltas[k];

                if alpha < 0.0 {
                    ds[k] = 0.0;
                }
                if beta < 0.0 {
                    ds[k + 1] = 0.0;
                }

                let norm_sq = alpha * alpha + beta * beta;
                if norm_sq > 9.0 {
                    let tau = 3.0 / norm_sq.sqrt();
                    ds[k] = tau * alpha * deltas[k];
                    ds[k + 1] = tau * beta * deltas[k];
                }
            }
        }

        Some(Self {
            tenor_hours: tenors,
            basis_yields_annual: yields,
            spline_tangents: ds,
            diurnal_series: DiurnalFundingFourierSeries::default(),
        })
    }

    /// Evaluates continuous annualized basis yield y(tau) at maturity tenor `tau_hours`.
    pub fn evaluate_basis_yield(&self, tau_hours: f64) -> f64 {
        let n = self.tenor_hours.len();
        if tau_hours <= self.tenor_hours[0] {
            return self.basis_yields_annual[0];
        }
        if tau_hours >= self.tenor_hours[n - 1] {
            return self.basis_yields_annual[n - 1];
        }

        let mut k = 0;
        while k < n - 2 && self.tenor_hours[k + 1] < tau_hours {
            k += 1;
        }

        let x0 = self.tenor_hours[k];
        let x1 = self.tenor_hours[k + 1];
        let h = x1 - x0;
        let t = (tau_hours - x0) / h;

        let y0 = self.basis_yields_annual[k];
        let y1 = self.basis_yields_annual[k + 1];
        let d0 = self.spline_tangents[k];
        let d1 = self.spline_tangents[k + 1];

        // Hermite basis polynomials
        let h00 = 2.0 * t.powi(3) - 3.0 * t.powi(2) + 1.0;
        let h10 = t.powi(3) - 2.0 * t.powi(2) + t;
        let h01 = -2.0 * t.powi(3) + 3.0 * t.powi(2);
        let h11 = t.powi(3) - t.powi(2);

        y0 * h00 + h * d0 * h10 + y1 * h01 + h * d1 * h11
    }

    /// Evaluates instantaneous forward basis rate: f(tau) = y(tau) + tau * y'(tau).
    pub fn evaluate_forward_basis_rate(&self, tau_hours: f64) -> f64 {
        let n = self.tenor_hours.len();
        if tau_hours <= self.tenor_hours[0] {
            return self.basis_yields_annual[0] + self.tenor_hours[0] * self.spline_tangents[0];
        }
        if tau_hours >= self.tenor_hours[n - 1] {
            return self.basis_yields_annual[n - 1]
                + self.tenor_hours[n - 1] * self.spline_tangents[n - 1];
        }

        let mut k = 0;
        while k < n - 2 && self.tenor_hours[k + 1] < tau_hours {
            k += 1;
        }

        let x0 = self.tenor_hours[k];
        let x1 = self.tenor_hours[k + 1];
        let h = x1 - x0;
        let t = (tau_hours - x0) / h;

        let y0 = self.basis_yields_annual[k];
        let y1 = self.basis_yields_annual[k + 1];
        let d0 = self.spline_tangents[k];
        let d1 = self.spline_tangents[k + 1];

        // First derivative of Hermite basis polynomials w.r.t t
        let dh00 = 6.0 * t.powi(2) - 6.0 * t;
        let dh10 = 3.0 * t.powi(2) - 4.0 * t + 1.0;
        let dh01 = -6.0 * t.powi(2) + 6.0 * t;
        let dh11 = 3.0 * t.powi(2) - 2.0 * t;

        let dy_dt = y0 * dh00 + h * d0 * dh10 + y1 * dh01 + h * d1 * dh11;
        let dy_dtau = dy_dt / h;

        let y_tau = self.evaluate_basis_yield(tau_hours);
        y_tau + tau_hours * dy_dtau
    }

    /// Evaluates the expected cash flow for an asymmetric settlement pair (e.g. 8h Binance vs 1h Hyperliquid)
    /// over a designated holding horizon `holding_hours`.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_discrete_cash_flow_spread(
        &self,
        holding_hours: f64,
        time_to_venue_a_settlement_hours: f64,
        venue_a_cadence_hours: f64,
        venue_a_rate_per_epoch: f64,
        time_to_venue_b_settlement_hours: f64,
        venue_b_cadence_hours: f64,
        venue_b_rate_per_epoch: f64,
    ) -> (f64, f64, f64) {
        // Calculate number of discrete cash flow events in holding horizon
        let count_a = if holding_hours >= time_to_venue_a_settlement_hours {
            1.0 + ((holding_hours - time_to_venue_a_settlement_hours) / venue_a_cadence_hours)
                .floor()
        } else {
            0.0
        };

        let count_b = if holding_hours >= time_to_venue_b_settlement_hours {
            1.0 + ((holding_hours - time_to_venue_b_settlement_hours) / venue_b_cadence_hours)
                .floor()
        } else {
            0.0
        };

        let cashflow_a = count_a * venue_a_rate_per_epoch;
        let cashflow_b = count_b * venue_b_rate_per_epoch;
        let net_spread = cashflow_b - cashflow_a;

        (cashflow_a, cashflow_b, net_spread)
    }

    /// Evaluates optimal rollover timing criterion balancing yield improvement against round-trip fees.
    pub fn evaluate_rollover(
        &self,
        current_tenor_hours: f64,
        next_tenor_hours: f64,
        remaining_seconds: f64,
        roundtrip_fee_bps: f64,
        basis_volatility_annual: f64,
    ) -> FundingRolloverDecision {
        let y_curr_bps = self.evaluate_basis_yield(current_tenor_hours) * 10000.0;
        let y_next_bps = self.evaluate_basis_yield(next_tenor_hours) * 10000.0;
        let spread_bps = y_next_bps - y_curr_bps;

        let delta_tau_years = remaining_seconds / (365.25 * 86400.0);
        let fee_hurdle_bps = roundtrip_fee_bps / delta_tau_years.max(1e-4);
        let brownian_cushion_bps = basis_volatility_annual
            * 10000.0
            * (2.0 / (std::f64::consts::PI * delta_tau_years.max(1e-4))).sqrt();

        let should_roll = spread_bps >= (fee_hurdle_bps + brownian_cushion_bps);
        let optimal_sec = (remaining_seconds * 0.40).clamp(60.0, 3600.0);

        FundingRolloverDecision {
            current_funding_yield_bps: y_curr_bps,
            next_tenor_yield_bps: y_next_bps,
            yield_spread_bps: spread_bps,
            rollover_cost_bps: fee_hurdle_bps,
            optimal_rollover_seconds: optimal_sec,
            should_execute_rollover: should_roll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fritsch_carlson_fit_and_eval() {
        let xs = vec![1.0, 4.0, 8.0, 24.0];
        let ys = vec![4.0, 7.5, 12.0, 16.5];
        let spline = FritschCarlsonMonotoneSpline::fit(&xs, &ys).expect("fit must succeed");
        assert_eq!(spline.eval(1.0), 4.0);
        assert_eq!(spline.eval(24.0), 16.5);
        let y_mid = spline.eval(6.0);
        assert!(y_mid > 7.5 && y_mid < 12.0);
    }

    #[test]
    fn test_dynamic_fast_basis_harvest() {
        let engine = DynamicFastBasisHarvestEngine::new(2.5, 0.25, 0.2);
        let decision = engine.evaluate_harvest(25.0, 3.0, 0.5, 8.0, 0.8, 2.5, 18.0, 10000.0);
        assert!(decision.should_harvest_early);
        assert!(decision.irr_velocity_multiplier >= 2.5);
        assert!(decision.message.contains("Fast Basis Harvest Triggered"));
    }

    #[test]
    fn test_monotone_spline_no_runge_oscillations() {
        // Monotonically increasing basis curve across tenors: 1h, 4h, 8h, 24h, 168h
        let tenors = vec![1.0, 4.0, 8.0, 24.0, 168.0];
        let yields = vec![0.08, 0.09, 0.10, 0.12, 0.15]; // Annualized carry: 8% to 15%

        let engine = FundingTermStructureSplineEngine::new(tenors, yields)
            .expect("Spline creation must succeed");

        // Interpolate at intermediate points
        let y_2h = engine.evaluate_basis_yield(2.0);
        let y_6h = engine.evaluate_basis_yield(6.0);
        let y_12h = engine.evaluate_basis_yield(12.0);

        assert!((0.08..=0.09).contains(&y_2h));
        assert!((0.09..=0.10).contains(&y_6h));
        assert!((0.10..=0.12).contains(&y_12h));

        // Verify positive forward basis carry
        let f_4h = engine.evaluate_forward_basis_rate(4.0);
        assert!(f_4h > 0.0);
    }

    #[test]
    fn test_discrete_cash_flow_asymmetry() {
        let tenors = vec![1.0, 4.0, 8.0, 24.0];
        let yields = vec![0.08, 0.09, 0.10, 0.11];
        let engine = FundingTermStructureSplineEngine::new(tenors, yields).unwrap();

        let (cf_a, cf_b, net) = engine.evaluate_discrete_cash_flow_spread(
            2.0,    // 2h holding
            1.0,    // 1h until A settles
            8.0,    // A settles every 8h
            0.0003, // 3 bps per 8h
            0.5,    // 0.5h until B settles
            1.0,    // B settles every 1h
            0.00004,
        );

        assert_eq!(cf_a, 0.0003); // 1 epoch of A
        assert_eq!(cf_b, 0.00008); // 2 epochs of B (0.5h, 1.5h)
        assert!((net - (0.00008 - 0.0003)).abs() < 1e-12);
    }

    #[test]
    fn test_optimal_rollover_decision() {
        let tenors = vec![1.0, 4.0, 8.0, 24.0];
        let yields = vec![0.05, 0.08, 0.12, 0.15]; // Rapidly steepening curve
        let engine = FundingTermStructureSplineEngine::new(tenors, yields).unwrap();

        // 14 days remaining, small fee friction (0.5 bps round-trip), 50 bps basis tracking vol
        let decision_calm = engine.evaluate_rollover(
            1.0,            // From 1h
            8.0,            // To 8h (700 bps annualized spread)
            14.0 * 86400.0, // 14 days remaining
            0.5,            // 0.5 bps fee
            0.005,          // 50 bps basis tracking vol
        );
        assert!(decision_calm.yield_spread_bps > 500.0);
        assert!(decision_calm.should_execute_rollover);

        // Huge fee friction (500 bps) over 1 hour
        let decision_high_fee = engine.evaluate_rollover(1.0, 4.0, 3600.0, 500.0, 0.05);
        assert!(!decision_high_fee.should_execute_rollover);
    }
}
