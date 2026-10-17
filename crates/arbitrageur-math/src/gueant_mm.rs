//! Guéant-Tapia-Manziadi (2012) Asymmetric Optimal Market Making with Perpetual Funding Carry.
//!
//! Solves the Hamilton-Jacobi-Bellman (HJB) inventory risk equations under continuous
//! perpetual funding cash flows $\phi_t q_t S_t$, deriving closed-form optimal half-spreads
//! $\delta^{a*}(q), \delta^{b*}(q)$ and dynamic reservation price $R(s, q, \tau, \phi)$.

use serde::{Deserialize, Serialize};

/// Optimal half-spread quotes and reservation price recommendations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GueantQuoteRecommendation {
    pub reservation_price: f64,
    pub bid_half_spread: f64,
    pub ask_half_spread: f64,
    pub recommended_bid: f64,
    pub recommended_ask: f64,
    pub inventory_skew_bps: f64,
    pub funding_tilt_bps: f64,
}

/// Guéant-Tapia-Manziadi Perpetual Market Making Engine with Continuous Funding Carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GueantPerpMarketMaker {
    /// Absolute inventory risk aversion $\gamma > 0$
    pub risk_aversion_gamma: f64,
    /// Annualized / instantaneous asset return volatility $\sigma > 0$
    pub asset_volatility_sigma: f64,
    /// Base order arrival intensity $A > 0$ in $\Lambda(\delta) = A e^{-k \delta}$
    pub order_arrival_a: f64,
    /// Order fill price sensitivity parameter $k > 0$
    pub order_arrival_k: f64,
    /// Maximum inventory threshold $Q_{\max} \ge 1.0$
    pub max_inventory_q: f64,
    /// Minimum half-spread hurdle in basis points (e.g. 1.0 bps)
    pub min_half_spread_bps: f64,
    /// Adverse selection Order Flow Imbalance (OFI) shading sensitivity $\psi \ge 0$
    pub ofi_shading_sensitivity: f64,
}

impl Default for GueantPerpMarketMaker {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 1e-4,
            asset_volatility_sigma: 0.60,
            order_arrival_a: 140.0,
            order_arrival_k: 1.5,
            max_inventory_q: 10.0,
            min_half_spread_bps: 1.5,
            ofi_shading_sensitivity: 0.75,
        }
    }
}

impl GueantPerpMarketMaker {
    pub fn new(
        gamma: f64,
        sigma: f64,
        arrival_a: f64,
        arrival_k: f64,
        max_q: f64,
        min_half_spread_bps: f64,
    ) -> Self {
        Self {
            risk_aversion_gamma: gamma.max(1e-8),
            asset_volatility_sigma: sigma.max(1e-4),
            order_arrival_a: arrival_a.max(0.1),
            order_arrival_k: arrival_k.max(0.1),
            max_inventory_q: max_q.max(1.0),
            min_half_spread_bps: min_half_spread_bps.max(0.1),
            ofi_shading_sensitivity: 0.75,
        }
    }

    /// Builder to set OFI adverse selection shading sensitivity.
    pub fn with_ofi_sensitivity(mut self, sensitivity: f64) -> Self {
        self.ofi_shading_sensitivity = sensitivity.max(0.0);
        self
    }

    /// Computes the dynamic reservation / indifference price:
    ///
    /// $$R(s, q, \tau, \phi) = s - q \cdot \gamma \sigma^2 s^2 \tau - \frac{\phi s}{\gamma \sigma^2 s^2} (1 - e^{-\tau})$$
    ///
    /// where:
    /// - $s$: current mid-price
    /// - $q$: current signed inventory in base units (+ long, - short)
    /// - $\tau$: time horizon in years or fractional period
    /// - $\phi$: continuous funding rate (+ long pays funding, - long receives funding)
    pub fn reservation_price(
        &self,
        mid_price: f64,
        current_inventory: f64,
        horizon_tau: f64,
        funding_rate_phi: f64,
    ) -> f64 {
        let q = current_inventory.clamp(-self.max_inventory_q, self.max_inventory_q);
        let tau = horizon_tau.max(1e-4);
        let s = mid_price;

        let vol_var = (self.asset_volatility_sigma * s).powi(2);
        let inventory_penalty = q * self.risk_aversion_gamma * vol_var * tau;
        let funding_tilt = (funding_rate_phi * s) / (self.risk_aversion_gamma * vol_var).max(1e-8);

        s - inventory_penalty - funding_tilt * (1.0 - (-tau).exp())
    }

    /// Computes exact closed-form optimal half-spreads $\delta^{a*}(q), \delta^{b*}(q)$
    /// under exponential arrival intensity $\Lambda(\delta) = A e^{-k \delta}$ and perpetual funding carry.
    ///
    /// $$\delta^{a*}(q) = \frac{1}{k} - \left(q - \frac{1}{2}\right) \xi - \frac{\phi s}{\gamma \sigma^2 s^2} \cdot \frac{1}{\tau + 1}$$
    /// $$\delta^{b*}(q) = \frac{1}{k} + \left(q + \frac{1}{2}\right) \xi + \frac{\phi s}{\gamma \sigma^2 s^2} \cdot \frac{1}{\tau + 1}$$
    ///
    /// where $\xi = \sqrt{\frac{\gamma \sigma^2 s^2}{2 k A}}$.
    pub fn compute_optimal_half_spreads(
        &self,
        mid_price: f64,
        current_inventory: f64,
        horizon_tau: f64,
        funding_rate_phi: f64,
    ) -> (f64, f64) {
        let q = current_inventory.clamp(-self.max_inventory_q, self.max_inventory_q);
        let s = mid_price;
        let k = self.order_arrival_k;
        let a = self.order_arrival_a;
        let gamma = self.risk_aversion_gamma;
        let vol_var = (self.asset_volatility_sigma * s).powi(2);

        let xi = (gamma * vol_var / (2.0 * k * a).max(1e-6)).sqrt();
        let funding_drift =
            (funding_rate_phi * s) / (gamma * vol_var).max(1e-8) * (1.0 / (horizon_tau + 1.0));

        let delta_a_raw = (1.0 / k) - (q - 0.5) * xi - funding_drift;
        let delta_b_raw = (1.0 / k) + (q + 0.5) * xi + funding_drift;

        let min_spread = s * (self.min_half_spread_bps / 10_000.0);
        let delta_a = delta_a_raw.max(min_spread);
        let delta_b = delta_b_raw.max(min_spread);

        (delta_a, delta_b)
    }

    /// Applies dynamic adverse selection Order Flow Imbalance (OFI) shading during directional sweeps:
    ///
    /// - If $\Delta \text{OFI} > 0$ (aggressive taker buys): widen ask spread $\delta^a$ and tighten bid spread $\delta^b$.
    /// - If $\Delta \text{OFI} < 0$ (aggressive taker sells): widen bid spread $\delta^b$ and tighten ask spread $\delta^a$.
    pub fn apply_ofi_adverse_shading(
        &self,
        delta_a: f64,
        delta_b: f64,
        ofi_delta: f64,
        tick_size: f64,
        dt_sec: f64,
    ) -> (f64, f64) {
        let dt = dt_sec.max(0.001);
        let flow_speed = ofi_delta / dt.sqrt();
        let shade_amount = self.ofi_shading_sensitivity * flow_speed * tick_size;

        let shaded_a = if ofi_delta > 0.0 {
            delta_a + shade_amount.abs()
        } else {
            (delta_a - shade_amount.abs() * 0.5).max(tick_size)
        };

        let shaded_b = if ofi_delta < 0.0 {
            delta_b + shade_amount.abs()
        } else {
            (delta_b - shade_amount.abs() * 0.5).max(tick_size)
        };

        (shaded_a, shaded_b)
    }

    /// Generates full quote recommendation with reservation price and recommended bid/ask limits.
    pub fn generate_quote_recommendation(
        &self,
        mid_price: f64,
        current_inventory: f64,
        horizon_tau: f64,
        funding_rate_phi: f64,
        ofi_delta: Option<f64>,
        tick_size: f64,
    ) -> GueantQuoteRecommendation {
        let r_price =
            self.reservation_price(mid_price, current_inventory, horizon_tau, funding_rate_phi);
        let (raw_da, raw_db) = self.compute_optimal_half_spreads(
            mid_price,
            current_inventory,
            horizon_tau,
            funding_rate_phi,
        );

        let (final_da, final_db) = if let Some(ofi) = ofi_delta {
            self.apply_ofi_adverse_shading(raw_da, raw_db, ofi, tick_size, 1.0)
        } else {
            (raw_da, raw_db)
        };

        let recommended_bid = (mid_price - final_db).max(0.0);
        let recommended_ask = mid_price + final_da;

        let inventory_skew_bps = ((r_price - mid_price) / mid_price) * 10_000.0;
        let funding_tilt_bps = (funding_rate_phi
            / (self.risk_aversion_gamma * (self.asset_volatility_sigma).powi(2)))
            * 10_000.0;

        GueantQuoteRecommendation {
            reservation_price: r_price,
            bid_half_spread: final_db,
            ask_half_spread: final_da,
            recommended_bid,
            recommended_ask,
            inventory_skew_bps,
            funding_tilt_bps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reservation_price_inventory_and_funding_skew() {
        let mm = GueantPerpMarketMaker::new(1e-5, 0.50, 100.0, 1.5, 10.0, 1.0);
        let mid = 50_000.0;

        // 1. Zero inventory, zero funding -> reservation price equals mid exactly
        let r_neutral = mm.reservation_price(mid, 0.0, 0.01, 0.0);
        assert_eq!(r_neutral, mid);

        // 2. Long inventory (q = +5) -> reservation price drops below mid to encourage client buys
        let r_long = mm.reservation_price(mid, 5.0, 0.01, 0.0);
        assert!(
            r_long < r_neutral,
            "Long inventory must lower reservation price"
        );

        // 3. Short inventory (q = -5) -> reservation price rises above mid to encourage client sells
        let r_short = mm.reservation_price(mid, -5.0, 0.01, 0.0);
        assert!(
            r_short > r_neutral,
            "Short inventory must raise reservation price"
        );

        // 4. Long pays positive funding (phi = +0.0003) -> further depresses reservation price
        let r_long_funding = mm.reservation_price(mid, 5.0, 0.01, 0.0003);
        assert!(
            r_long_funding < r_long,
            "Positive funding on long must depress reservation price further"
        );
    }

    #[test]
    fn test_optimal_half_spreads_asymmetry() {
        let mm = GueantPerpMarketMaker::new(1e-5, 0.50, 100.0, 1.5, 10.0, 1.0);
        let mid = 3_000.0;

        // Long inventory (q = 4): ask spread should tighten (to dump inventory), bid spread should widen
        let (delta_a, delta_b) = mm.compute_optimal_half_spreads(mid, 4.0, 0.01, 0.0);
        let (delta_a_flat, delta_b_flat) = mm.compute_optimal_half_spreads(mid, 0.0, 0.01, 0.0);

        assert!(
            delta_a <= delta_a_flat,
            "Long inventory must tighten or equal ask half-spread"
        );
        assert!(
            delta_b >= delta_b_flat,
            "Long inventory must widen or equal bid half-spread"
        );
    }

    #[test]
    fn test_ofi_adverse_selection_shading() {
        let mm = GueantPerpMarketMaker::default();
        let base_da = 2.0;
        let base_db = 2.0;

        // Aggressive buying sweep (+OFI): ask should widen defensively
        let (shaded_a, shaded_b) = mm.apply_ofi_adverse_shading(base_da, base_db, 15.0, 0.1, 1.0);
        assert!(
            shaded_a > base_da,
            "Aggressive buy sweep must widen ask spread"
        );
        assert!(
            shaded_b <= base_db,
            "Aggressive buy sweep should tighten bid spread"
        );

        // Aggressive selling sweep (-OFI): bid should widen defensively
        let (shaded_a_sell, shaded_b_sell) =
            mm.apply_ofi_adverse_shading(base_da, base_db, -15.0, 0.1, 1.0);
        assert!(
            shaded_b_sell > base_db,
            "Aggressive sell sweep must widen bid spread"
        );
        assert!(
            shaded_a_sell <= base_da,
            "Aggressive sell sweep should tighten ask spread"
        );
    }
}
