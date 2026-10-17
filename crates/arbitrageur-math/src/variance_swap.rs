//! Discrete Variance Swap Replication & Volatility Risk Premium (VRP) Dispersion Engine.
//!
//! Replicates model-free log-contract variance swap strikes from discrete option chains (Deribit, Aevo, Lyra)
//! following the Neuberger (1994) and Demeterfi-Derman-Kamal-Zou (1999) replication theorem:
//!
//! VarSwap_0(T) = \frac{2 e^{rT}}{T} \left[ \int_0^{F_0} \frac{P(K, T)}{K^2} dK + \int_{F_0}^\infty \frac{C(K, T)}{K^2} dK \right]
//!
//! Using discrete numerical integration with trapezoidal strike steps \Delta K_i:
//! VarSwap_0(T) \approx \frac{2 e^{rT}}{T} \sum_{i=1}^M \frac{\Delta K_i}{K_i^2} Q(K_i)
//! where Q(K_i) is the out-of-the-money put for K_i < F_0, the out-of-the-money call for K_i > F_0,
//! and the average at the forward ATM boundary.
//!
//! Also measures the continuous Volatility Risk Premium (VRP = \sqrt{VarSwap} - RV) to isolate
//! structural cross-asset dispersion between benchmark variance (BTC) and constituent variance (ETH).

use serde::{Deserialize, Serialize};

/// Discrete option quote on a single strike level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscreteOptionQuote {
    pub strike: f64,
    pub bid_price: f64,
    pub ask_price: f64,
    pub is_call: bool,
}

impl DiscreteOptionQuote {
    pub fn new(strike: f64, bid_price: f64, ask_price: f64, is_call: bool) -> Self {
        Self {
            strike: strike.max(1e-4),
            bid_price: bid_price.max(0.0),
            ask_price: ask_price.max(0.0),
            is_call,
        }
    }

    /// Mid price between bid and ask
    pub fn mid_price(&self) -> f64 {
        0.5 * (self.bid_price + self.ask_price)
    }
}

/// Discrete Variance Swap Replicator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarianceSwapReplicator {
    /// Annualized risk-free interest rate r (e.g. 0.04 = 4%)
    pub risk_free_rate: f64,
}

impl Default for VarianceSwapReplicator {
    fn default() -> Self {
        Self {
            risk_free_rate: 0.04,
        }
    }
}

impl VarianceSwapReplicator {
    pub fn new(risk_free_rate: f64) -> Self {
        Self {
            risk_free_rate: risk_free_rate.max(0.0),
        }
    }

    /// Replicates fair annualized variance swap strike VarSwap_0(T) from a discrete option slice.
    /// Returns annualized fair variance strike \sigma_{var}^2 (e.g. 0.36 = 60% implied vol).
    pub fn replicate_variance_strike(
        &self,
        forward_price: f64,
        time_to_expiry_years: f64,
        mut options: Vec<DiscreteOptionQuote>,
    ) -> Option<f64> {
        let t = time_to_expiry_years;
        if t <= 1e-4 || options.len() < 3 || forward_price <= 0.0 {
            return None;
        }

        // Sort options by strike ascending
        options.sort_by(|a, b| {
            a.strike
                .partial_cmp(&b.strike)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let m = options.len();
        let f0 = forward_price;
        let mut integral_sum = 0.0;

        for i in 0..m {
            let k_i = options[i].strike;
            let mid_price = options[i].mid_price();

            // Compute delta K_i
            let delta_k = if i == 0 {
                (options[1].strike - options[0].strike).max(1e-4)
            } else if i == m - 1 {
                (options[m - 1].strike - options[m - 2].strike).max(1e-4)
            } else {
                0.5 * (options[i + 1].strike - options[i - 1].strike).max(1e-4)
            };

            // Select OTM contract Q(K_i)
            let q_price = if (k_i - f0).abs() < (delta_k * 0.5) {
                // Near ATM: split 50/50 between call and put
                mid_price * 0.5
            } else {
                // Out-of-the-money contract price
                mid_price
            };

            integral_sum += (delta_k / (k_i * k_i)) * q_price;
        }

        let discount_factor = (self.risk_free_rate * t).exp();
        let var_strike = (2.0 * discount_factor / t) * integral_sum;

        if var_strike.is_finite() && var_strike >= 0.0 {
            Some(var_strike)
        } else {
            None
        }
    }

    /// Computes Volatility Risk Premium (VRP = \sqrt{VarSwap} - RV) in volatility points.
    pub fn compute_vrp(&self, var_strike: f64, realized_vol: f64) -> f64 {
        let implied_vol = var_strike.max(0.0).sqrt();
        implied_vol - realized_vol.max(0.0)
    }

    /// Evaluates cross-asset variance dispersion between primary benchmark (e.g. BTC) and constituent (e.g. ETH).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_variance_dispersion(
        &self,
        symbol_bench: &str,
        var_strike_bench: f64,
        rv_bench: f64,
        symbol_const: &str,
        var_strike_const: f64,
        rv_const: f64,
        min_vrp_spread_hurdle: f64,
    ) -> VarianceDispersionOpportunity {
        let vrp_bench = self.compute_vrp(var_strike_bench, rv_bench);
        let vrp_const = self.compute_vrp(var_strike_const, rv_const);
        let vrp_spread = vrp_const - vrp_bench; // Positive if constituent variance has higher risk premium

        let hurdle = min_vrp_spread_hurdle.max(0.01);

        if vrp_spread >= hurdle {
            // Constituent variance is richer than benchmark variance:
            // Sell Constituent Variance (ETH) & Buy Benchmark Variance (BTC)
            VarianceDispersionOpportunity {
                benchmark_symbol: symbol_bench.to_string(),
                constituent_symbol: symbol_const.to_string(),
                benchmark_vrp: vrp_bench,
                constituent_vrp: vrp_const,
                vrp_spread,
                action: VarianceDispersionAction::SellConstituentBuyBenchmark,
            }
        } else if vrp_spread <= -hurdle {
            // Benchmark variance is richer than constituent variance:
            // Sell Benchmark Variance (BTC) & Buy Constituent Variance (ETH)
            VarianceDispersionOpportunity {
                benchmark_symbol: symbol_bench.to_string(),
                constituent_symbol: symbol_const.to_string(),
                benchmark_vrp: vrp_bench,
                constituent_vrp: vrp_const,
                vrp_spread,
                action: VarianceDispersionAction::SellBenchmarkBuyConstituent,
            }
        } else {
            VarianceDispersionOpportunity {
                benchmark_symbol: symbol_bench.to_string(),
                constituent_symbol: symbol_const.to_string(),
                benchmark_vrp: vrp_bench,
                constituent_vrp: vrp_const,
                vrp_spread,
                action: VarianceDispersionAction::HoldNeutral,
            }
        }
    }

    /// Replicates fair annualized variance strike VarSwap_0(T) with SVI power-law wing extrapolation
    /// for deep OTM puts (below min strike) and deep OTM calls (above max strike).
    pub fn replicate_variance_strike_svi_tails(
        &self,
        forward_price: f64,
        time_to_expiry_years: f64,
        options: Vec<DiscreteOptionQuote>,
        svi: &SviParameters,
    ) -> Option<f64> {
        let base_var =
            self.replicate_variance_strike(forward_price, time_to_expiry_years, options.clone())?;
        if options.is_empty() || forward_price <= 0.0 || time_to_expiry_years <= 1e-4 {
            return Some(base_var);
        }

        let mut sorted = options;
        sorted.sort_by(|a, b| {
            a.strike
                .partial_cmp(&b.strike)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let k_min = sorted.first()?.strike;
        let k_max = sorted.last()?.strike;
        let t = time_to_expiry_years;

        // Numerical integration of SVI put tail from 0.05 * k_min to k_min
        let mut put_tail_integral = 0.0;
        let p_steps = 25;
        let k_lower = (0.05 * k_min).max(1e-4);
        let dk_p = (k_min - k_lower) / p_steps as f64;
        for j in 0..p_steps {
            let k = k_lower + (j as f64 + 0.5) * dk_p;
            let log_m = (k / forward_price).ln();
            let iv = svi.implied_volatility(log_m, t);
            if let Some(p_bs) = crate::black_scholes::black_scholes_price(
                crate::black_scholes::OptionType::Put,
                forward_price,
                k,
                t,
                self.risk_free_rate,
                iv,
            ) {
                put_tail_integral += (dk_p / (k * k)) * p_bs;
            }
        }

        // Numerical integration of SVI call tail from k_max to 3.0 * k_max
        let mut call_tail_integral = 0.0;
        let c_steps = 25;
        let k_upper = (3.0 * k_max).max(k_max + 1.0);
        let dk_c = (k_upper - k_max) / c_steps as f64;
        for j in 0..c_steps {
            let k = k_max + (j as f64 + 0.5) * dk_c;
            let log_m = (k / forward_price).ln();
            let iv = svi.implied_volatility(log_m, t);
            if let Some(c_bs) = crate::black_scholes::black_scholes_price(
                crate::black_scholes::OptionType::Call,
                forward_price,
                k,
                t,
                self.risk_free_rate,
                iv,
            ) {
                call_tail_integral += (dk_c / (k * k)) * c_bs;
            }
        }

        let discount_factor = (self.risk_free_rate * t).exp();
        let tail_var = (2.0 * discount_factor / t) * (put_tail_integral + call_tail_integral);
        let total_var = base_var + tail_var;

        if total_var.is_finite() && total_var >= 0.0 {
            Some(total_var)
        } else {
            Some(base_var)
        }
    }

    /// Evaluates forward variance strike between expiries T1 and T2 (T2 > T1):
    /// VarSwap_{T1, T2} = (T2 * VarSwap(T2) - T1 * VarSwap(T1)) / (T2 - T1)
    pub fn forward_variance_strike(
        &self,
        t1: f64,
        var_strike_t1: f64,
        t2: f64,
        var_strike_t2: f64,
    ) -> Option<f64> {
        let dt = t2 - t1;
        if dt <= 1e-4 || t1 <= 0.0 || var_strike_t1 < 0.0 || var_strike_t2 < 0.0 {
            return None;
        }
        let fwd_var = (t2 * var_strike_t2 - t1 * var_strike_t1) / dt;
        if fwd_var.is_finite() && fwd_var >= 0.0 {
            Some(fwd_var)
        } else {
            Some(0.0)
        }
    }

    /// Evaluates forward Volatility Risk Premium: VRP_{T1, T2} = sqrt(VarSwap_{T1, T2}) - RV_{fwd}
    pub fn forward_vrp(&self, fwd_var_strike: f64, expected_forward_rv: f64) -> f64 {
        let fwd_iv = fwd_var_strike.max(0.0).sqrt();
        fwd_iv - expected_forward_rv.max(0.0)
    }

    /// Evaluates Calendar VRP term structure trading opportunity across front-month (T1) and back-month (T2).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_calendar_vrp_spread(
        &self,
        symbol: &str,
        t1: f64,
        var_strike_t1: f64,
        rv_t1: f64,
        t2: f64,
        var_strike_t2: f64,
        rv_t2: f64,
        calendar_hurdle: f64,
    ) -> CalendarVrpOpportunity {
        let vrp_front = self.compute_vrp(var_strike_t1, rv_t1);
        let vrp_back = self.compute_vrp(var_strike_t2, rv_t2);
        let fwd_var = self
            .forward_variance_strike(t1, var_strike_t1, t2, var_strike_t2)
            .unwrap_or(var_strike_t2);
        let fwd_vrp = self.forward_vrp(fwd_var, rv_t2);

        // Calendar spread = Front VRP - Back VRP
        let calendar_spread = vrp_front - vrp_back;
        let hurdle = calendar_hurdle.max(0.005);

        let action = if calendar_spread >= hurdle {
            CalendarVrpAction::ShortFrontLongBackCalendar
        } else if calendar_spread <= -hurdle {
            CalendarVrpAction::LongFrontShortBackCalendar
        } else {
            CalendarVrpAction::HoldNeutral
        };

        CalendarVrpOpportunity {
            symbol: symbol.to_string(),
            t1,
            t2,
            front_vrp: vrp_front,
            back_vrp: vrp_back,
            forward_variance_strike: fwd_var,
            forward_vrp: fwd_vrp,
            calendar_spread,
            action,
        }
    }

    /// Evaluates the Whalley-Wilmott (1997) optimal non-intervention delta rehedging bandwidth
    /// for a variance swap log-contract strip:
    /// Gamma_{strip}(S) = (2 * e^{rT}) / (T * S^2)
    /// H_{WW} = ( (3 * c_{perp} * S * Gamma_{strip}^2) / (2 * lambda_{risk} * sigma^2) )^{1/3}
    pub fn whalley_wilmott_rehedge_bandwidth(
        &self,
        spot_price: f64,
        time_to_expiry_years: f64,
        volatility: f64,
        perp_fee_cost_rate: f64,
        risk_aversion: f64,
    ) -> f64 {
        let s = spot_price.max(1.0);
        let t = time_to_expiry_years.max(1e-4);
        let sigma = volatility.max(0.01);
        let c = perp_fee_cost_rate.clamp(1e-5, 0.05);
        let lambda = risk_aversion.clamp(1e-4, 10.0);

        let discount = (self.risk_free_rate * t).exp();
        let gamma_strip = (2.0 * discount) / (t * s * s);

        let num = 3.0 * c * s * gamma_strip * gamma_strip;
        let denom = 2.0 * lambda * sigma * sigma;

        if denom <= 1e-12 {
            return 0.05;
        }

        (num / denom).powf(1.0 / 3.0).clamp(1e-4, 2.0)
    }

    /// Evaluates whether a perpetual delta re-hedge is required, returning the signed delta rebalance size if triggered.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_perp_rehedge_trigger(
        &self,
        spot_price: f64,
        forward_price: f64,
        time_to_expiry_years: f64,
        volatility: f64,
        current_perp_hedge_qty: f64,
        perp_fee_cost_rate: f64,
        risk_aversion: f64,
    ) -> Option<f64> {
        let s = spot_price.max(1.0);
        let f0 = forward_price.max(1.0);
        let t = time_to_expiry_years.max(1e-4);
        let discount = (self.risk_free_rate * t).exp();

        // Target delta of variance swap log contract strip: Delta_{strip} = e^{rT} * (1/F0 - 1/S)
        let target_delta = discount * (1.0 / f0 - 1.0 / s);
        // Current hedge tracking error
        let delta_error = current_perp_hedge_qty - target_delta;
        let h_ww = self.whalley_wilmott_rehedge_bandwidth(
            s,
            t,
            volatility,
            perp_fee_cost_rate,
            risk_aversion,
        );

        if delta_error.abs() > h_ww {
            // Rehedge back to target
            Some(-delta_error)
        } else {
            None
        }
    }
}

/// Raw SVI (Stochastic Volatility Inspired) parameterization:
/// w(k) = a + b * (rho * (k - m) + sqrt((k - m)^2 + sigma^2))
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SviParameters {
    /// Baseline total variance level (a).
    pub a: f64,
    /// Angle between asymptotes / ATM curvature (b >= 0).
    pub b: f64,
    /// Orientation / skew asymmetry (rho in (-1, 1)).
    pub rho: f64,
    /// Horizontal translation / log-moneyness shift (m).
    pub m: f64,
    /// ATM smoothness parameter (sigma > 0).
    pub sigma: f64,
}

impl Default for SviParameters {
    fn default() -> Self {
        Self {
            a: 0.04,
            b: 0.15,
            rho: -0.25,
            m: 0.0,
            sigma: 0.10,
        }
    }
}

impl SviParameters {
    pub fn new(a: f64, b: f64, rho: f64, m: f64, sigma: f64) -> Self {
        Self {
            a,
            b: b.max(1e-6),
            rho: rho.clamp(-0.999, 0.999),
            m,
            sigma: sigma.max(1e-6),
        }
    }

    /// Evaluates total implied variance w(k) = sigma^2(k, tau) * tau
    pub fn total_variance(&self, k: f64) -> f64 {
        let diff = k - self.m;
        let rad = (diff * diff + self.sigma * self.sigma).sqrt();
        (self.a + self.b * (self.rho * diff + rad)).max(1e-8)
    }

    /// Evaluates annualized implied volatility sigma(k, tau)
    pub fn implied_volatility(&self, k: f64, tau_years: f64) -> f64 {
        let w = self.total_variance(k);
        let tau = tau_years.max(1e-6);
        (w / tau).sqrt()
    }

    /// Evaluates ATM implied volatility (k = 0).
    pub fn atm_volatility(&self, tau_years: f64) -> f64 {
        self.implied_volatility(0.0, tau_years)
    }

    /// Evaluates local skew slope at the money: d sigma / d k |_{k=0}
    pub fn atm_skew_slope(&self, tau_years: f64) -> f64 {
        let tau = tau_years.max(1e-6);
        let w0 = self.total_variance(0.0);
        let rad = (self.m * self.m + self.sigma * self.sigma).sqrt();
        let dw_dk = self.b * (self.rho - self.m / rad);
        (dw_dk / (2.0 * (w0 * tau).sqrt())).clamp(-10.0, 10.0)
    }

    /// Computes 25-delta Risk Reversal (RR25 = IV(25C) - IV(25P))
    pub fn risk_reversal_25(&self, tau_years: f64) -> f64 {
        let iv_call = self.implied_volatility(0.15, tau_years);
        let iv_put = self.implied_volatility(-0.15, tau_years);
        iv_call - iv_put
    }

    /// Computes 25-delta Butterfly (BF25 = 0.5 * (IV(25C) + IV(25P)) - IV(ATM))
    pub fn butterfly_25(&self, tau_years: f64) -> f64 {
        let iv_call = self.implied_volatility(0.15, tau_years);
        let iv_put = self.implied_volatility(-0.15, tau_years);
        let iv_atm = self.atm_volatility(tau_years);
        0.5 * (iv_call + iv_put) - iv_atm
    }
}

/// Action recommendation for calendar variance term structure trading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarVrpAction {
    /// Sell front-month rich implied variance, buy back-month underpriced variance
    ShortFrontLongBackCalendar,
    /// Buy front-month underpriced variance, sell back-month rich variance
    LongFrontShortBackCalendar,
    /// Term structure spread within fair range: hold flat
    HoldNeutral,
}

/// Evaluated calendar VRP term structure opportunity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarVrpOpportunity {
    pub symbol: String,
    pub t1: f64,
    pub t2: f64,
    pub front_vrp: f64,
    pub back_vrp: f64,
    pub forward_variance_strike: f64,
    pub forward_vrp: f64,
    pub calendar_spread: f64,
    pub action: CalendarVrpAction,
}

/// Action recommendation for variance dispersion arbitrage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarianceDispersionAction {
    /// Sell constituent variance (ETH) and buy benchmark variance (BTC)
    SellConstituentBuyBenchmark,
    /// Sell benchmark variance (BTC) and buy constituent variance (ETH)
    SellBenchmarkBuyConstituent,
    /// Spread within fair range: hold flat
    HoldNeutral,
}

/// Evaluated variance dispersion opportunity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarianceDispersionOpportunity {
    pub benchmark_symbol: String,
    pub constituent_symbol: String,
    pub benchmark_vrp: f64,
    pub constituent_vrp: f64,
    pub vrp_spread: f64,
    pub action: VarianceDispersionAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discrete_variance_strike_replication() {
        let replicator = VarianceSwapReplicator::new(0.04);
        let f0 = 50_000.0;
        let t_exp = 30.0 / 365.25; // ~0.0821 years

        // Construct synthetic option strip
        let strikes: [f64; 5] = [40_000.0, 45_000.0, 50_000.0, 55_000.0, 60_000.0];
        let mut options = Vec::new();
        for &k in &strikes {
            let is_call = k >= f0;
            let price = if is_call {
                f64::max(f0 - k, 0.0) + 1500.0
            } else {
                f64::max(k - f0, 0.0) + 1500.0
            };
            options.push(DiscreteOptionQuote::new(
                k,
                price * 0.99,
                price * 1.01,
                is_call,
            ));
        }

        let var_strike_opt = replicator.replicate_variance_strike(f0, t_exp, options);
        assert!(var_strike_opt.is_some(), "Should replicate variance strike");
        let var_strike = var_strike_opt.unwrap();
        assert!(
            var_strike > 0.0,
            "Variance strike must be positive: {var_strike}"
        );

        let iv = var_strike.sqrt();
        assert!(
            iv > 0.10 && iv < 2.0,
            "Implied vol must be reasonable: {iv}"
        );
    }

    #[test]
    fn test_vrp_and_variance_dispersion_decision() {
        let replicator = VarianceSwapReplicator::new(0.0);

        // BTC: IV = 55%, RV = 50% => VRP = +5%
        let var_btc = 0.55 * 0.55;
        let rv_btc = 0.50;

        // ETH: IV = 75%, RV = 60% => VRP = +15%
        let var_eth = 0.75 * 0.75;
        let rv_eth = 0.60;

        let opp = replicator.evaluate_variance_dispersion(
            "BTC", var_btc, rv_btc, "ETH", var_eth, rv_eth, 0.05, // 5% hurdle
        );

        // VRP spread = 15% - 5% = +10% >= 5% hurdle => Sell ETH (rich), Buy BTC (cheap)
        assert_eq!(
            opp.action,
            VarianceDispersionAction::SellConstituentBuyBenchmark
        );
        assert!((opp.vrp_spread - 0.10).abs() < 1e-4);
    }

    #[test]
    fn test_svi_tail_extrapolation() {
        let replicator = VarianceSwapReplicator::new(0.04);
        let f0 = 50_000.0;
        let t_exp = 30.0 / 365.25;

        let strikes: [f64; 5] = [45_000.0, 48_000.0, 50_000.0, 52_000.0, 55_000.0];
        let mut options = Vec::new();
        for &k in &strikes {
            let is_call = k >= f0;
            let price = if is_call {
                f64::max(f0 - k, 0.0) + 1200.0
            } else {
                f64::max(k - f0, 0.0) + 1200.0
            };
            options.push(DiscreteOptionQuote::new(
                k,
                price * 0.99,
                price * 1.01,
                is_call,
            ));
        }

        let base_var = replicator
            .replicate_variance_strike(f0, t_exp, options.clone())
            .unwrap();
        let svi = SviParameters::new(0.04, 0.15, -0.20, 0.0, 0.10);
        let tail_var = replicator
            .replicate_variance_strike_svi_tails(f0, t_exp, options, &svi)
            .unwrap();

        // Extrapolated tail adds positive variance from deep OTM wings
        assert!(
            tail_var >= base_var,
            "Tail extrapolated variance ({tail_var}) must be >= base variance ({base_var})"
        );
    }

    #[test]
    fn test_forward_variance_and_calendar_vrp() {
        let replicator = VarianceSwapReplicator::new(0.04);
        let t1 = 30.0 / 365.25;
        let t2 = 60.0 / 365.25;

        let var_t1 = 0.60 * 0.60; // 60% IV
        let var_t2 = 0.55 * 0.55; // 55% IV

        let fwd_var = replicator
            .forward_variance_strike(t1, var_t1, t2, var_t2)
            .unwrap();
        assert!(
            fwd_var >= 0.0,
            "Forward variance strike must be non-negative: {fwd_var}"
        );

        // Calendar VRP: Front IV = 60%, RV = 45% => VRP_1 = +15%
        // Back IV = 55%, RV = 50% => VRP_2 = +5%
        // Calendar spread = +10% >= 2% hurdle => Short Front, Long Back
        let cal_opp = replicator
            .evaluate_calendar_vrp_spread("BTC", t1, var_t1, 0.45, t2, var_t2, 0.50, 0.02);
        assert_eq!(
            cal_opp.action,
            CalendarVrpAction::ShortFrontLongBackCalendar
        );
        assert!((cal_opp.calendar_spread - 0.10).abs() < 1e-4);
    }

    #[test]
    fn test_whalley_wilmott_rehedge_bandwidth() {
        let replicator = VarianceSwapReplicator::new(0.04);
        let spot = 50_000.0;
        let forward = 50_200.0;
        let t = 0.25;
        let vol = 0.60;
        let fee_rate = 0.0005; // 5 bps
        let risk_aversion = 0.05;

        let h_ww =
            replicator.whalley_wilmott_rehedge_bandwidth(spot, t, vol, fee_rate, risk_aversion);
        assert!(
            h_ww > 0.0 && h_ww < 1.0,
            "H_WW bandwidth must be reasonable: {h_ww}"
        );

        // Check rehedge trigger with small error (below band) => None
        let trig_none = replicator.evaluate_perp_rehedge_trigger(
            spot,
            forward,
            t,
            vol,
            0.0,
            fee_rate,
            risk_aversion,
        );
        // Target delta = e^{0.04*0.25} * (1/50200 - 1/50000) ~= -8.04e-8 (very small)
        assert!(trig_none.is_none());

        // Check rehedge trigger with large error (above band) => Some
        let trig_some = replicator.evaluate_perp_rehedge_trigger(
            spot,
            forward,
            t,
            vol,
            1.5,
            fee_rate,
            risk_aversion,
        );
        assert!(trig_some.is_some());
    }
}
