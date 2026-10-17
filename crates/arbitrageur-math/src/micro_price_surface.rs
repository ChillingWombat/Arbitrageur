//! Multi-Level Order Book Micro-Price Imbalance Surface & Online Lead-Lag Discovery.
//!
//! Implements:
//! 1. **Multi-Level Stoikov Micro-Price Surface**:
//!    Extends the classical top-of-book micro-price (Stoikov 2018) to $K$ depth levels
//!    with spatial exponential decay $w_l = e^{-\kappa(l-1)}$, sub-linear concave volume
//!    sensitivity $\alpha \in [0.4, 1.0]$, and queue cancellation velocity differentials.
//! 2. **Online Multinomial Logistic Lead-Lag Classifier**:
//!    Tick-by-tick Normalized Online Gradient Descent (OGD) predicting directional mid-price
//!    jumps on follower venues (Hyperliquid, Lighter, Deribit) conditional on order flow
//!    arrivals at leader venues (Binance, OKX).
//! 3. **Closed-Form Anticipatory Spread Inversion Sniping Hurdle**:
//!    Analytically determines when expected follower micro-price drift justifies aggressive
//!    taker IOC pre-emption before the local order book updates, beating reactive market makers.

use serde::{Deserialize, Serialize};

/// Configuration parameters for the multi-level micro-price surface and lead-lag predictor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiLevelMicroPriceConfig {
    /// Number of order book levels to include in surface evaluation (e.g. 5 or 10).
    pub depth_levels: usize,
    /// Spatial depth exponential decay rate $\kappa$ (e.g. 0.35).
    pub depth_decay_kappa: f64,
    /// Sub-linear concave volume sensitivity exponent $\alpha \in [0.4, 1.0]$ (e.g. 0.75).
    pub volume_exponent_alpha: f64,
    /// Cancellation velocity weighting coefficient $\eta$ (e.g. 0.20).
    pub cancel_weight_eta: f64,
    /// Order book elasticity parameter $\zeta$ scaling the hyperbolic tangent adjustment (e.g. 1.50).
    pub elasticity_zeta: f64,
    /// Online learning rate $\eta_{\text{lr}}$ for Normalized OGD (e.g. 0.01).
    pub learning_rate: f64,
    /// Fixed minimum profit hurdle in basis points (e.g. 2.0 bps).
    pub min_hurdle_bps: f64,
}

impl Default for MultiLevelMicroPriceConfig {
    fn default() -> Self {
        Self {
            depth_levels: 5,
            depth_decay_kappa: 0.35,
            volume_exponent_alpha: 0.75,
            cancel_weight_eta: 0.20,
            elasticity_zeta: 1.50,
            learning_rate: 0.01,
            min_hurdle_bps: 2.0,
        }
    }
}

/// Directional anticipatory sniping decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnticipatorySnipingDecision {
    /// Anticipatory upward drift exceeds hurdle: aggressively lift follower ask (IOC buy).
    SnipeBuy,
    /// Anticipatory downward drift exceeds hurdle: aggressively hit follower bid (IOC sell).
    SnipeSell,
    /// Toxic leader sweep flowing against pending local order: immediately pull quotes.
    RejectAdverseSweep,
    /// Expected drift does not compensate for round-trip friction and latency jitter: hold.
    HoldNoAdvantage,
}

/// Evaluation result from anticipatory lead-lag sniping analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnticipatorySnipingResult {
    /// Tactical execution action.
    pub decision: AnticipatorySnipingDecision,
    /// Expected forward price drift on follower venue in basis points.
    pub expected_drift_bps: f64,
    /// Total round-trip friction hurdle (half spread + fees + jitter cushion) in basis points.
    pub friction_hurdle_bps: f64,
    /// Net expected edge over friction in basis points.
    pub net_edge_bps: f64,
    /// Predicted probability of upward mid-price jump $P(\text{Up})$.
    pub prob_up: f64,
    /// Predicted probability of stationary price $P(\text{Stay})$.
    pub prob_stay: f64,
    /// Predicted probability of downward mid-price jump $P(\text{Down})$.
    pub prob_down: f64,
}

/// Multi-level order book micro-price surface and online lead-lag predictive engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiLevelMicroPriceSurface {
    pub config: MultiLevelMicroPriceConfig,
    /// Weights matrix for 3 classes (0: Up, 1: Stay, 2: Down) across 6 features.
    pub weights: [[f64; 6]; 3],
    /// Running sample count for online model updates.
    pub updates_count: u64,
}

impl MultiLevelMicroPriceSurface {
    pub fn new(config: MultiLevelMicroPriceConfig) -> Self {
        // Initialize weights with slight prior favoring zero drift on neutral features
        let mut weights = [[0.0; 6]; 3];
        weights[0][0] = 0.50; // Positive leader imbalance correlates with Up
        weights[0][1] = 0.40; // Positive leader OFI correlates with Up
        weights[2][0] = -0.50; // Negative leader imbalance correlates with Down
        weights[2][1] = -0.40; // Negative leader OFI correlates with Down

        Self {
            config,
            weights,
            updates_count: 0,
        }
    }

    /// Computes generalized multi-level order book imbalance $I_K \in [-1.0, 1.0]$.
    ///
    /// $$I_K = \frac{\sum_{l=1}^K w_l (Q_b^{(l)})^\alpha - \sum_{l=1}^K w_l (Q_a^{(l)})^\alpha}
    ///              {\sum_{l=1}^K w_l (Q_b^{(l)})^\alpha + \sum_{l=1}^K w_l (Q_a^{(l)})^\alpha}$$
    pub fn compute_multi_level_imbalance(
        &self,
        bid_depths: &[f64],
        ask_depths: &[f64],
        v_cancel_bid: f64,
        v_cancel_ask: f64,
    ) -> f64 {
        let k = self
            .config
            .depth_levels
            .min(bid_depths.len())
            .min(ask_depths.len());
        if k == 0 {
            return 0.0;
        }

        let mut num = 0.0;
        let mut den = 0.0;

        for l in 0..k {
            let w = (-self.config.depth_decay_kappa * l as f64).exp();
            let q_b = bid_depths[l]
                .max(0.0)
                .powf(self.config.volume_exponent_alpha);
            let q_a = ask_depths[l]
                .max(0.0)
                .powf(self.config.volume_exponent_alpha);

            num += w * (q_b - q_a);
            den += w * (q_b + q_a);
        }

        let i_raw = if den > 1e-12 { num / den } else { 0.0 };

        // Incorporate queue cancellation velocity differential
        let cancel_diff = if v_cancel_bid + v_cancel_ask > 1e-9 {
            (v_cancel_bid - v_cancel_ask) / (v_cancel_bid + v_cancel_ask)
        } else {
            0.0
        };

        (i_raw - self.config.cancel_weight_eta * cancel_diff).clamp(-1.0, 1.0)
    }

    /// Computes continuous multi-level micro-price:
    ///
    /// $$P_{\text{micro}} = P_{\text{mid}} + \frac{S}{2} \cdot \tanh\left( \frac{\zeta \cdot S \cdot I_{\text{eff}}}{2} \right)$$
    pub fn compute_surface_micro_price(
        &self,
        mid_price: f64,
        spread: f64,
        bid_depths: &[f64],
        ask_depths: &[f64],
        v_cancel_bid: f64,
        v_cancel_ask: f64,
    ) -> f64 {
        if spread <= 0.0 || !spread.is_finite() {
            return mid_price;
        }

        let i_eff =
            self.compute_multi_level_imbalance(bid_depths, ask_depths, v_cancel_bid, v_cancel_ask);
        let g_val = (0.5 * self.config.elasticity_zeta * spread * i_eff).tanh();
        mid_price + 0.5 * spread * g_val
    }

    /// Evaluates softmax jump probabilities: $[P(\text{Up}), P(\text{Stay}), P(\text{Down})]$.
    ///
    /// Feature vector format:
    /// `[i_k_lead, ofi_lead, v_micro_lead, rel_spread_lead, i_k_follow, price_ratio_log]`
    #[allow(clippy::needless_range_loop)]
    pub fn predict_jump_probabilities(&self, features: &[f64; 6]) -> [f64; 3] {
        let mut logits = [0.0; 3];
        let mut max_l = f64::NEG_INFINITY;

        for c in 0..3 {
            let mut dot = 0.0;
            for f in 0..6 {
                dot += self.weights[c][f] * features[f];
            }
            logits[c] = dot;
            if dot > max_l {
                max_l = dot;
            }
        }

        let mut sum_exp = 0.0;
        let mut probs = [0.0; 3];
        for c in 0..3 {
            probs[c] = (logits[c] - max_l).exp();
            sum_exp += probs[c];
        }

        for c in 0..3 {
            probs[c] /= sum_exp.max(1e-12);
        }
        probs
    }

    /// Tick-by-tick Normalized Online Gradient Descent (OGD) update.
    /// `target_class`: 0 for Up, 1 for Stay, 2 for Down.
    #[allow(clippy::needless_range_loop)]
    pub fn online_update(&mut self, features: &[f64; 6], target_class: usize) {
        if target_class > 2 {
            return;
        }

        let probs = self.predict_jump_probabilities(features);
        let norm_sq: f64 = features.iter().map(|x| x * x).sum::<f64>() + 1e-6;

        for c in 0..3 {
            let error = probs[c] - if c == target_class { 1.0 } else { 0.0 };
            for f in 0..6 {
                self.weights[c][f] -= self.config.learning_rate * error * (features[f] / norm_sq);
            }
        }
        self.updates_count += 1;
    }

    /// Evaluates whether forward expected drift justifies anticipatory pre-emption.
    ///
    /// $$E[\Delta P_{\text{follow}}] \ge \frac{1}{2} S_{\text{follow}} + f_{\text{taker}} + f_{\text{exit}} + 1.96 \sigma \sqrt{\tau}$$
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_anticipatory_sniping(
        &self,
        features: &[f64; 6],
        spread_follower_bps: f64,
        taker_fee_bps: f64,
        exit_fee_bps: f64,
        expected_jump_size_bps: f64,
        latency_jitter_sec: f64,
        vol_follower: f64,
    ) -> AnticipatorySnipingResult {
        let probs = self.predict_jump_probabilities(features);
        let prob_up = probs[0];
        let prob_stay = probs[1];
        let prob_down = probs[2];

        // Directional bias: P(Up) - P(Down) in [-1.0, 1.0]
        let directional_bias = prob_up - prob_down;
        let expected_drift_bps = directional_bias * expected_jump_size_bps;

        // Convert annual volatility to latency window volatility in basis points
        let tau_years = (latency_jitter_sec.max(1e-4) / (365.25 * 86400.0)).max(1e-12);
        let jitter_cushion_bps = 1.96 * vol_follower * tau_years.sqrt() * 10000.0;
        let friction_hurdle_bps =
            0.5 * spread_follower_bps + taker_fee_bps + exit_fee_bps + jitter_cushion_bps;

        let net_edge_bps = expected_drift_bps.abs() - friction_hurdle_bps;

        let decision = if expected_drift_bps > friction_hurdle_bps
            && net_edge_bps >= self.config.min_hurdle_bps
        {
            AnticipatorySnipingDecision::SnipeBuy
        } else if expected_drift_bps < -friction_hurdle_bps
            && net_edge_bps >= self.config.min_hurdle_bps
        {
            AnticipatorySnipingDecision::SnipeSell
        } else if expected_drift_bps.abs() > friction_hurdle_bps * 0.70 {
            AnticipatorySnipingDecision::RejectAdverseSweep
        } else {
            AnticipatorySnipingDecision::HoldNoAdvantage
        };

        AnticipatorySnipingResult {
            decision,
            expected_drift_bps,
            friction_hurdle_bps,
            net_edge_bps,
            prob_up,
            prob_stay,
            prob_down,
        }
    }
}

/// Depth level quote snapshot for multi-level OFI computation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DepthLevelQuote {
    pub bid_price: f64,
    pub bid_size: f64,
    pub ask_price: f64,
    pub ask_size: f64,
}

impl Default for DepthLevelQuote {
    fn default() -> Self {
        Self {
            bid_price: 0.0,
            bid_size: 0.0,
            ask_price: 0.0,
            ask_size: 0.0,
        }
    }
}

impl DepthLevelQuote {
    pub fn new(bid_price: f64, bid_size: f64, ask_price: f64, ask_size: f64) -> Self {
        Self {
            bid_price,
            bid_size,
            ask_price,
            ask_size,
        }
    }
}

/// Order Flow Imbalance (OFI) Toxicity and Adverse Selection Regime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfiToxicityRegime {
    /// Symmetrical order flow with normal price impact: benign market conditions.
    Benign,
    /// Mild directional imbalance within normal market maker absorption capacity.
    MildImbalance,
    /// Toxic aggressive buying pressure: pull ask orders and shade reservation price upwards.
    ToxicBidSurge,
    /// Toxic aggressive selling pressure: pull bid orders and shade reservation price downwards.
    ToxicAskSurge,
}

/// Result of multi-level OFI evaluation, Kyle's lambda estimation, and reservation price skewing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfiToxicityEvaluation {
    /// Multi-level weighted Order Flow Imbalance OFI^{(L)}(t).
    pub ofi_surface: f64,
    /// Current estimated Kyle's lambda price impact coefficient \lambda_{Kyle}(t).
    pub kyle_lambda: f64,
    /// Product of absolute OFI and Kyle's lambda: total adverse selection pressure.
    pub toxicity_metric: f64,
    /// Adverse selection regime classification.
    pub regime: OfiToxicityRegime,
    /// Avellaneda-Stoikov reservation price with OFI skew shading r(q, t).
    pub shaded_reservation_price: f64,
    /// Net skew offset applied to mid price (in price units).
    pub total_skew_offset: f64,
    /// Whether resting passive quotes on the impacted side should be proactively withdrawn/shaded.
    pub should_evacuate_passive_quotes: bool,
}

/// Configuration parameters for Multi-Level OFI and Kyle's Lambda Toxicity Engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MultiLevelOfiConfig {
    /// Number of depth levels L to aggregate (default: 5).
    pub depth_levels: usize,
    /// Spatial decay rate \kappa across order book levels (default: 0.40).
    pub depth_decay_kappa: f64,
    /// Recursive EWMA decay factor \beta for Kyle's lambda estimation (default: 0.95).
    pub ewma_decay_beta: f64,
    /// Epsilon variance regularizer to prevent division by zero (default: 1e-6).
    pub variance_epsilon: f64,
    /// Sensitivity coefficient \theta_{OFI} scaling the OFI skew addition to reservation price (default: 1.0).
    pub ofi_skew_theta: f64,
    /// Inventory risk aversion parameter \gamma in Avellaneda-Stoikov (default: 0.05).
    pub inventory_gamma: f64,
    /// Toxicity threshold above which flow is flagged as toxic surge (default: 0.05).
    pub toxicity_threshold: f64,
}

impl Default for MultiLevelOfiConfig {
    fn default() -> Self {
        Self {
            depth_levels: 5,
            depth_decay_kappa: 0.40,
            ewma_decay_beta: 0.95,
            variance_epsilon: 1e-6,
            ofi_skew_theta: 1.0,
            inventory_gamma: 0.05,
            toxicity_threshold: 0.05,
        }
    }
}

/// Real-time Multi-Level Order Flow Imbalance (OFI) & Kyle's Lambda Adverse Selection Engine.
///
/// Implements:
/// 1. **Multi-Level OFI Surface**:
///    $$\text{OFI}^{(L)}(t) = \sum_{\ell=1}^L e^{-\kappa (\ell - 1)} \Delta \text{OFI}_\ell(t)$$
/// 2. **Online Recursive Kyle's Lambda**:
///    $$\lambda_{\text{Kyle}}(t) = \frac{\operatorname{Cov}(\Delta P, \text{OFI}^{(L)})}{\operatorname{Var}(\text{OFI}^{(L)}) + \epsilon}$$
/// 3. **Avellaneda-Stoikov Reservation Price with OFI Skew Shading**:
///    $$r(q, t) = s(t) - q \gamma \sigma^2 (T - t) + \theta_{\text{OFI}} \cdot \lambda_{\text{Kyle}}(t) \cdot \text{OFI}^{(L)}(t)$$
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MultiLevelOfiToxicityEngine {
    pub config: MultiLevelOfiConfig,
    /// Previous order book depth level quotes (up to 10 levels).
    prev_levels: [DepthLevelQuote; 10],
    /// Number of valid depth levels in prev_levels buffer.
    prev_levels_count: usize,
    /// Previous mid price.
    prev_mid_price: Option<f64>,
    /// EWMA mean of multi-level OFI: \mu_{OFI}.
    ewma_mean_ofi: f64,
    /// EWMA mean of mid price change: \mu_{\Delta P}.
    ewma_mean_delta_p: f64,
    /// EWMA variance of multi-level OFI: \sigma^2_{OFI}.
    ewma_var_ofi: f64,
    /// EWMA covariance between mid price change and OFI: \operatorname{Cov}(\Delta P, OFI).
    ewma_cov_delta_p_ofi: f64,
    /// Current estimated Kyle's lambda.
    current_kyle_lambda: f64,
    /// Total updates observed.
    updates_count: u64,
}

impl Default for MultiLevelOfiToxicityEngine {
    fn default() -> Self {
        Self::new(MultiLevelOfiConfig::default())
    }
}

impl MultiLevelOfiToxicityEngine {
    pub fn new(config: MultiLevelOfiConfig) -> Self {
        Self {
            config,
            prev_levels: [DepthLevelQuote::default(); 10],
            prev_levels_count: 0,
            prev_mid_price: None,
            ewma_mean_ofi: 0.0,
            ewma_mean_delta_p: 0.0,
            ewma_var_ofi: 1.0,
            ewma_cov_delta_p_ofi: 0.01,
            current_kyle_lambda: 0.01,
            updates_count: 0,
        }
    }

    /// Computes discrete Order Flow Imbalance for a single depth level \ell:
    ///
    /// \Delta I_{bid, \ell}(t) = \begin{cases} v_{b, \ell}(t), & p_{b, \ell}(t) > p_{b, \ell}(t-1) \\
    ///                                       v_{b, \ell}(t) - v_{b, \ell}(t-1), & p_{b, \ell}(t) = p_{b, \ell}(t-1) \\
    ///                                       -v_{b, \ell}(t-1), & p_{b, \ell}(t) < p_{b, \ell}(t-1) \end{cases}
    ///
    /// \Delta I_{ask, \ell}(t) = \begin{cases} -v_{a, \ell}(t-1), & p_{a, \ell}(t) > p_{a, \ell}(t-1) \\
    ///                                       v_{a, \ell}(t) - v_{a, \ell}(t-1), & p_{a, \ell}(t) = p_{a, \ell}(t-1) \\
    ///                                       v_{a, \ell}(t), & p_{a, \ell}(t) < p_{a, \ell}(t-1) \end{cases}
    ///
    /// \Delta \text{OFI}_\ell(t) = \Delta I_{bid, \ell}(t) - \Delta I_{ask, \ell}(t)
    pub fn compute_level_ofi(prev: &DepthLevelQuote, curr: &DepthLevelQuote) -> f64 {
        const EPS: f64 = 1e-9;

        let delta_i_bid = if curr.bid_price > prev.bid_price + EPS {
            curr.bid_size
        } else if (curr.bid_price - prev.bid_price).abs() <= EPS {
            curr.bid_size - prev.bid_size
        } else {
            -prev.bid_size
        };

        let delta_i_ask = if curr.ask_price > prev.ask_price + EPS {
            -prev.ask_size
        } else if (curr.ask_price - prev.ask_price).abs() <= EPS {
            curr.ask_size - prev.ask_size
        } else {
            curr.ask_size
        };

        delta_i_bid - delta_i_ask
    }

    /// Computes multi-level weighted Order Flow Imbalance across L depth levels.
    pub fn compute_multi_level_ofi(
        &self,
        prev_levels: &[DepthLevelQuote],
        curr_levels: &[DepthLevelQuote],
    ) -> f64 {
        let l_max = self
            .config
            .depth_levels
            .min(prev_levels.len())
            .min(curr_levels.len());
        if l_max == 0 {
            return 0.0;
        }

        let mut total_ofi = 0.0;
        for l in 0..l_max {
            let p = &prev_levels[l];
            let c = &curr_levels[l];
            let ofi_l = Self::compute_level_ofi(p, c);
            let weight = (-self.config.depth_decay_kappa * l as f64).exp();
            total_ofi += weight * ofi_l;
        }

        total_ofi
    }

    /// Evaluates Avellaneda-Stoikov reservation price with OFI skew shading.
    /// Returns (shaded_reservation_price, total_skew_offset).
    pub fn calculate_reservation_price(
        &self,
        mid_price: f64,
        inventory_q: f64,
        asset_vol: f64,
        horizon_t: f64,
        ofi_surface: f64,
        kyle_lambda: f64,
    ) -> (f64, f64) {
        let t_rem = horizon_t.max(0.0001);
        let vol_sq = (asset_vol * asset_vol).max(0.0);
        let inventory_skew = -inventory_q * self.config.inventory_gamma * vol_sq * t_rem;
        let ofi_skew = self.config.ofi_skew_theta * kyle_lambda * ofi_surface;
        let total_offset = inventory_skew + ofi_skew;
        let reservation_price = mid_price + total_offset;
        (reservation_price, total_offset)
    }

    /// Ingests a new depth snapshot, updates recursive Kyle's lambda, and evaluates OFI toxicity.
    pub fn update(
        &mut self,
        current_levels: &[DepthLevelQuote],
        current_mid: f64,
        inventory_q: f64,
        asset_vol: f64,
        horizon_t: f64,
    ) -> OfiToxicityEvaluation {
        if self.prev_levels_count == 0 || self.prev_mid_price.is_none() {
            let count = current_levels.len().min(10);
            self.prev_levels[..count].copy_from_slice(&current_levels[..count]);
            self.prev_levels_count = count;
            self.prev_mid_price = Some(current_mid);
            let (r_px, offset) = self.calculate_reservation_price(
                current_mid,
                inventory_q,
                asset_vol,
                horizon_t,
                0.0,
                self.current_kyle_lambda,
            );
            return OfiToxicityEvaluation {
                ofi_surface: 0.0,
                kyle_lambda: self.current_kyle_lambda,
                toxicity_metric: 0.0,
                regime: OfiToxicityRegime::Benign,
                shaded_reservation_price: r_px,
                total_skew_offset: offset,
                should_evacuate_passive_quotes: false,
            };
        }

        let ofi_surface = self
            .compute_multi_level_ofi(&self.prev_levels[..self.prev_levels_count], current_levels);
        let delta_p = current_mid - self.prev_mid_price.unwrap_or(current_mid);

        // Recursive EWMA estimation of Kyle's lambda
        let beta = self.config.ewma_decay_beta.clamp(0.50, 0.999);
        let one_minus_beta = 1.0 - beta;

        let prev_mean_ofi = self.ewma_mean_ofi;
        let prev_mean_dp = self.ewma_mean_delta_p;

        self.ewma_mean_ofi = beta * self.ewma_mean_ofi + one_minus_beta * ofi_surface;
        self.ewma_mean_delta_p = beta * self.ewma_mean_delta_p + one_minus_beta * delta_p;

        let diff_ofi = ofi_surface - prev_mean_ofi;
        let diff_dp = delta_p - prev_mean_dp;

        self.ewma_var_ofi = (beta * self.ewma_var_ofi + one_minus_beta * diff_ofi * diff_ofi)
            .max(self.config.variance_epsilon);
        self.ewma_cov_delta_p_ofi =
            beta * self.ewma_cov_delta_p_ofi + one_minus_beta * diff_dp * diff_ofi;

        let raw_lambda =
            self.ewma_cov_delta_p_ofi / (self.ewma_var_ofi + self.config.variance_epsilon);
        // Kyle's lambda price impact is clamped non-negative
        self.current_kyle_lambda = raw_lambda.max(0.0);
        self.updates_count += 1;

        // Toxicity metric: adverse selection magnitude
        let toxicity_metric = ofi_surface.abs() * self.current_kyle_lambda;

        let regime = if toxicity_metric > self.config.toxicity_threshold {
            if ofi_surface > 0.0 {
                OfiToxicityRegime::ToxicBidSurge
            } else {
                OfiToxicityRegime::ToxicAskSurge
            }
        } else if toxicity_metric > 0.40 * self.config.toxicity_threshold {
            OfiToxicityRegime::MildImbalance
        } else {
            OfiToxicityRegime::Benign
        };

        let should_evacuate = matches!(
            regime,
            OfiToxicityRegime::ToxicBidSurge | OfiToxicityRegime::ToxicAskSurge
        );

        let (r_px, offset) = self.calculate_reservation_price(
            current_mid,
            inventory_q,
            asset_vol,
            horizon_t,
            ofi_surface,
            self.current_kyle_lambda,
        );

        // Update stored states
        let count = current_levels.len().min(10);
        self.prev_levels[..count].copy_from_slice(&current_levels[..count]);
        self.prev_levels_count = count;
        self.prev_mid_price = Some(current_mid);

        OfiToxicityEvaluation {
            ofi_surface,
            kyle_lambda: self.current_kyle_lambda,
            toxicity_metric,
            regime,
            shaded_reservation_price: r_px,
            total_skew_offset: offset,
            should_evacuate_passive_quotes: should_evacuate,
        }
    }

    pub fn current_lambda(&self) -> f64 {
        self.current_kyle_lambda
    }

    pub fn reset_state(&mut self) {
        self.prev_levels = [DepthLevelQuote::default(); 10];
        self.prev_levels_count = 0;
        self.prev_mid_price = None;
        self.ewma_mean_ofi = 0.0;
        self.ewma_mean_delta_p = 0.0;
        self.ewma_var_ofi = 1.0;
        self.ewma_cov_delta_p_ofi = 0.01;
        self.current_kyle_lambda = 0.01;
        self.updates_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_level_imbalance_and_micro_price() {
        let config = MultiLevelMicroPriceConfig::default();
        let surface = MultiLevelMicroPriceSurface::new(config);

        // Symmetric book -> imbalance 0, micro-price equals mid-price
        let bids = vec![10.0, 8.0, 6.0, 4.0, 2.0];
        let asks = vec![10.0, 8.0, 6.0, 4.0, 2.0];
        let imb = surface.compute_multi_level_imbalance(&bids, &asks, 0.0, 0.0);
        assert!((imb - 0.0).abs() < 1e-6);

        let mid = 60000.0;
        let spread = 2.0;
        let px = surface.compute_surface_micro_price(mid, spread, &bids, &asks, 0.0, 0.0);
        assert!((px - mid).abs() < 1e-4);

        // Heavy bid side -> positive imbalance, micro-price pulled toward ask
        let bids_heavy = vec![50.0, 40.0, 30.0, 20.0, 10.0];
        let imb_heavy = surface.compute_multi_level_imbalance(&bids_heavy, &asks, 0.0, 0.0);
        assert!(imb_heavy > 0.40);

        let px_heavy =
            surface.compute_surface_micro_price(mid, spread, &bids_heavy, &asks, 0.0, 0.0);
        assert!(px_heavy > mid);
        assert!(px_heavy <= mid + spread * 0.5);
    }

    #[test]
    fn test_online_gradient_descent_update_adapts_probabilities() {
        let config = MultiLevelMicroPriceConfig {
            learning_rate: 0.10,
            ..Default::default()
        };
        let mut surface = MultiLevelMicroPriceSurface::new(config);

        // Feature vector indicating strong positive jump signal
        let features = [0.80, 2.50, 0.50, 1.20, -0.10, 0.005];
        let initial_probs = surface.predict_jump_probabilities(&features);

        // Train with positive jump target (class 0: Up) repeatedly
        for _ in 0..15 {
            surface.online_update(&features, 0);
        }

        let adapted_probs = surface.predict_jump_probabilities(&features);
        assert!(
            adapted_probs[0] > initial_probs[0],
            "P(Up) must increase after positive jump updates"
        );
        assert!(adapted_probs[0] > 0.60);
        assert!(adapted_probs[2] < 0.20);
    }

    #[test]
    fn test_anticipatory_sniping_triggers_and_rejects() {
        let config = MultiLevelMicroPriceConfig {
            min_hurdle_bps: 1.0,
            ..Default::default()
        };
        let surface = MultiLevelMicroPriceSurface::new(config);

        // Strong positive features (leader surging up)
        let strong_up_features = [0.90, 3.50, 1.20, 1.50, 0.10, 0.01];

        // Low friction follower book -> should trigger SnipeBuy
        let result = surface.evaluate_anticipatory_sniping(
            &strong_up_features,
            1.0,  // spread 1 bps
            0.5,  // taker fee 0.5 bps
            0.5,  // exit fee 0.5 bps
            12.0, // expected jump 12 bps
            0.02, // 20ms jitter
            0.50, // 50% annual vol -> ~0.014 bps jitter
        );

        match result.decision {
            AnticipatorySnipingDecision::SnipeBuy => {
                assert!(result.expected_drift_bps > 0.0);
                assert!(result.net_edge_bps >= 1.0);
            }
            other => panic!("Expected SnipeBuy, got {:?}", other),
        }

        // Neutral features -> HoldNoAdvantage
        let neutral_features = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let neutral_res = surface.evaluate_anticipatory_sniping(
            &neutral_features,
            2.0,
            1.0,
            1.0,
            5.0,
            0.02,
            0.50,
        );
        assert_eq!(
            neutral_res.decision,
            AnticipatorySnipingDecision::HoldNoAdvantage
        );
    }

    #[test]
    fn test_multi_level_ofi_calculation_and_decay() {
        let config = MultiLevelOfiConfig {
            depth_levels: 3,
            depth_decay_kappa: 0.50, // decay weights: e^0 = 1.0, e^-0.5 = 0.6065, e^-1.0 = 0.3679
            ..Default::default()
        };
        let engine = MultiLevelOfiToxicityEngine::new(config);

        let t0_levels = vec![
            DepthLevelQuote::new(100.0, 10.0, 101.0, 10.0),
            DepthLevelQuote::new(99.0, 20.0, 102.0, 20.0),
            DepthLevelQuote::new(98.0, 30.0, 103.0, 30.0),
        ];

        // t1: Bid level 0 size increases from 10 to 15, Ask level 0 size drops from 10 to 8
        // Level 0: delta_bid = 15 - 10 = +5, delta_ask = 8 - 10 = -2 => ofi_0 = 5 - (-2) = +7
        // Level 1: identical => ofi_1 = 0
        // Level 2: identical => ofi_2 = 0
        let t1_levels = vec![
            DepthLevelQuote::new(100.0, 15.0, 101.0, 8.0),
            DepthLevelQuote::new(99.0, 20.0, 102.0, 20.0),
            DepthLevelQuote::new(98.0, 30.0, 103.0, 30.0),
        ];

        let ofi = engine.compute_multi_level_ofi(&t0_levels, &t1_levels);
        assert!(
            (ofi - 7.0).abs() < 1e-6,
            "OFI should be exactly 7.0 at level 0, got {}",
            ofi
        );

        // t2: Higher bid price entered at level 0 (e.g. 100.5 with size 12)
        // Level 0: curr_bid_px (100.5) > prev_bid_px (100.0) => delta_bid = 12
        // delta_ask = 8 - 8 = 0 => ofi_0 = 12
        let t2_levels = vec![
            DepthLevelQuote::new(100.5, 12.0, 101.0, 8.0),
            DepthLevelQuote::new(99.0, 20.0, 102.0, 20.0),
            DepthLevelQuote::new(98.0, 30.0, 103.0, 30.0),
        ];
        let ofi_higher_bid = engine.compute_multi_level_ofi(&t1_levels, &t2_levels);
        assert!((ofi_higher_bid - 12.0).abs() < 1e-6);
    }

    #[test]
    fn test_kyles_lambda_online_recursive_estimation() {
        let config = MultiLevelOfiConfig {
            ewma_decay_beta: 0.80,
            ..Default::default()
        };
        let mut engine = MultiLevelOfiToxicityEngine::new(config);

        let base_levels = vec![
            DepthLevelQuote::new(50000.0, 5.0, 50001.0, 5.0),
            DepthLevelQuote::new(49999.0, 10.0, 50002.0, 10.0),
        ];

        // Step 1: Initialise
        let eval0 = engine.update(&base_levels, 50000.5, 0.0, 0.50, 1.0);
        assert_eq!(eval0.regime, OfiToxicityRegime::Benign);

        // Feed positively correlated price changes and OFI
        let mut mid = 50000.5;
        for i in 1..=20 {
            mid += 0.5; // positive delta_p
            let curr = vec![
                DepthLevelQuote::new(mid - 0.5, 5.0 + i as f64 * 2.0, mid + 0.5, 3.0),
                DepthLevelQuote::new(mid - 1.5, 10.0, mid + 1.5, 10.0),
            ];
            let eval = engine.update(&curr, mid, 0.0, 0.50, 1.0);
            assert!(eval.ofi_surface > 0.0);
        }

        // Kyle's lambda should adapt to be strictly positive
        assert!(engine.current_lambda() > 0.0);
    }

    #[test]
    fn test_avellaneda_stoikov_reservation_price_and_toxicity_evacuation() {
        let config = MultiLevelOfiConfig {
            toxicity_threshold: 0.05,
            ofi_skew_theta: 1.0,
            inventory_gamma: 0.10,
            ..Default::default()
        };
        let mut engine = MultiLevelOfiToxicityEngine::new(config);

        let initial_levels = vec![
            DepthLevelQuote::new(100.0, 10.0, 101.0, 10.0),
            DepthLevelQuote::new(99.0, 20.0, 102.0, 20.0),
        ];
        engine.update(&initial_levels, 100.5, 0.0, 0.40, 1.0);

        // Huge surge of aggressive buying (bids stack up, asks consumed)
        let toxic_bid_levels = vec![
            DepthLevelQuote::new(100.5, 150.0, 101.0, 1.0),
            DepthLevelQuote::new(100.0, 100.0, 102.0, 5.0),
        ];
        let eval_toxic = engine.update(&toxic_bid_levels, 100.75, 0.0, 0.40, 1.0);

        // Shaded reservation price must increase (reservation price shifted upwards)
        assert!(eval_toxic.shaded_reservation_price > 100.75);
        assert!(eval_toxic.total_skew_offset > 0.0);

        // If toxicity exceeds threshold, evacuation flag is true
        if eval_toxic.toxicity_metric > config.toxicity_threshold {
            assert!(eval_toxic.should_evacuate_passive_quotes);
            assert_eq!(eval_toxic.regime, OfiToxicityRegime::ToxicBidSurge);
        }

        // Verify inventory skew: holding positive inventory (long) shades reservation price downward
        let (r_long, offset_long) =
            engine.calculate_reservation_price(100.0, 5.0, 0.40, 1.0, 0.0, 0.01);
        assert!(r_long < 100.0);
        assert!(offset_long < 0.0);

        // Holding short inventory shades reservation price upward
        let (r_short, offset_short) =
            engine.calculate_reservation_price(100.0, -5.0, 0.40, 1.0, 0.0, 0.01);
        assert!(r_short > 100.0);
        assert!(offset_short > 0.0);
    }
}
