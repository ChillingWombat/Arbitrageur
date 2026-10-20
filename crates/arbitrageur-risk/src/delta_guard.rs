use arbitrageur_core::position::Position;
use arbitrageur_core::types::{dec, Decimal, Side};
use arbitrageur_math::black_scholes::{black_scholes_greeks, OptionType};
use std::collections::HashMap;
use tracing::warn;

/// f64 -> Decimal through shortest string rendering. `None` on non-finite values.
pub(crate) fn f64_to_dec_opt(value: f64) -> Option<Decimal> {
    if value.is_finite() {
        value.to_string().parse().ok()
    } else {
        None
    }
}

/// Portfolio delta and multi-Greek neutrality tracker.
#[derive(Debug, Clone)]
pub struct DeltaGuard {
    /// Maximum allowed unhedged notional in USD per base asset
    max_unhedged_usd_per_asset: Decimal,
    /// Optional maximum allowed analytical portfolio Gamma per asset (units: 1/USD)
    pub max_portfolio_gamma_per_asset: Option<Decimal>,
    /// Optional maximum allowed analytical portfolio Vega per asset (USD per 1% vol)
    pub max_portfolio_vega_per_asset: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GreekLimitBreach {
    Delta {
        asset: String,
        net_usd: Decimal,
        limit_usd: Decimal,
    },
    Gamma {
        asset: String,
        net_gamma: Decimal,
        limit: Decimal,
    },
    Vega {
        asset: String,
        net_vega: Decimal,
        limit: Decimal,
    },
}

/// Provider interface for dynamic volatility surface / SABR smile lookups.
pub trait VolatilitySmileProvider: Send + Sync {
    fn get_implied_volatility(&self, asset: &str, expiry: &str, strike: f64) -> Option<f64>;

    /// Returns the SVI smile-adjusted Black-Scholes delta when a calibrated
    /// slice is available for the option.
    fn get_svi_smile_adjusted_delta(
        &self,
        _underlying: &str,
        _strike: f64,
        _time_to_expiry_years: f64,
        _spot: f64,
        _bs_delta: f64,
        _bs_vega_raw: f64,
    ) -> Option<f64> {
        None
    }
}

/// Aggregated multi-asset portfolio risk report with cross-asset correlation & beta.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrossAssetRiskReport {
    pub asset_deltas: HashMap<String, Decimal>,
    pub asset_gammas: HashMap<String, Decimal>,
    pub btc_equivalent_delta: Decimal,
    pub cross_asset_gamma_risk: Decimal,
    pub is_portfolio_neutral: bool,
}

/// State of an asset pair for multi-asset cross-gamma rehedging evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrossAssetPairState {
    pub spot_i: Decimal,
    pub spot_j: Decimal,
    pub vol_i: Decimal,
    pub vol_j: Decimal,
    pub gamma_ii: Decimal,
    pub gamma_ij: Decimal,
    pub correlation_ij: Decimal,
}

impl DeltaGuard {
    pub fn new(max_unhedged_usd_per_asset: Decimal) -> Self {
        Self {
            max_unhedged_usd_per_asset,
            max_portfolio_gamma_per_asset: None,
            max_portfolio_vega_per_asset: None,
        }
    }

    pub fn with_greek_limits(
        max_unhedged_usd_per_asset: Decimal,
        max_gamma: Option<Decimal>,
        max_vega: Option<Decimal>,
    ) -> Self {
        Self {
            max_unhedged_usd_per_asset,
            max_portfolio_gamma_per_asset: max_gamma,
            max_portfolio_vega_per_asset: max_vega,
        }
    }

    pub fn set_max_gamma(&mut self, limit: Option<Decimal>) {
        self.max_portfolio_gamma_per_asset = limit;
    }

    pub fn set_max_vega(&mut self, limit: Option<Decimal>) {
        self.max_portfolio_vega_per_asset = limit;
    }

    /// Get current max unhedged USD exposure limit
    pub fn max_unhedged_usd_per_asset(&self) -> Decimal {
        self.max_unhedged_usd_per_asset
    }

    /// Set max unhedged USD exposure limit
    pub fn set_max_unhedged_usd_per_asset(&mut self, limit: Decimal) {
        self.max_unhedged_usd_per_asset = limit;
    }

    /// Evaluates whether an endogenous volatility jump requires defensive Greek bandwidth contraction.
    pub fn evaluate_jump_bandwidth_scaling(
        &self,
        jump_assessment: &arbitrageur_math::volatility_jump::JumpAssessment,
    ) -> f64 {
        if jump_assessment.is_jump || jump_assessment.relative_jump_ratio >= 0.40 {
            // Contraction factor: tighten delta and gamma limits by up to 50% during jump cascades
            (1.0 - 0.5 * jump_assessment.relative_jump_ratio).clamp(0.40, 1.0)
        } else {
            1.0
        }
    }

    /// Aggregates positions across all venues by base currency and returns net delta and USD exposure.
    /// Signed base quantities are netted first, then the residual quantity is
    /// valued at the maximum positive mark observed for that base, so equal
    /// long/short quantities remain delta-neutral even when venue marks differ.
    ///
    /// Option legs are parity-netted per (expiry, strike): put-call parity
    /// makes `Δ_call − Δ_put = 1` identically, so an opposite-sign C/P pair
    /// contributes exactly ±1 per unit of base regardless of greeks — a
    /// conversion (short C + long P) is exactly −1, matching the +1 perp leg
    /// it hedges. Unmatched option residuals count ±1 by type (call +1 /
    /// put −1) — the deep-ITM bound, conservative for guard purposes.
    pub fn calculate_net_delta(
        &self,
        positions: &[Position],
    ) -> HashMap<String, (Decimal, Decimal)> {
        let mut delta_map: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        let mut max_mark: HashMap<String, Decimal> = HashMap::new();
        // (base, expiry, strike) -> (net signed call qty, net signed put qty)
        let mut opt_pairs: HashMap<(String, String, String), (Decimal, Decimal)> = HashMap::new();

        let mut opt_strikes: HashMap<String, Vec<Decimal>> = HashMap::new();
        let mut long_call_premium_usd: HashMap<String, Decimal> = HashMap::new();
        let mut long_put_premium_usd: HashMap<String, Decimal> = HashMap::new();

        for pos in positions {
            let base = pos
                .symbol
                .split('-')
                .next()
                .unwrap_or(&pos.symbol)
                .to_uppercase();
            delta_map.entry(base.clone()).or_insert((dec!(0), dec!(0)));

            let opt_key = parse_option_key(&pos.symbol);
            if let Some((expiry, strike_str, is_call)) = opt_key {
                if let Ok(strike) = strike_str.parse::<Decimal>() {
                    opt_strikes.entry(base.clone()).or_default().push(strike);
                }
                // Track long option premium value separately for calls and puts
                if pos.side == Side::Buy && pos.mark_price > dec!(0) {
                    if is_call {
                        *long_call_premium_usd.entry(base.clone()).or_insert(dec!(0)) +=
                            pos.quantity * pos.mark_price;
                    } else {
                        *long_put_premium_usd.entry(base.clone()).or_insert(dec!(0)) +=
                            pos.quantity * pos.mark_price;
                    }
                }
                let e = opt_pairs
                    .entry((base, expiry, strike_str))
                    .or_insert((dec!(0), dec!(0)));
                if is_call {
                    e.0 += pos.signed_quantity();
                } else {
                    e.1 += pos.signed_quantity();
                }
            } else {
                if pos.mark_price > dec!(0) {
                    let m = max_mark.entry(base.clone()).or_insert(dec!(0));
                    if pos.mark_price > *m {
                        *m = pos.mark_price;
                    }
                }
                delta_map.entry(base).or_insert((dec!(0), dec!(0))).0 += pos.signed_quantity();
            }
        }

        // If base asset has no spot/perp mark, derive asset price from option strikes
        for (base, mut strikes) in opt_strikes {
            if !max_mark.contains_key(&base) && !strikes.is_empty() {
                strikes.sort();
                let median_strike = strikes[strikes.len() / 2];
                max_mark.insert(base, median_strike);
            }
        }

        for ((base, _, _), (calls, puts)) in opt_pairs {
            // Opposite-sign pairing: a synthetic forward is C − P. Where call
            // and put nets oppose, the matched portion is exactly ±m of base
            // delta — venue greeks cannot improve on the parity identity.
            // `paired` is signed: negative = conversion-like (short C + long
            // P → −m), positive = reversal-like (long C + short P → +m).
            let paired = if calls < dec!(0) && puts > dec!(0) {
                -(-calls).min(puts)
            } else if calls > dec!(0) && puts < dec!(0) {
                calls.min(-puts)
            } else {
                dec!(0)
            };
            let (res_calls, res_puts) = (calls - paired, puts + paired);
            let contribution = paired + res_calls - res_puts;
            if contribution != dec!(0) {
                delta_map.entry(base).or_insert((dec!(0), dec!(0))).0 += contribution;
            }
        }

        for (base, entry) in delta_map.iter_mut() {
            let asset_px = max_mark.get(base).copied().unwrap_or(dec!(0));
            let raw_exposure = entry.0 * asset_px;
            // If net delta is positive and composed of long options, the maximum downside
            // loss is strictly bounded by the call option premium paid.
            if entry.0 > dec!(0) {
                if let Some(&prem) = long_call_premium_usd.get(base) {
                    if prem > dec!(0) && raw_exposure > prem {
                        entry.1 = prem;
                        continue;
                    }
                }
            } else if entry.0 < dec!(0) {
                if let Some(&prem) = long_put_premium_usd.get(base) {
                    if prem > dec!(0) && raw_exposure.abs() > prem {
                        entry.1 = -prem;
                        continue;
                    }
                }
            }
            entry.1 = raw_exposure;
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

    /// Pre-trade check for a proposed two-leg pair: aggregates signed base
    /// quantities (Buy positive, Sell negative) by uppercase base asset, then
    /// values each residual quantity at that base's maximum leg price. Rejects
    /// when any absolute exposure exceeds `max_unhedged_usd_per_asset`.
    /// Nonpositive quantities or prices are rejected as that leg's base with a
    /// sentinel exposure exceeding the limit.
    #[allow(clippy::too_many_arguments)]
    pub fn check_proposed_pair(
        &self,
        symbol_a: &str,
        side_a: Side,
        qty_a: Decimal,
        price_a: Decimal,
        symbol_b: &str,
        side_b: Side,
        qty_b: Decimal,
        price_b: Decimal,
    ) -> Result<(), (String, Decimal)> {
        let sentinel = self.max_unhedged_usd_per_asset.abs() + dec!(1);

        let base_a = symbol_a
            .split('-')
            .next()
            .unwrap_or(symbol_a)
            .to_uppercase();
        if qty_a <= dec!(0) || price_a <= dec!(0) {
            return Err((base_a, sentinel));
        }
        let base_b = symbol_b
            .split('-')
            .next()
            .unwrap_or(symbol_b)
            .to_uppercase();
        if qty_b <= dec!(0) || price_b <= dec!(0) {
            return Err((base_b, sentinel));
        }

        let signed_a = match side_a {
            Side::Buy => qty_a,
            Side::Sell => -qty_a,
        };
        let signed_b = match side_b {
            Side::Buy => qty_b,
            Side::Sell => -qty_b,
        };

        let mut net_qty: HashMap<String, Decimal> = HashMap::new();
        let mut max_px: HashMap<String, Decimal> = HashMap::new();
        for (base, signed, price) in [(base_a, signed_a, price_a), (base_b, signed_b, price_b)] {
            *net_qty.entry(base.clone()).or_insert(dec!(0)) += signed;
            let m = max_px.entry(base).or_insert(dec!(0));
            if price > *m {
                *m = price;
            }
        }

        for (base, qty) in net_qty {
            let usd_exposure = qty * max_px.get(&base).copied().unwrap_or(dec!(0));
            if usd_exposure.abs() > self.max_unhedged_usd_per_asset {
                warn!(
                    "Proposed pair breaches delta neutrality on asset {}: Net USD exposure ${:.2} (limit: ${:.2})",
                    base, usd_exposure, self.max_unhedged_usd_per_asset
                );
                return Err((base, usd_exposure));
            }
        }

        Ok(())
    }

    /// Calculate multi-asset correlation-weighted portfolio dollar delta.
    /// Incorporates cross-asset correlation (default rho = 0.85 between BTC and ETH)
    /// to prevent masked directional exposure from correlated coin positions:
    /// Delta_port = sqrt(max(0, Delta_BTC^2 + Delta_ETH^2 + 2*rho*Delta_BTC*Delta_ETH + sum_others Delta_i^2))
    pub fn calculate_correlated_portfolio_delta(
        &self,
        positions: &[Position],
        correlation_btc_eth: Option<Decimal>,
    ) -> (HashMap<String, (Decimal, Decimal)>, Decimal) {
        let deltas = self.calculate_net_delta(positions);
        let rho = correlation_btc_eth.unwrap_or(dec!(0.85));

        let btc_exposure = deltas.get("BTC").map(|(_, usd)| *usd).unwrap_or(dec!(0));
        let eth_exposure = deltas.get("ETH").map(|(_, usd)| *usd).unwrap_or(dec!(0));

        let mut other_sum_sq = dec!(0);
        for (asset, (_, usd)) in &deltas {
            if asset != "BTC" && asset != "ETH" {
                other_sum_sq += *usd * *usd;
            }
        }

        let cross_term = dec!(2) * rho * btc_exposure * eth_exposure;
        let btc_sq = btc_exposure * btc_exposure;
        let eth_sq = eth_exposure * eth_exposure;
        let total_variance = (btc_sq + eth_sq + cross_term + other_sum_sq).max(dec!(0));

        let port_delta = if total_variance > dec!(0) {
            let f = total_variance.to_string().parse::<f64>().unwrap_or(0.0);
            f64_to_dec_opt(f.sqrt()).unwrap_or(dec!(0))
        } else {
            dec!(0)
        };

        (deltas, port_delta)
    }

    /// Validates both per-asset delta limits and cross-correlated portfolio dollar delta.
    pub fn check_portfolio_delta_neutrality(
        &self,
        positions: &[Position],
        max_correlated_portfolio_usd: Decimal,
        correlation_btc_eth: Option<Decimal>,
    ) -> Result<(), (String, Decimal)> {
        // First check per-asset silos
        self.check_delta_neutrality(positions)?;

        // Next check aggregate correlated exposure
        let (_deltas, port_delta) =
            self.calculate_correlated_portfolio_delta(positions, correlation_btc_eth);
        if port_delta > max_correlated_portfolio_usd {
            warn!(
                "Portfolio correlated delta breach: Total correlated USD exposure ${:.2} (limit: ${:.2})",
                port_delta, max_correlated_portfolio_usd
            );
            return Err(("PORTFOLIO".to_string(), port_delta));
        }

        Ok(())
    }

    /// Computes the closed-form Whalley-Wilmott optimal re-hedging bandwidth:
    /// Delta_S* = S * [ (3 * c_taker) / (2 * gamma * S^2 * |Gamma|) ]^(1/3) * (1 + lambda_J * E[J^2] / sigma^2)^(-1/6)
    /// Delta_Delta* = [ (3 * c_taker * S * |Gamma|^2) / (2 * gamma) ]^(1/3) * (1 + lambda_J * E[J^2] / sigma^2)^(-1/6)
    pub fn compute_whalley_wilmott_bandwidth(
        &self,
        spot_price: Decimal,
        portfolio_gamma: Decimal,
        config: Option<&WhalleyWilmottConfig>,
    ) -> Option<RehedgeBandwidth> {
        let default_cfg = WhalleyWilmottConfig::default();
        let cfg = config.unwrap_or(&default_cfg);

        let s_f64 = spot_price.to_string().parse::<f64>().ok()?;
        let gamma_abs = portfolio_gamma.abs().to_string().parse::<f64>().ok()?;
        let c_f64 = cfg.taker_fee_rate.to_string().parse::<f64>().ok()?;
        let risk_f64 = cfg.risk_aversion.to_string().parse::<f64>().ok()?.max(1e-6);
        let lambda_f64 = cfg.jump_intensity.to_string().parse::<f64>().ok()?;
        let ej2_f64 = cfg.jump_variance.to_string().parse::<f64>().ok()?;
        let sigma_f64 = cfg
            .annual_volatility
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(1e-6);

        let jump_ratio = (lambda_f64 * ej2_f64) / (sigma_f64 * sigma_f64);
        let jump_correction = (1.0 + jump_ratio).powf(-1.0 / 6.0);

        let (delta_s_star, delta_band_width) = if gamma_abs < 1e-6 {
            let max_unhedged_delta_f64 = self
                .max_unhedged_usd_per_asset
                .to_string()
                .parse::<f64>()
                .ok()?
                .max(0.0);
            let fallback = (0.05 * s_f64.max(1.0)).min(max_unhedged_delta_f64);
            let fallback_delta = f64_to_dec_opt(fallback)?;
            (fallback_delta, fallback_delta)
        } else {
            let s_term = (3.0 * c_f64) / (2.0 * risk_f64 * s_f64 * s_f64 * gamma_abs);
            let delta_s_star_f64 = s_f64 * s_term.cbrt() * jump_correction;

            let delta_term = (3.0 * c_f64 * s_f64 * gamma_abs * gamma_abs) / (2.0 * risk_f64);
            let delta_delta_star_f64 = delta_term.cbrt() * jump_correction;
            (
                f64_to_dec_opt(delta_s_star_f64)?,
                f64_to_dec_opt(delta_delta_star_f64)?,
            )
        };

        Some(RehedgeBandwidth {
            delta_s_star,
            delta_band_width,
            lower_price_trigger: (spot_price - delta_s_star).max(dec!(0)),
            upper_price_trigger: spot_price + delta_s_star,
        })
    }

    /// Evaluates if an open position or portfolio requires a dynamic delta re-hedging trade.
    pub fn evaluate_rehedge_trigger(
        &self,
        current_spot: Decimal,
        last_hedged_spot: Decimal,
        current_net_delta: Decimal,
        target_delta: Decimal,
        portfolio_gamma: Decimal,
        config: Option<&WhalleyWilmottConfig>,
    ) -> (bool, Option<Decimal>, Option<String>) {
        let Some(bands) =
            self.compute_whalley_wilmott_bandwidth(current_spot, portfolio_gamma, config)
        else {
            return (false, None, None);
        };

        let spot_drift = (current_spot - last_hedged_spot).abs();
        let delta_drift = (current_net_delta - target_delta).abs();

        if spot_drift >= bands.delta_s_star || delta_drift >= bands.delta_band_width {
            let rehedge_qty = target_delta - current_net_delta;
            let reason = format!(
                "Whalley-Wilmott re-hedge triggered: spot drift ${:.2} (band: ${:.2}) or delta drift {:.4} (band: {:.4})",
                spot_drift, bands.delta_s_star, delta_drift, bands.delta_band_width
            );
            (true, Some(rehedge_qty), Some(reason))
        } else {
            (false, None, None)
        }
    }

    /// Computes the closed-form Multi-Asset Whalley-Wilmott optimal re-hedging bandwidth with effective cross-gamma:
    /// Gamma_eff = | Gamma_ii + Gamma_ij * rho_ij * (S_j * vol_j) / (S_i * vol_i) |
    /// Delta_S* = S_i * [ (3 * c_taker * sigma_i^2) / (2 * gamma * S_i * Gamma_eff) ]^(1/3) * (1 + lambda_J * E[J^2] / sigma^2)^(-1/6)
    pub fn compute_multi_asset_whalley_wilmott_bandwidth(
        &self,
        pair: &CrossAssetPairState,
        config: Option<&WhalleyWilmottConfig>,
    ) -> Option<RehedgeBandwidth> {
        let default_cfg = WhalleyWilmottConfig::default();
        let cfg = config.unwrap_or(&default_cfg);

        let s_i = pair.spot_i.to_string().parse::<f64>().ok()?;
        let s_j = pair.spot_j.to_string().parse::<f64>().ok()?;
        let v_i = pair.vol_i.to_string().parse::<f64>().ok()?.max(1e-4);
        let v_j = pair.vol_j.to_string().parse::<f64>().ok()?.max(1e-4);
        let g_ii = pair.gamma_ii.to_string().parse::<f64>().ok()?;
        let g_ij = pair.gamma_ij.to_string().parse::<f64>().ok()?;
        let rho = pair
            .correlation_ij
            .to_string()
            .parse::<f64>()
            .ok()?
            .clamp(-0.99, 0.99);

        // Effective cross-gamma accounting for price & vol ratio
        let spillover = g_ij * rho * (s_j * v_j) / (s_i * v_i);
        let gamma_eff_f64 = (g_ii + spillover).abs().max(1e-9);

        let c_f64 = cfg.taker_fee_rate.to_string().parse::<f64>().ok()?;
        let risk_f64 = cfg.risk_aversion.to_string().parse::<f64>().ok()?.max(1e-6);
        let lambda_f64 = cfg.jump_intensity.to_string().parse::<f64>().ok()?;
        let ej2_f64 = cfg.jump_variance.to_string().parse::<f64>().ok()?;

        let jump_ratio = (lambda_f64 * ej2_f64) / (v_i * v_i);
        let jump_correction = (1.0 + jump_ratio).powf(-1.0 / 6.0);

        let s_term = (3.0 * c_f64) / (2.0 * risk_f64 * s_i * s_i * gamma_eff_f64);
        let delta_s_star_f64 = s_i * s_term.cbrt() * jump_correction;

        let delta_term = (3.0 * c_f64 * s_i * gamma_eff_f64 * gamma_eff_f64) / (2.0 * risk_f64);
        let delta_delta_star_f64 = delta_term.cbrt() * jump_correction;

        let delta_s_star = Decimal::from_f64_retain(delta_s_star_f64)?;
        let delta_band_width = Decimal::from_f64_retain(delta_delta_star_f64)?;

        Some(RehedgeBandwidth {
            delta_s_star,
            delta_band_width,
            lower_price_trigger: (pair.spot_i - delta_s_star).max(dec!(0)),
            upper_price_trigger: pair.spot_i + delta_s_star,
        })
    }

    /// Evaluates if an open multi-asset options position requires a dynamic delta re-hedging trade.
    pub fn evaluate_multi_asset_rehedge_trigger(
        &self,
        pair: &CrossAssetPairState,
        last_hedged_spot_i: Decimal,
        current_net_delta_i: Decimal,
        target_delta_i: Decimal,
        config: Option<&WhalleyWilmottConfig>,
    ) -> (bool, Option<Decimal>, Option<String>) {
        let Some(bands) = self.compute_multi_asset_whalley_wilmott_bandwidth(pair, config) else {
            return (false, None, None);
        };

        let spot_drift = (pair.spot_i - last_hedged_spot_i).abs();
        let delta_drift = (current_net_delta_i - target_delta_i).abs();

        if spot_drift >= bands.delta_s_star || delta_drift >= bands.delta_band_width {
            let rehedge_qty = target_delta_i - current_net_delta_i;
            let reason = format!(
                "Multi-Asset Whalley-Wilmott re-hedge triggered: spot drift ${:.2} (band: ${:.2}) or delta drift {:.4} (band: {:.4})",
                spot_drift, bands.delta_s_star, delta_drift, bands.delta_band_width
            );
            (true, Some(rehedge_qty), Some(reason))
        } else {
            (false, None, None)
        }
    }

    /// Computes the closed-form Bichuch (2014) & Guasoni-Muhle-Karbe (2013) 2D asymptotic
    /// non-intervention boundaries under proportional fees and quadratic market impact:
    /// Delta delta* = [(3 * c_prop * S * Gamma^2) / (2 * gamma)]^(1/3)
    ///                * [1 + 0.25 * (2 * lambda_quad^3 * Gamma^2 / (gamma * c_prop^2 * S))^(1/6)]^(-1)
    ///                * (1 + lambda_J * E[J^2] / sigma^2)^(-1/6)
    /// g* = [(3 * c_opt * sigma^2) / (2 * gamma * S^2)]^(1/4)
    pub fn compute_bichuch_2d_boundaries(
        &self,
        spot: Decimal,
        portfolio_gamma: Decimal,
        config: Option<&BichuchDeltaGammaConfig>,
    ) -> Option<Bichuch2DBoundaries> {
        let default_cfg = BichuchDeltaGammaConfig::default();
        let cfg = config.unwrap_or(&default_cfg);

        let s_f64 = spot.to_string().parse::<f64>().ok()?;
        if s_f64 <= 0.0 {
            return None;
        }
        let gamma_f64 = portfolio_gamma
            .abs()
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(1e-9);

        let c_prop = cfg
            .prop_cost_rate
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(1e-6);
        let lambda_quad = cfg
            .quad_impact_coeff
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(0.0);
        let c_opt = cfg
            .option_cost_rate
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(1e-6);
        let gamma_risk = cfg.risk_aversion.to_string().parse::<f64>().ok()?.max(1e-6);
        let sigma = cfg
            .implied_volatility
            .to_string()
            .parse::<f64>()
            .ok()?
            .max(1e-4);
        let lambda_j = cfg.jump_intensity.to_string().parse::<f64>().ok()?;
        let ej2 = cfg.jump_variance.to_string().parse::<f64>().ok()?;

        // Jump correction factor
        let jump_ratio = (lambda_j * ej2) / (sigma * sigma);
        let jump_corr = (1.0 + jump_ratio).powf(-1.0 / 6.0);

        // Base Whalley-Wilmott cubic root
        let base_term = (3.0 * c_prop * s_f64 * gamma_f64 * gamma_f64) / (2.0 * gamma_risk);
        let delta_delta_0 = base_term.cbrt();

        // Quadratic market impact correction factor
        let quad_inner = (2.0 * lambda_quad.powi(3) * gamma_f64 * gamma_f64)
            / (gamma_risk * c_prop * c_prop * s_f64);
        let quad_corr = 1.0 / (1.0 + 0.25 * quad_inner.powf(1.0 / 6.0));

        let delta_bandwidth_f64 = delta_delta_0 * quad_corr * jump_corr;
        let delta_bandwidth = Decimal::from_f64_retain(delta_bandwidth_f64)?;

        // Spot bandwidth Delta S* = Delta delta* / Gamma
        let delta_s_star_f64 = delta_bandwidth_f64 / gamma_f64;
        let spot_bandwidth = Decimal::from_f64_retain(delta_s_star_f64)?;

        // Gamma non-intervention boundary g*
        let gamma_term = (3.0 * c_opt * sigma * sigma) / (2.0 * gamma_risk * s_f64 * s_f64);
        let g_star_f64 = gamma_term.powf(0.25);
        let gamma_bandwidth = Decimal::from_f64_retain(g_star_f64)?;

        Some(Bichuch2DBoundaries {
            delta_bandwidth,
            spot_bandwidth,
            gamma_bandwidth,
            lower_price_trigger: (spot - spot_bandwidth).max(dec!(0)),
            upper_price_trigger: spot + spot_bandwidth,
        })
    }

    /// Evaluates 2D rehedging trigger state against Bichuch asymptotic boundaries.
    /// Checks both Delta drift (linear rehedge) and Gamma curvature (options adjustment).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_bichuch_2d_rehedge_trigger(
        &self,
        spot: Decimal,
        last_hedged_spot: Decimal,
        current_delta: Decimal,
        target_delta: Decimal,
        current_gamma: Decimal,
        target_gamma: Decimal,
        config: Option<&BichuchDeltaGammaConfig>,
    ) -> BichuchRehedgeDecision {
        let Some(b) = self.compute_bichuch_2d_boundaries(spot, current_gamma, config) else {
            return BichuchRehedgeDecision::NoAction;
        };

        let spot_drift = (spot - last_hedged_spot).abs();
        let delta_drift = (current_delta - target_delta).abs();
        let gamma_drift = (current_gamma - target_gamma).abs();

        let delta_triggered = delta_drift >= b.delta_bandwidth || spot_drift >= b.spot_bandwidth;
        let gamma_triggered = gamma_drift >= b.gamma_bandwidth;

        match (delta_triggered, gamma_triggered) {
            (true, true) => {
                let rebal_qty = target_delta - current_delta;
                let gamma_err = target_gamma - current_gamma;
                BichuchRehedgeDecision::Both {
                    rebalance_delta: rebal_qty,
                    gamma_error: gamma_err,
                    reason: format!(
                        "2D Bichuch breach: Delta drift {:.4} (band: {:.4}) or spot drift ${:.2} (band: ${:.2}), AND Gamma drift {:.6} (band: {:.6})",
                        delta_drift, b.delta_bandwidth, spot_drift, b.spot_bandwidth, gamma_drift, b.gamma_bandwidth
                    ),
                }
            }
            (true, false) => {
                let rebal_qty = target_delta - current_delta;
                BichuchRehedgeDecision::DeltaRebalance {
                    rebalance_qty: rebal_qty,
                    reason: format!(
                        "Bichuch Delta breach: Delta drift {:.4} (band: {:.4}) or spot drift ${:.2} (band: ${:.2})",
                        delta_drift, b.delta_bandwidth, spot_drift, b.spot_bandwidth
                    ),
                }
            }
            (false, true) => {
                let gamma_err = target_gamma - current_gamma;
                BichuchRehedgeDecision::GammaRebalance {
                    gamma_error: gamma_err,
                    reason: format!(
                        "Bichuch Gamma breach: Gamma drift {:.6} (band: {:.6})",
                        gamma_drift, b.gamma_bandwidth
                    ),
                }
            }
            (false, false) => BichuchRehedgeDecision::NoAction,
        }
    }

    /// Solves the synthetic perpetual delta rebalancing quantity to neutralize
    /// the dollar delta of coin-margined options:
    /// Delta_$ = S_perp * Delta_coin + V_coin
    /// Q_perp* = - (Delta_$ / S_perp) * beta
    pub fn synthetic_perp_rebalance_qty(
        &self,
        coin_delta: Decimal,
        coin_mark_value: Decimal,
        spot_perp: Decimal,
        beta: Decimal,
    ) -> Decimal {
        if spot_perp <= dec!(0) {
            return dec!(0);
        }
        let dollar_delta = spot_perp * coin_delta + coin_mark_value;
        let base_rehedge_qty = -dollar_delta / spot_perp;
        (base_rehedge_qty * beta).round_dp(6)
    }

    /// Evaluates execution fee and funding carry cost to select the optimal perpetual
    /// venue (e.g. Hyperliquid vs Binance vs OKX) for delta rehedging:
    /// Total Cost = Notional * (fee_bps / 10,000) + Notional * funding_rate_8h * (holding_hours / 8.0)
    pub fn evaluate_perpetual_venue_selection(
        &self,
        venues: &[(&str, Decimal, Decimal)], // (venue_name, fee_bps, funding_rate_8h)
        target_notional_usd: Decimal,
        holding_period_hours: f64,
    ) -> Option<String> {
        if venues.is_empty() || target_notional_usd <= dec!(0) {
            return None;
        }
        let holding_ratio =
            Decimal::from_f64_retain(holding_period_hours.max(0.1) / 8.0).unwrap_or(dec!(1.0));

        let mut best_venue = None;
        let mut min_cost = Decimal::MAX;

        for (venue, fee_bps, funding_rate_8h) in venues {
            let fee_cost = target_notional_usd * (*fee_bps / dec!(10000.0));
            let carry_cost = target_notional_usd * *funding_rate_8h * holding_ratio;
            let total_cost = fee_cost + carry_cost;
            if total_cost < min_cost {
                min_cost = total_cost;
                best_venue = Some(venue.to_string());
            }
        }

        best_venue
    }

    /// Evaluates cross-asset portfolio curvature and beta-adjusted BTC-equivalent net delta.
    pub fn evaluate_cross_asset_risk(
        &self,
        greeks: &HashMap<String, PortfolioGreeks>,
        spot_btc: Decimal,
        spot_eth: Decimal,
        beta_eth_btc: Decimal,
        corr_eth_btc: Decimal,
        max_btc_eq_delta_usd: Decimal,
    ) -> CrossAssetRiskReport {
        let delta_btc = greeks.get("BTC").map(|g| g.net_delta).unwrap_or(dec!(0));
        let delta_eth = greeks.get("ETH").map(|g| g.net_delta).unwrap_or(dec!(0));
        let gamma_btc = greeks.get("BTC").map(|g| g.net_gamma).unwrap_or(dec!(0));
        let gamma_eth = greeks.get("ETH").map(|g| g.net_gamma).unwrap_or(dec!(0));

        // Convert ETH delta to BTC-equivalent via beta: Delta_BTC_eq = Delta_BTC + (S_ETH / S_BTC) * beta * Delta_ETH
        let eth_value_ratio = if spot_btc > dec!(0) {
            spot_eth / spot_btc
        } else {
            dec!(0)
        };
        let btc_eq_delta = delta_btc + (delta_eth * eth_value_ratio * beta_eth_btc);

        // Cross-asset quadratic curvature: Gamma_cross = 2 * rho * sqrt(|Gamma_BTC| * |Gamma_ETH|)
        let g_prod = (gamma_btc.abs() * gamma_eth.abs())
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);
        let rho = corr_eth_btc.to_string().parse::<f64>().unwrap_or(0.85);
        let cross_gamma_f64 = 2.0 * rho * g_prod.sqrt();
        let cross_gamma = Decimal::from_f64_retain(cross_gamma_f64).unwrap_or(dec!(0));

        let btc_eq_usd = btc_eq_delta.abs() * spot_btc;
        let is_portfolio_neutral = btc_eq_usd <= max_btc_eq_delta_usd;

        CrossAssetRiskReport {
            asset_deltas: greeks
                .iter()
                .map(|(k, v)| (k.clone(), v.net_delta))
                .collect(),
            asset_gammas: greeks
                .iter()
                .map(|(k, v)| (k.clone(), v.net_gamma))
                .collect(),
            btc_equivalent_delta: btc_eq_delta,
            cross_asset_gamma_risk: cross_gamma,
            is_portfolio_neutral,
        }
    }

    /// Aggregates analytical Black-Scholes Greeks (Delta, Gamma, Vega, Theta) across positions.
    /// Linear positions (spot and perps) contribute Delta = signed_qty, Gamma = 0, Vega = 0, Theta = 0.
    /// Option positions compute real Black-76 Greeks via analytical model.
    pub fn calculate_analytical_greeks(
        &self,
        positions: &[Position],
        default_iv: Option<f64>,
    ) -> HashMap<String, PortfolioGreeks> {
        self.calculate_analytical_greeks_at(positions, default_iv, None)
    }

    /// Aggregates analytical Greeks evaluated at a specific timestamp for deterministic testing.
    pub fn calculate_analytical_greeks_at(
        &self,
        positions: &[Position],
        default_iv: Option<f64>,
        now_ms: Option<i64>,
    ) -> HashMap<String, PortfolioGreeks> {
        self.calculate_analytical_greeks_with_smile(positions, None, default_iv, now_ms)
    }

    /// Aggregates analytical Greeks utilizing a dynamic VolatilitySmileProvider if present.
    pub fn calculate_analytical_greeks_with_smile(
        &self,
        positions: &[Position],
        vol_provider: Option<&dyn VolatilitySmileProvider>,
        default_iv: Option<f64>,
        now_ms: Option<i64>,
    ) -> HashMap<String, PortfolioGreeks> {
        let mut greeks_map: HashMap<String, PortfolioGreeks> = HashMap::new();
        let mut spot_marks: HashMap<String, f64> = HashMap::new();

        // Pass 1: Collect underlying mark prices
        for pos in positions {
            let base = pos
                .symbol
                .split('-')
                .next()
                .unwrap_or(&pos.symbol)
                .to_uppercase();
            if parse_option_key(&pos.symbol).is_none() && pos.mark_price > dec!(0) {
                let px = pos.mark_price.to_string().parse::<f64>().unwrap_or(0.0);
                if px > 0.0 {
                    let entry = spot_marks.entry(base).or_insert(0.0);
                    if px > *entry {
                        *entry = px;
                    }
                }
            }
        }

        let fallback_iv = default_iv.unwrap_or(0.60); // 60% default crypto IV proxy
        let current_now = now_ms.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        // Pass 2: Aggregate analytical Greeks
        for pos in positions {
            let base = pos
                .symbol
                .split('-')
                .next()
                .unwrap_or(&pos.symbol)
                .to_uppercase();
            let entry = greeks_map.entry(base.clone()).or_insert(PortfolioGreeks {
                net_delta: dec!(0),
                net_gamma: dec!(0),
                net_vega: dec!(0),
                net_theta: dec!(0),
            });

            let signed_qty = pos.signed_quantity();
            if let Some((expiry, strike_str, is_call)) = parse_option_key(&pos.symbol) {
                let strike = strike_str.parse::<f64>().unwrap_or(0.0);
                let spot = spot_marks.get(&base).copied().unwrap_or(strike);
                if spot > 0.0 && strike > 0.0 {
                    let opt_type = if is_call {
                        OptionType::Call
                    } else {
                        OptionType::Put
                    };
                    // Dynamic time to expiration derived from symbol, fallback to 14d
                    let t = parse_expiry_tenor_years(&expiry, current_now)
                        .unwrap_or(14.0 / 365.0)
                        .max(1e-4);
                    let iv = vol_provider
                        .and_then(|p| p.get_implied_volatility(&base, &expiry, strike))
                        .unwrap_or(fallback_iv);

                    if let Some(g) = black_scholes_greeks(opt_type, spot, strike, t, 0.0, iv) {
                        let adjusted_delta = vol_provider
                            .and_then(|provider| {
                                provider.get_svi_smile_adjusted_delta(
                                    &base, strike, t, spot, g.delta, g.vega,
                                )
                            })
                            .unwrap_or(g.delta);
                        if let Some(d) = f64_to_dec_opt(adjusted_delta) {
                            entry.net_delta += signed_qty * d;
                        }
                        if let Some(gamma) = f64_to_dec_opt(g.gamma) {
                            entry.net_gamma += signed_qty * gamma;
                        }
                        if let Some(v) = f64_to_dec_opt(g.vega) {
                            entry.net_vega += signed_qty * v;
                        }
                        if let Some(th) = f64_to_dec_opt(g.theta) {
                            entry.net_theta += signed_qty * th;
                        }
                        continue;
                    }
                }
                // Fallback to conservative bound if greeks fail
                let d = if is_call { dec!(1) } else { dec!(-1) };
                entry.net_delta += signed_qty * d;
            } else {
                // Linear spot or perpetual
                entry.net_delta += signed_qty;
            }
        }

        greeks_map
    }

    /// Validates Delta, Gamma, and Vega against configured portfolio limits.
    pub fn check_multi_greek_limits(
        &self,
        positions: &[Position],
        default_iv: Option<f64>,
        now_ms: Option<i64>,
    ) -> Result<(), GreekLimitBreach> {
        // 1. Delta check
        if let Err((asset, net_usd)) = self.check_delta_neutrality(positions) {
            return Err(GreekLimitBreach::Delta {
                asset,
                net_usd,
                limit_usd: self.max_unhedged_usd_per_asset,
            });
        }
        // 2. Analytical Greeks (Gamma & Vega)
        let greeks = self.calculate_analytical_greeks_at(positions, default_iv, now_ms);
        for (asset, g) in greeks {
            if let Some(max_gamma) = self.max_portfolio_gamma_per_asset {
                if g.net_gamma.abs() > max_gamma {
                    warn!(
                        "Portfolio Gamma limit breached on {asset}: {:.6} (limit: {:.6})",
                        g.net_gamma, max_gamma
                    );
                    return Err(GreekLimitBreach::Gamma {
                        asset,
                        net_gamma: g.net_gamma,
                        limit: max_gamma,
                    });
                }
            }
            if let Some(max_vega) = self.max_portfolio_vega_per_asset {
                if g.net_vega.abs() > max_vega {
                    warn!(
                        "Portfolio Vega limit breached on {asset}: ${:.2} (limit: ${:.2})",
                        g.net_vega, max_vega
                    );
                    return Err(GreekLimitBreach::Vega {
                        asset,
                        net_vega: g.net_vega,
                        limit: max_vega,
                    });
                }
            }
        }
        Ok(())
    }

    /// Validates segregated venue solvency barriers for cross-perp skew positions under extreme price shocks.
    #[allow(clippy::too_many_arguments)]
    pub fn verify_cross_perp_skew_margin_solvency(
        &self,
        free_margin_a: f64,
        mmr_a: f64,
        free_margin_b: f64,
        mmr_b: f64,
        notional_usd: f64,
        spot_price: f64,
        is_long_a: bool,
        is_inverse_a: bool,
        is_inverse_b: bool,
        custom_hessian: Option<&arbitrageur_math::margin_deleveraging::SecondOrderMarginHessian>,
    ) -> Result<(), String> {
        let default_hessian =
            arbitrageur_math::margin_deleveraging::SecondOrderMarginHessian::default();
        let hessian = custom_hessian.unwrap_or(&default_hessian);

        // Check Venue A
        hessian.check_venue_solvency_barrier(
            free_margin_a,
            mmr_a,
            notional_usd,
            spot_price,
            is_long_a,
            is_inverse_a,
        )?;

        // Check Venue B
        let is_long_b = !is_long_a;
        hessian.check_venue_solvency_barrier(
            free_margin_b,
            mmr_b,
            notional_usd,
            spot_price,
            is_long_b,
            is_inverse_b,
        )?;

        Ok(())
    }

    /// Evaluates portfolio-wide second-order convexity losses across spot shocks and ensures they do not breach the loss budget.
    pub fn check_second_order_convexity_limits(
        &self,
        net_delta_usd: f64,
        net_gamma_usd: f64,
        spot_price: f64,
        convexity_loss_limit_usd: f64,
    ) -> Result<f64, String> {
        let shocks = [-0.35, -0.25, -0.15, 0.15, 0.25, 0.35];
        let mut max_loss = 0.0;
        let s0 = spot_price.max(1.0);

        for &shock in &shocks {
            let _ds = s0 * shock;
            // Second-order Taylor loss: -(Delta * ds + 0.5 * Gamma * ds^2)
            let pnl = net_delta_usd * shock + 0.5 * net_gamma_usd * shock * shock;
            let loss = -pnl;
            if loss > max_loss {
                max_loss = loss;
            }
        }

        if max_loss > convexity_loss_limit_usd {
            Err(format!(
                "Portfolio second-order convexity loss ${:.2} breaches budget ${:.2}",
                max_loss, convexity_loss_limit_usd
            ))
        } else {
            Ok(max_loss)
        }
    }

    /// Solves joint multi-asset Delta-Gamma-Vega quadratic optimization across liquid hedging instruments
    /// using arbitrageur_math::portfolio_greeks_opt::MultiGreekQpOptimizer.
    pub fn optimize_portfolio_greeks(
        &self,
        asset: &str,
        greeks: &HashMap<String, PortfolioGreeks>,
        instruments: &[arbitrageur_math::portfolio_greeks_opt::HedgingInstrument],
        config: Option<arbitrageur_math::portfolio_greeks_opt::MultiGreekOptimizationConfig>,
    ) -> Option<arbitrageur_math::portfolio_greeks_opt::MultiGreekSolution> {
        let current = greeks.get(asset)?;
        let delta_f64 = current.net_delta.to_string().parse::<f64>().unwrap_or(0.0);
        let gamma_f64 = current.net_gamma.to_string().parse::<f64>().unwrap_or(0.0);
        let vega_f64 = current.net_vega.to_string().parse::<f64>().unwrap_or(0.0);

        let max_delta = self
            .max_unhedged_usd_per_asset
            .to_string()
            .parse::<f64>()
            .unwrap_or(1000.0);
        let max_gamma = self
            .max_portfolio_gamma_per_asset
            .and_then(|g| g.to_string().parse::<f64>().ok())
            .unwrap_or(0.10);
        let max_vega = self
            .max_portfolio_vega_per_asset
            .and_then(|v| v.to_string().parse::<f64>().ok())
            .unwrap_or(100.0);

        let target = arbitrageur_math::portfolio_greeks_opt::MultiGreekPortfolioTarget {
            current_delta: delta_f64,
            current_gamma: gamma_f64,
            current_vega: vega_f64,
            max_delta_tolerance: max_delta,
            max_gamma_tolerance: max_gamma,
            max_vega_tolerance: max_vega,
        };

        let optimizer = arbitrageur_math::portfolio_greeks_opt::MultiGreekQpOptimizer::new(
            config.unwrap_or_default(),
        );

        Some(optimizer.solve(&target, instruments))
    }

    /// Computes the closed-form impulse control optimal delta rebalancing bandwidth:
    /// Delta delta* = (6 * c_fixed * sigma_Delta^2 / (gamma * sigma^2 * S^2))^(1/4)
    ///              + ((3 * c_prop + 2*sqrt(3 * lambda * c_fixed)) / (2 * gamma * sigma^2 * S^2))^(1/3)
    pub fn compute_impulse_rebalance_bandwidth(
        &self,
        spot_price: Decimal,
        basis_vol: Option<f64>,
        spot_vol: Option<f64>,
        fixed_fee_usd: Option<f64>,
        taker_fee_bps: Option<f64>,
    ) -> Option<Decimal> {
        let s = spot_price.to_string().parse::<f64>().ok()?;
        if s <= 0.0 {
            return None;
        }
        let c_fixed = fixed_fee_usd.unwrap_or(1.50).max(0.01);
        let bps = taker_fee_bps.unwrap_or(4.5).max(0.1);
        let c_prop = (bps / 10000.0) * s;
        let sigma_delta = basis_vol.unwrap_or(0.15).max(0.01);
        let sigma_spot = spot_vol.unwrap_or(0.65).max(0.01);
        let gamma_risk = 1e-4_f64;
        let lambda_slip = 1e-5_f64;

        let denom = gamma_risk * (sigma_spot * sigma_spot) * (s * s);
        if denom <= 0.0 {
            return None;
        }

        // Term 1: (6 * c_fixed * sigma_Delta^2 / denom)^(1/4)
        let term1 = ((6.0 * c_fixed * sigma_delta * sigma_delta) / denom).powf(0.25);

        // Term 2: ((3 * c_prop + 2 * sqrt(3 * lambda * c_fixed)) / (2 * denom))^(1/3)
        let inner_slip = 2.0 * (3.0 * lambda_slip * c_fixed).sqrt();
        let term2 = ((3.0 * c_prop + inner_slip) / (2.0 * denom)).cbrt();

        let bandwidth = term1 + term2;
        Decimal::from_f64_retain(bandwidth)
    }

    /// Evaluates multi-Greek second-order Taylor series portfolio PnL across standard macro crash scenarios.
    pub fn stress_test_portfolio_macro_scenarios(
        &self,
        spot_price: Decimal,
        greeks: &PortfolioGreeks,
        scenarios: Option<&[arbitrageur_math::portfolio_greeks_opt::MacroStressScenario]>,
    ) -> Vec<arbitrageur_math::portfolio_greeks_opt::MacroStressResult> {
        let spot = spot_price.to_string().parse::<f64>().unwrap_or(0.0);
        let delta = greeks.net_delta.to_string().parse::<f64>().unwrap_or(0.0);
        let gamma = greeks.net_gamma.to_string().parse::<f64>().unwrap_or(0.0);
        let vega = greeks.net_vega.to_string().parse::<f64>().unwrap_or(0.0);
        let theta = greeks.net_theta.to_string().parse::<f64>().unwrap_or(0.0);
        let rho = 0.0;

        match scenarios {
            Some(scs) => scs
                .iter()
                .map(|sc| {
                    arbitrageur_math::portfolio_greeks_opt::MultiGreekCoVaRStressEngine::evaluate_scenario_pnl(
                        spot, delta, gamma, vega, theta, rho, sc,
                    )
                })
                .collect(),
            None => arbitrageur_math::portfolio_greeks_opt::MultiGreekCoVaRStressEngine::run_standard_macro_suite(
                spot, delta, gamma, vega, theta, rho,
            ),
        }
    }

    /// Computes systemic Conditional Value-at-Risk (\Delta CoVaR^{j|i}) from benchmark asset to target asset.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_systemic_delta_covar(
        &self,
        benchmark_asset: &str,
        target_asset: &str,
        sigma_benchmark: f64,
        sigma_target: f64,
        correlation: f64,
        benchmark_var_99: f64,
        benchmark_median: f64,
    ) -> arbitrageur_math::portfolio_greeks_opt::CoVaRReport {
        arbitrageur_math::portfolio_greeks_opt::MultiGreekCoVaRStressEngine::compute_delta_covar(
            benchmark_asset,
            target_asset,
            sigma_benchmark,
            sigma_target,
            correlation,
            benchmark_var_99,
            benchmark_median,
        )
    }

    /// Evaluates Extreme Value Theory Generalized Pareto Distribution Expected Shortfall ES_{99.9%}.
    pub fn evaluate_tail_evt_expected_shortfall(
        &self,
        threshold_u: f64,
        shape_xi: f64,
        scale_beta: f64,
        exceedance_prob: f64,
        target_quantile: f64,
    ) -> arbitrageur_math::portfolio_greeks_opt::EvtGpdExpectedShortfall {
        arbitrageur_math::portfolio_greeks_opt::MultiGreekCoVaRStressEngine::compute_evt_gpd_expected_shortfall(
            threshold_u,
            shape_xi,
            scale_beta,
            exceedance_prob,
            target_quantile,
        )
    }
}

/// Analytical Black-Scholes portfolio Greek sensitivities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortfolioGreeks {
    pub net_delta: Decimal,
    pub net_gamma: Decimal,
    pub net_vega: Decimal,
    pub net_theta: Decimal,
}

/// Whalley-Wilmott optimal dynamic delta re-hedging band parameters.
#[derive(Debug, Clone)]
pub struct WhalleyWilmottConfig {
    /// Taker trading fee rate c_taker (e.g. 0.0005 for 5 bps taker fee).
    pub taker_fee_rate: Decimal,
    /// Absolute risk aversion parameter gamma (e.g. 0.05).
    pub risk_aversion: Decimal,
    /// Poisson jump arrival intensity lambda_J (e.g. 2.0 jumps per year).
    pub jump_intensity: Decimal,
    /// Second moment of jump size distribution E[J^2] (e.g. 0.04).
    pub jump_variance: Decimal,
    /// Annualized underlying volatility sigma (e.g. 0.70).
    pub annual_volatility: Decimal,
}

impl Default for WhalleyWilmottConfig {
    fn default() -> Self {
        Self {
            taker_fee_rate: dec!(0.0005),
            risk_aversion: dec!(0.05),
            jump_intensity: dec!(2.0),
            jump_variance: dec!(0.04),
            annual_volatility: dec!(0.70),
        }
    }
}

/// Evaluated Whalley-Wilmott re-hedging bandwidth around the current spot and delta.
#[derive(Debug, Clone)]
pub struct RehedgeBandwidth {
    /// Price width threshold Delta S*
    pub delta_s_star: Decimal,
    /// Delta bandwidth threshold Delta Delta*
    pub delta_band_width: Decimal,
    /// Lower trigger price (S - Delta S*)
    pub lower_price_trigger: Decimal,
    /// Upper trigger price (S + Delta S*)
    pub upper_price_trigger: Decimal,
}

/// Configuration parameters for Bichuch (2014) & Guasoni-Muhle-Karbe (2013) 2D Delta-Gamma rehedging.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BichuchDeltaGammaConfig {
    /// Proportional transaction cost c_prop (e.g. 0.0005 for 5 bps taker fee)
    pub prop_cost_rate: Decimal,
    /// Quadratic market impact coefficient lambda_quad (e.g. 1e-6 in 1/USD)
    pub quad_impact_coeff: Decimal,
    /// Option execution transaction cost c_opt (e.g. 0.0005 for 5 bps taker fee on options)
    pub option_cost_rate: Decimal,
    /// Absolute risk aversion gamma (e.g. 0.0001)
    pub risk_aversion: Decimal,
    /// Annualized implied volatility sigma (e.g. 0.65)
    pub implied_volatility: Decimal,
    /// Merton jump arrival intensity lambda_J (e.g. 2.0 jumps/yr)
    pub jump_intensity: Decimal,
    /// Second moment of jump size distribution E[J^2] (e.g. 0.04)
    pub jump_variance: Decimal,
}

impl Default for BichuchDeltaGammaConfig {
    fn default() -> Self {
        Self {
            prop_cost_rate: dec!(0.0005),
            quad_impact_coeff: dec!(0.000001),
            option_cost_rate: dec!(0.0005),
            risk_aversion: dec!(0.0001),
            implied_volatility: dec!(0.65),
            jump_intensity: dec!(2.0),
            jump_variance: dec!(0.04),
        }
    }
}

/// 2D Bichuch Non-Intervention Boundaries in (Delta, Gamma) space.
#[derive(Debug, Clone)]
pub struct Bichuch2DBoundaries {
    /// Optimal Delta tracking error bandwidth Delta delta*
    pub delta_bandwidth: Decimal,
    /// Corresponding underlying spot price bandwidth Delta S* = Delta delta* / |Gamma|
    pub spot_bandwidth: Decimal,
    /// Optimal Gamma non-intervention bandwidth g*
    pub gamma_bandwidth: Decimal,
    /// Lower spot trigger price (S - Delta S*)
    pub lower_price_trigger: Decimal,
    /// Upper spot trigger price (S + Delta S*)
    pub upper_price_trigger: Decimal,
}

/// 2D Rehedging decision outcome
#[derive(Debug, Clone, PartialEq)]
pub enum BichuchRehedgeDecision {
    NoAction,
    DeltaRebalance {
        rebalance_qty: Decimal,
        reason: String,
    },
    GammaRebalance {
        gamma_error: Decimal,
        reason: String,
    },
    Both {
        rebalance_delta: Decimal,
        gamma_error: Decimal,
        reason: String,
    },
}

/// Parse a Deribit-style option symbol (`BTC-27NOV26-112000-C`) into
/// (expiry, strike, is_call). Same four-part shape as the fixed-contract
/// detector used by the samples pipeline: ≥4 dash parts, last part `C`/`P`,
/// penultimate part numeric. Anything else returns `None` and counts as a
/// plain linear position.
fn parse_option_key(symbol: &str) -> Option<(String, String, bool)> {
    let parts: Vec<&str> = symbol.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let is_call = match *parts.last()? {
        "C" => true,
        "P" => false,
        _ => return None,
    };
    let strike = parts[parts.len() - 2];
    strike.parse::<f64>().ok()?;
    // Everything between base and strike is the expiry key — keeps pairs
    // grouped across tenor spellings without validating the date itself.
    let expiry = parts[1..parts.len() - 2].join("-");
    Some((expiry, strike.to_string(), is_call))
}

/// Parse Deribit expiry string like "27NOV26" into time to expiry T in years.
/// Deribit settlements occur at 08:00:00 UTC on the expiry date.
pub fn parse_expiry_tenor_years(expiry_str: &str, now_ms: i64) -> Option<f64> {
    if expiry_str.len() == 7 {
        let day_str = &expiry_str[0..2];
        let mon_str = &expiry_str[2..5].to_ascii_uppercase();
        let yr_str = &expiry_str[5..7];

        let day = day_str.parse::<u32>().ok()?;
        let yr_2digit = yr_str.parse::<i32>().ok()?;
        let year = 2000 + yr_2digit;

        let month = match mon_str.as_str() {
            "JAN" => 1,
            "FEB" => 2,
            "MAR" => 3,
            "APR" => 4,
            "MAY" => 5,
            "JUN" => 6,
            "JUL" => 7,
            "AUG" => 8,
            "SEP" => 9,
            "OCT" => 10,
            "NOV" => 11,
            "DEC" => 12,
            _ => return None,
        };

        let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        let naive_dt = date.and_hms_opt(8, 0, 0)?;
        let expiry_utc =
            chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive_dt, chrono::Utc);
        let expiry_ms = expiry_utc.timestamp_millis();

        let diff_ms = expiry_ms - now_ms;
        if diff_ms <= 0 {
            return Some(60.0 / (365.25 * 86400.0));
        }
        let years = (diff_ms as f64) / (1000.0 * 86400.0 * 365.25);
        return Some(years);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_proposed_pair_balanced_passes() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(60000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_check_proposed_pair_equal_qty_different_prices_passes() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Equal quantities hedge fully even when venue marks differ materially
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(62000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_calculate_net_delta_equal_qty_different_marks() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "v".into(),
            symbol: "BTC-USDC".into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let deltas = guard.calculate_net_delta(&[
            pos(Side::Buy, dec!(1.0), dec!(60000.0)),
            pos(Side::Sell, dec!(1.0), dec!(62000.0)),
        ]);
        let (net_qty, exposure) = deltas["BTC"];
        assert_eq!(net_qty, dec!(0));
        assert_eq!(exposure, dec!(0));
    }

    #[test]
    fn test_conversion_package_nets_to_zero_delta() {
        // Short call + long put at the same strike/expiry + long perp is a
        // delta-neutral conversion. Put-call parity makes the option pair
        // exactly −1 per unit — the +1 perp must cancel it precisely.
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "v".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let deltas = guard.calculate_net_delta(&[
            pos("BTC-27NOV26-112000-C", Side::Sell, dec!(1.0), dec!(274.0)),
            pos("BTC-27NOV26-112000-P", Side::Buy, dec!(1.0), dec!(28586.0)),
            pos("BTC-PERPETUAL", Side::Buy, dec!(0.99983), dec!(82984.0)),
        ]);
        let (qty, _) = deltas["BTC"];
        assert!(qty.abs() < dec!(0.001), "conversion must net ~0, got {qty}");
    }

    #[test]
    fn test_unpaired_option_counts_by_type() {
        // Lone options without a parity partner stay conservative: calls +1,
        // puts −1 per unit (deep-ITM bound). A straddle nets ~0.
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |sym: &str, side: Side, qty: Decimal| Position {
            venue: "v".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: dec!(1),
            mark_price: dec!(50000),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        let lone_call =
            guard.calculate_net_delta(&[pos("BTC-27NOV26-112000-C", Side::Buy, dec!(0.5))]);
        assert_eq!(lone_call["BTC"].0, dec!(0.5));
        let straddle = guard.calculate_net_delta(&[
            pos("BTC-27NOV26-112000-C", Side::Buy, dec!(1.0)),
            pos("BTC-27NOV26-112000-P", Side::Buy, dec!(1.0)),
        ]);
        assert_eq!(straddle["BTC"].0, dec!(0));
    }

    #[test]
    fn test_check_proposed_pair_mismatched_fails() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Buy 1.0 @ 60000 vs sell 0.5 @ 70000 leaves +0.5 BTC valued at max leg price
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(0.5),
            dec!(70000.0),
        );
        let (base, exposure) = res.expect_err("mismatched pair must fail");
        assert_eq!(base, "BTC");
        assert_eq!(exposure, dec!(35000.0));
    }

    #[test]
    fn test_check_proposed_pair_cross_asset_independent() {
        let guard = DeltaGuard::new(dec!(1000.0));
        // Different base assets cannot hedge each other; each leg over limit
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
            dec!(60000.0),
            "ETH-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(3000.0),
        );
        assert!(res.is_err());

        // Small balanced cross-asset pair within limit still passes each asset
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(0.01),
            dec!(60000.0),
            "ETH-USDC",
            Side::Sell,
            dec!(0.1),
            dec!(3000.0),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_check_proposed_pair_nonpositive_rejected() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let res = guard.check_proposed_pair(
            "BTC-USDC",
            Side::Buy,
            dec!(0.0),
            dec!(60000.0),
            "BTC-USDC",
            Side::Sell,
            dec!(1.0),
            dec!(60000.0),
        );
        let (base, exposure) = res.expect_err("zero qty must fail");
        assert_eq!(base, "BTC");
        assert!(exposure.abs() > dec!(1000.0));
    }

    #[test]
    fn test_long_option_exposure_bounded_by_premium() {
        let guard = DeltaGuard::new(dec!(2500.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "deribit".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        // A deep OTM call with premium of $11.41 for 1.0 contract and a $118k put in the portfolio
        let positions = vec![
            pos("BTC-5OCT26-90000-C", Side::Buy, dec!(1.0), dec!(11.41)),
            pos(
                "BTC-25DEC26-200000-P",
                Side::Sell,
                dec!(0.1),
                dec!(118883.72),
            ),
            pos("BTC-25DEC26-200000-C", Side::Buy, dec!(0.1), dec!(5974.37)),
        ];
        let deltas = guard.calculate_net_delta(&positions);
        let (_, exposure) = deltas["BTC"];
        // Exposure must NOT blow up to $130,000+!
        assert!(
            exposure < dec!(2500.0),
            "exposure must be bounded, got {exposure}"
        );
    }

    #[test]
    fn test_box_spread_with_lone_call_not_polluted_by_put_premium() {
        let guard = DeltaGuard::new(dec!(2500.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "deribit".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };
        // Box Spread: K1=125k, K2=160k
        // Long 125k C ($296), Short 160k C ($4368)
        // Short 125k P ($38470), Long 160k P ($74013)
        // Lone Call: 240k C ($3924) with qty 0.1
        let positions = vec![
            pos("BTC-25DEC26-125000-C", Side::Buy, dec!(0.1), dec!(296.90)),
            pos("BTC-25DEC26-160000-C", Side::Sell, dec!(0.1), dec!(4368.72)),
            pos(
                "BTC-25DEC26-125000-P",
                Side::Sell,
                dec!(0.1),
                dec!(38470.22),
            ),
            pos("BTC-25DEC26-160000-P", Side::Buy, dec!(0.1), dec!(74013.82)),
            pos("BTC-25DEC26-240000-C", Side::Buy, dec!(0.1), dec!(3924.15)),
        ];
        let deltas = guard.calculate_net_delta(&positions);
        let (qty, exposure) = deltas["BTC"];
        assert_eq!(qty, dec!(0.1));
        assert!(
            exposure < dec!(2500.0),
            "box spread long put premium must not disable call exposure bound; got {exposure}"
        );
    }

    #[test]
    fn test_correlated_portfolio_delta_and_neutrality_check() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "deribit".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };

        // BTC: +0.05 @ $60,000 = +$3,000 USD
        // ETH: +1.0 @ $3,000 = +$3,000 USD
        // Each asset is individually <= $5,000 limit.
        // But with rho = 0.85:
        // Variance = 3000^2 + 3000^2 + 2 * 0.85 * 3000 * 3000 = 9M + 9M + 15.3M = 33.3M
        // Sqrt(33.3M) = ~$5,770.6 > $5,000 limit!
        let positions = vec![
            pos("BTC-PERPETUAL", Side::Buy, dec!(0.05), dec!(60000.0)),
            pos("ETH-PERPETUAL", Side::Buy, dec!(1.0), dec!(3000.0)),
        ];

        let (_deltas, port_delta) =
            guard.calculate_correlated_portfolio_delta(&positions, Some(dec!(0.85)));
        assert!(
            port_delta > dec!(5700.0) && port_delta < dec!(5800.0),
            "correlated port delta should be ~$5770, got {port_delta}"
        );

        // Individual asset check passes
        assert!(guard.check_delta_neutrality(&positions).is_ok());

        // Portfolio correlated check breaches at $5,000 limit
        let res =
            guard.check_portfolio_delta_neutrality(&positions, dec!(5000.0), Some(dec!(0.85)));
        assert!(res.is_err());
        let (breach_key, breach_amt) = res.unwrap_err();
        assert_eq!(breach_key, "PORTFOLIO");
        assert!(breach_amt > dec!(5700.0));

        // Portfolio correlated check passes at $6,000 limit
        assert!(guard
            .check_portfolio_delta_neutrality(&positions, dec!(6000.0), Some(dec!(0.85)))
            .is_ok());
    }

    #[test]
    fn test_whalley_wilmott_rehedge_bandwidth_and_trigger() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let config = WhalleyWilmottConfig::default();

        // Test S = $60,000, Gamma = 0.0001 (moderate gamma near ATM)
        let spot = dec!(60000.0);
        let gamma = dec!(0.0001);

        let bands = guard
            .compute_whalley_wilmott_bandwidth(spot, gamma, Some(&config))
            .expect("bandwidth computation succeeds");

        assert!(bands.delta_s_star > dec!(0));
        assert!(bands.delta_band_width > dec!(0));
        assert_eq!(bands.upper_price_trigger, spot + bands.delta_s_star);
        assert_eq!(bands.lower_price_trigger, spot - bands.delta_s_star);

        // Within bandwidth: no rehedge triggered
        let (triggered, qty, _) = guard.evaluate_rehedge_trigger(
            spot + dec!(50.0), // $50 drift (well within bandwidth)
            spot,
            dec!(0.01), // small delta drift
            dec!(0.0),  // target delta = 0
            gamma,
            Some(&config),
        );
        assert!(!triggered);
        assert!(qty.is_none());

        // Price drift beyond delta_s_star triggers rehedge
        let large_drift_spot = spot + bands.delta_s_star + dec!(10.0);
        let (triggered, qty, reason) = guard.evaluate_rehedge_trigger(
            large_drift_spot,
            spot,
            dec!(0.05),
            dec!(0.0),
            gamma,
            Some(&config),
        );
        assert!(triggered);
        assert_eq!(qty, Some(dec!(-0.05))); // must sell 0.05 to re-neutralize delta
        assert!(reason
            .unwrap()
            .contains("Whalley-Wilmott re-hedge triggered"));
    }

    #[test]
    fn test_whalley_wilmott_zero_gamma_uses_bounded_fallback() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let bands = guard
            .compute_whalley_wilmott_bandwidth(dec!(60000.0), dec!(0), None)
            .expect("zero-gamma bandwidth must be finite");

        assert_eq!(bands.delta_s_star, dec!(3000.0));
        assert_eq!(bands.delta_band_width, dec!(3000.0));
        assert_eq!(bands.upper_price_trigger, dec!(63000.0));
        assert_eq!(bands.lower_price_trigger, dec!(57000.0));
    }

    #[test]
    fn test_calculate_analytical_greeks_option_butterfly() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "deribit".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };

        // Long 1 Call at 90k, Short 2 Calls at 100k, Long 1 Call at 110k
        // Underlying spot = 95k
        let positions = [
            pos("BTC-PERPETUAL", Side::Buy, dec!(0.0), dec!(95000.0)), // marks spot at 95k
            pos("BTC-27NOV26-90000-C", Side::Buy, dec!(1.0), dec!(7500.0)),
            pos("BTC-27NOV26-100000-C", Side::Sell, dec!(2.0), dec!(2500.0)),
            pos("BTC-27NOV26-110000-C", Side::Buy, dec!(1.0), dec!(700.0)),
        ];

        let greeks = guard.calculate_analytical_greeks(&positions, Some(0.50));
        let btc_greeks = greeks.get("BTC").expect("BTC Greeks present");

        // Non-zero analytical delta and gamma under asymmetric spot (95k vs 100k center)
        assert!(
            btc_greeks.net_delta != dec!(0),
            "Asymmetric butterfly must have non-zero analytical delta"
        );
        assert!(
            btc_greeks.net_gamma != dec!(0),
            "Butterfly body/wing curvature must yield non-zero gamma"
        );
    }

    #[test]
    fn test_compute_impulse_rebalance_bandwidth() {
        let guard = DeltaGuard::new(dec!(1000.0));
        let spot = dec!(65000.0);
        let band = guard
            .compute_impulse_rebalance_bandwidth(
                spot,
                Some(0.15),
                Some(0.65),
                Some(1.50),
                Some(4.5),
            )
            .expect("bandwidth computation succeeds");

        assert!(band > dec!(0), "Impulse band must be positive: got {band}");
        // For $65k BTC, optimal inaction band should be around 0.05 to 0.50 BTC depending on parameters
        assert!(
            band >= dec!(0.01) && band <= dec!(2.0),
            "Band should be economically bounded: {band}"
        );
    }

    #[test]
    fn test_parse_expiry_tenor_years() {
        // Assume now is 2026-10-05 08:00:00 UTC
        let now_dt = chrono::NaiveDate::from_ymd_opt(2026, 10, 5)
            .unwrap()
            .and_hms_opt(8, 0, 0)
            .unwrap();
        let now_utc =
            chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(now_dt, chrono::Utc);
        let now_ms = now_utc.timestamp_millis();

        // 25DEC26 is 81 days ahead
        let tenor = parse_expiry_tenor_years("25DEC26", now_ms).expect("tenor parsed");
        let expected_years = 81.0 / 365.25;
        assert!(
            (tenor - expected_years).abs() < 1e-3,
            "Tenor should match: {tenor} vs {expected_years}"
        );

        // Invalid format returns None
        assert!(parse_expiry_tenor_years("INVALID", now_ms).is_none());
    }

    #[test]
    fn test_check_multi_greek_limits() {
        let guard = DeltaGuard::with_greek_limits(
            dec!(1000.0),
            Some(dec!(0.0001)), // Low gamma limit
            Some(dec!(50.0)),   // Vega limit
        );

        let pos = |sym: &str, side: Side, qty: Decimal, mark: Decimal| Position {
            venue: "deribit".into(),
            symbol: sym.into(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };

        // Positions with heavy gamma (10 ATM calls)
        let positions = [
            pos("BTC-PERPETUAL", Side::Buy, dec!(0.0), dec!(100000.0)),
            pos("BTC-25DEC26-100000-C", Side::Buy, dec!(10.0), dec!(5000.0)),
        ];

        let breach = guard.check_multi_greek_limits(&positions, Some(0.60), None);
        assert!(breach.is_err());
        match breach.unwrap_err() {
            GreekLimitBreach::Delta { .. } | GreekLimitBreach::Gamma { .. } => {}
            other => panic!("Expected Delta or Gamma breach, got {:?}", other),
        }
    }

    #[test]
    fn test_multi_asset_whalley_wilmott_bandwidth() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let spot_btc = dec!(60000.0);
        let spot_eth = dec!(3000.0);
        let vol_btc = dec!(0.60);
        let vol_eth = dec!(0.75);
        let gamma_btc = dec!(0.0001);
        let gamma_cross = dec!(0.0005);
        let correlation = dec!(0.85);

        let pair_state = CrossAssetPairState {
            spot_i: spot_btc,
            spot_j: spot_eth,
            vol_i: vol_btc,
            vol_j: vol_eth,
            gamma_ii: gamma_btc,
            gamma_ij: gamma_cross,
            correlation_ij: correlation,
        };

        let bands = guard
            .compute_multi_asset_whalley_wilmott_bandwidth(&pair_state, None)
            .expect("bandwidth computed");

        assert!(bands.delta_s_star > dec!(0));
        assert!(bands.delta_band_width > dec!(0));
        assert!(bands.lower_price_trigger < spot_btc);
        assert!(bands.upper_price_trigger > spot_btc);

        // Test trigger evaluation when spot drift breaches upper band
        let mut drifted_pair = pair_state;
        drifted_pair.spot_i = spot_btc + bands.delta_s_star + dec!(10.0);
        let (triggered, rehedge_qty, reason) = guard.evaluate_multi_asset_rehedge_trigger(
            &drifted_pair,
            spot_btc,
            dec!(0.25),
            dec!(0.0),
            None,
        );

        assert!(triggered);
        assert_eq!(rehedge_qty, Some(dec!(-0.25)));
        assert!(reason.unwrap().contains("Multi-Asset Whalley-Wilmott"));
    }

    #[test]
    fn test_evaluate_cross_asset_risk() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let mut greeks = HashMap::new();
        greeks.insert(
            "BTC".into(),
            PortfolioGreeks {
                net_delta: dec!(0.10),
                net_gamma: dec!(0.00005),
                net_vega: dec!(120.0),
                net_theta: dec!(-40.0),
            },
        );
        greeks.insert(
            "ETH".into(),
            PortfolioGreeks {
                net_delta: dec!(1.50), // 1.5 ETH delta
                net_gamma: dec!(0.0008),
                net_vega: dec!(80.0),
                net_theta: dec!(-25.0),
            },
        );

        let spot_btc = dec!(60000.0);
        let spot_eth = dec!(3000.0);
        let beta = dec!(1.15);
        let corr = dec!(0.85);
        let max_usd = dec!(15000.0);

        let report =
            guard.evaluate_cross_asset_risk(&greeks, spot_btc, spot_eth, beta, corr, max_usd);

        // ETH value ratio = 3000 / 60000 = 0.05
        // ETH delta in BTC-eq = 1.5 * 0.05 * 1.15 = 0.08625 BTC
        // Total BTC-eq = 0.10 + 0.08625 = 0.18625 BTC ($11,175 USD <= $15,000 max)
        assert!(
            report.btc_equivalent_delta > dec!(0.18) && report.btc_equivalent_delta < dec!(0.19)
        );
        assert!(report.is_portfolio_neutral);
        assert!(report.cross_asset_gamma_risk > dec!(0));
    }

    struct MockSmileProvider;
    impl VolatilitySmileProvider for MockSmileProvider {
        fn get_implied_volatility(&self, asset: &str, _expiry: &str, strike: f64) -> Option<f64> {
            if asset == "BTC" {
                if strike > 60000.0 {
                    Some(0.85) // Rich OTM Call IV
                } else {
                    Some(0.55) // Low ATM/Put IV
                }
            } else {
                None
            }
        }
    }

    #[test]
    fn test_calculate_analytical_greeks_with_smile_provider() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let pos = Position {
            venue: "deribit".into(),
            symbol: "BTC-25DEC26-70000-C".into(),
            side: Side::Buy,
            quantity: dec!(1.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(60000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };

        let provider = MockSmileProvider;
        // Evaluate with smile provider (uses 0.85 IV for strike 70000)
        let greeks_smile = guard.calculate_analytical_greeks_with_smile(
            std::slice::from_ref(&pos),
            Some(&provider),
            None,
            None,
        );
        // Evaluate with flat default IV 0.60
        let greeks_flat = guard.calculate_analytical_greeks(&[pos], Some(0.60));

        let delta_smile = greeks_smile.get("BTC").unwrap().net_delta;
        let delta_flat = greeks_flat.get("BTC").unwrap().net_delta;

        // Higher IV (0.85 vs 0.60) yields significantly higher OTM call delta
        assert!(delta_smile > delta_flat);
    }

    struct MockSviProvider;
    impl VolatilitySmileProvider for MockSviProvider {
        fn get_implied_volatility(&self, _asset: &str, _expiry: &str, _strike: f64) -> Option<f64> {
            Some(0.60)
        }

        fn get_svi_smile_adjusted_delta(
            &self,
            underlying: &str,
            strike: f64,
            _time_to_expiry_years: f64,
            _spot: f64,
            _bs_delta: f64,
            _bs_vega_raw: f64,
        ) -> Option<f64> {
            (underlying == "BTC" && (strike - 70_000.0).abs() < 1e-6).then_some(0.90)
        }
    }

    #[test]
    fn test_svi_smile_adjusted_delta_overrides_black_scholes_delta() {
        let guard = DeltaGuard::new(dec!(5000.0));
        let pos = Position {
            venue: "deribit".into(),
            symbol: "BTC-25DEC26-70000-C".into(),
            side: Side::Buy,
            quantity: dec!(1.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(60000.0),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(1),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: chrono::Utc::now(),
        };

        let greeks = guard.calculate_analytical_greeks_with_smile(
            &[pos],
            Some(&MockSviProvider),
            None,
            None,
        );
        assert_eq!(greeks.get("BTC").unwrap().net_delta, dec!(0.90));
    }

    #[test]
    fn test_bichuch_2d_boundaries_and_rehedge_trigger() {
        let guard = DeltaGuard::new(dec!(10000.0));
        let spot = dec!(60000.0);
        let gamma = dec!(0.0001); // 1e-4 / USD

        let cfg = BichuchDeltaGammaConfig {
            prop_cost_rate: dec!(0.0005),
            quad_impact_coeff: dec!(0.000001),
            option_cost_rate: dec!(0.0005),
            risk_aversion: dec!(0.0001),
            implied_volatility: dec!(0.65),
            jump_intensity: dec!(2.0),
            jump_variance: dec!(0.04),
        };

        let bounds = guard
            .compute_bichuch_2d_boundaries(spot, gamma, Some(&cfg))
            .expect("bichuch boundaries must compute");

        assert!(bounds.delta_bandwidth > dec!(0));
        assert!(bounds.spot_bandwidth > dec!(0));
        assert!(bounds.gamma_bandwidth > dec!(0));

        // Within boundaries: No action
        let decision_quiet = guard.evaluate_bichuch_2d_rehedge_trigger(
            spot,
            spot,
            dec!(0.01),
            dec!(0.0),
            gamma,
            gamma,
            Some(&cfg),
        );
        assert_eq!(decision_quiet, BichuchRehedgeDecision::NoAction);

        // Delta drift exceeding bandwidth: DeltaRebalance
        let excessive_delta = bounds.delta_bandwidth * dec!(1.5);
        let decision_delta = guard.evaluate_bichuch_2d_rehedge_trigger(
            spot,
            spot,
            excessive_delta,
            dec!(0.0),
            gamma,
            gamma,
            Some(&cfg),
        );
        match decision_delta {
            BichuchRehedgeDecision::DeltaRebalance { rebalance_qty, .. } => {
                assert_eq!(rebalance_qty, -excessive_delta);
            }
            other => panic!("Expected DeltaRebalance, got {:?}", other),
        }

        // Gamma drift exceeding bandwidth: GammaRebalance
        let excessive_gamma = gamma + bounds.gamma_bandwidth * dec!(1.5);
        let decision_gamma = guard.evaluate_bichuch_2d_rehedge_trigger(
            spot,
            spot,
            dec!(0.01),
            dec!(0.0),
            excessive_gamma,
            gamma,
            Some(&cfg),
        );
        match decision_gamma {
            BichuchRehedgeDecision::GammaRebalance { .. } => {}
            other => panic!("Expected GammaRebalance, got {:?}", other),
        }
    }

    #[test]
    fn test_synthetic_perp_rebalance_qty_and_venue_selection() {
        let guard = DeltaGuard::new(dec!(10000.0));
        let spot_perp = dec!(60000.0);
        let coin_delta = dec!(0.5); // 0.5 BTC delta
        let coin_mark_value = dec!(12000.0); // Option mark $12k USD
        let beta = dec!(1.0);

        // Dollar delta = 60,000 * 0.5 + 12,000 = $42,000 USD
        // Q_perp* = -(42,000 / 60,000) * 1.0 = -0.70 BTC
        let q_perp =
            guard.synthetic_perp_rebalance_qty(coin_delta, coin_mark_value, spot_perp, beta);
        assert_eq!(q_perp, dec!(-0.7));

        // Venue selection: compare Hyperliquid (1.5 bps taker, 0.0001 8h funding)
        // vs Binance (4.5 bps taker, 0.0001 8h funding)
        let venues = [
            ("hyperliquid", dec!(1.5), dec!(0.0001)),
            ("binance", dec!(4.5), dec!(0.0001)),
        ];
        let best = guard.evaluate_perpetual_venue_selection(&venues, dec!(42000.0), 4.0);
        assert_eq!(best, Some("hyperliquid".to_string()));
    }

    #[test]
    fn test_delta_guard_multi_greek_qp_optimization() {
        use arbitrageur_math::portfolio_greeks_opt::HedgingInstrument;

        let guard = DeltaGuard::with_greek_limits(dec!(500.0), Some(dec!(0.05)), Some(dec!(20.0)));

        let mut greeks_map = HashMap::new();
        greeks_map.insert(
            "BTC".to_string(),
            PortfolioGreeks {
                net_delta: dec!(3.0),
                net_gamma: dec!(-0.25),
                net_vega: dec!(-40.0),
                net_theta: dec!(15.0),
            },
        );

        let instruments = vec![
            HedgingInstrument {
                symbol: "BTC-PERP".to_string(),
                venue: "hyperliquid".to_string(),
                delta: 1.0,
                gamma: 0.0,
                vega: 0.0,
                linear_cost: 0.05,
                quadratic_impact: 0.01,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            HedgingInstrument {
                symbol: "BTC-ATM-CALL".to_string(),
                venue: "deribit".to_string(),
                delta: 0.5,
                gamma: 0.20,
                vega: 5.0,
                linear_cost: 0.10,
                quadratic_impact: 0.02,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
            HedgingInstrument {
                symbol: "BTC-FAR-CALL".to_string(),
                venue: "deribit".to_string(),
                delta: 0.5,
                gamma: 0.05,
                vega: 25.0,
                linear_cost: 0.15,
                quadratic_impact: 0.03,
                min_trade_size: -10.0,
                max_trade_size: 10.0,
            },
        ];

        let opt_solution = guard
            .optimize_portfolio_greeks("BTC", &greeks_map, &instruments, None)
            .expect("Optimal solution");

        assert!(opt_solution.post_hedge_delta.abs() < 3.0);
        assert!(opt_solution.post_hedge_gamma.abs() < 0.25);
        assert!(opt_solution.post_hedge_vega.abs() < 40.0);
    }

    #[test]
    fn test_delta_guard_jump_bandwidth_scaling() {
        use arbitrageur_math::volatility_jump::{JumpAction, JumpAssessment};

        let guard = DeltaGuard::new(dec!(1000.0));

        // 1. Normal diffusion case
        let normal_assessment = JumpAssessment {
            is_jump: false,
            latest_return_bps: 2.5,
            realized_variance: 0.0001,
            bipower_variation: 0.000095,
            relative_jump_ratio: 0.05,
            continuous_variance: 0.000095,
            jump_variance: 0.000005,
            dynamic_threshold_bps: 35.0,
            z_jump_stat: 0.4,
            action: JumpAction::NormalDiffusion,
        };
        let scaling_normal = guard.evaluate_jump_bandwidth_scaling(&normal_assessment);
        assert_eq!(scaling_normal, 1.0);

        // 2. High jump ratio / jump detected
        let jump_assessment = JumpAssessment {
            is_jump: true,
            latest_return_bps: -75.0,
            realized_variance: 0.002,
            bipower_variation: 0.001,
            relative_jump_ratio: 0.50,
            continuous_variance: 0.001,
            jump_variance: 0.001,
            dynamic_threshold_bps: 40.0,
            z_jump_stat: 4.8,
            action: JumpAction::PullBidsAndWidenAsks {
                jump_return_bps: -75.0,
                relative_jump_ratio: 0.50,
                hurdle_expansion_bps: 30.0,
            },
        };
        let scaling_jump = guard.evaluate_jump_bandwidth_scaling(&jump_assessment);
        assert!((scaling_jump - 0.75).abs() < 1e-6);

        // 3. Extreme jump ratio clamping
        let extreme_assessment = JumpAssessment {
            is_jump: true,
            latest_return_bps: -250.0,
            realized_variance: 0.01,
            bipower_variation: 0.001,
            relative_jump_ratio: 0.90,
            continuous_variance: 0.001,
            jump_variance: 0.009,
            dynamic_threshold_bps: 50.0,
            z_jump_stat: 8.5,
            action: JumpAction::PullBidsAndWidenAsks {
                jump_return_bps: -250.0,
                relative_jump_ratio: 0.90,
                hurdle_expansion_bps: 50.0,
            },
        };
        let scaling_extreme = guard.evaluate_jump_bandwidth_scaling(&extreme_assessment);
        // 1.0 - 0.5 * 0.90 = 0.55 clamped to [0.40, 1.0]
        assert!((scaling_extreme - 0.55).abs() < 1e-6);
    }

    #[test]
    fn test_delta_guard_macro_stress_and_covar_and_es() {
        let guard = DeltaGuard::new(dec!(10000.0));
        let greeks = PortfolioGreeks {
            net_delta: dec!(1.5),
            net_gamma: dec!(-0.0002),
            net_vega: dec!(40.0),
            net_theta: dec!(-10.0),
        };
        let spot = dec!(60000.0);

        // 1. Standard macro stress testing suite
        let stress_results = guard.stress_test_portfolio_macro_scenarios(spot, &greeks, None);
        assert_eq!(stress_results.len(), 4);
        assert!(stress_results[0].total_stressed_pnl < 0.0); // Black Swan crash produces net loss

        // 2. Systemic CoVaR evaluation between BTC and ETH
        let covar =
            guard.compute_systemic_delta_covar("BTC", "ETH", 0.50, 0.75, 0.80, -0.10, 0.001);
        // beta = 0.80 * (0.75 / 0.50) = 1.20
        assert!((covar.systemic_beta - 1.20).abs() < 1e-4);
        assert!(covar.delta_covar < 0.0);

        // 3. EVT Generalized Pareto Distribution Expected Shortfall
        let evt = guard.evaluate_tail_evt_expected_shortfall(0.05, 0.15, 0.02, 0.05, 0.999);
        assert!(evt.tail_var > 0.05);
        assert!(evt.tail_expected_shortfall > evt.tail_var);
    }

    #[test]
    fn test_delta_guard_cross_perp_solvency_and_convexity_limits() {
        let guard = DeltaGuard::new(dec!(10000.0));

        // 1. Solvency barrier check with sufficient vs insufficient margin
        let res_ok = guard.verify_cross_perp_skew_margin_solvency(
            15_000.0, // free A
            1_000.0,  // mmr A
            15_000.0, // free B
            1_000.0,  // mmr B
            20_000.0, // notional
            3_000.0,  // spot
            true,     // is_long_a
            false,    // linear A
            false,    // linear B
            None,
        );
        assert!(res_ok.is_ok(), "Healthy margin should pass solvency guard");

        let res_breach = guard.verify_cross_perp_skew_margin_solvency(
            500.0,    // insufficient free A
            1_000.0,  // mmr A
            15_000.0, // free B
            1_000.0,  // mmr B
            20_000.0, // notional
            3_000.0,  // spot
            true,     // is_long_a
            false, false, None,
        );
        assert!(
            res_breach.is_err(),
            "Insufficient margin must trigger solvency barrier failure"
        );

        // 2. Second-order convexity limit check
        // With delta = $0, gamma = -$50,000 (negative dollar gamma), spot shock = 35%
        // Loss = 0.5 * 50,000 * 0.35^2 = $3,062.50
        let loss_res = guard.check_second_order_convexity_limits(0.0, -50_000.0, 3_000.0, 5_000.0);
        assert!(loss_res.is_ok());
        let loss = loss_res.unwrap();
        assert!((loss - 3062.5).abs() < 1e-2);

        // With tighter loss budget ($2,000), should fail
        let loss_fail = guard.check_second_order_convexity_limits(0.0, -50_000.0, 3_000.0, 2_000.0);
        assert!(loss_fail.is_err());
    }
}
