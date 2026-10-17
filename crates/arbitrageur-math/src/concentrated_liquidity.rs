//! Concentrated Liquidity AMM (Uniswap v3 / Aerodrome Slipstream) Virtual Reserves,
//! Discrete Tick Crossing, and DEX-CLOB Spatial Arbitrage Optimization.
//!
//! Provides:
//! 1. **Virtual Reserve Math**: Exact closed-form transitions for square-root price
//!    $\sqrt{P}$, virtual liquidity $L$, and token deltas:
//!    $$\Delta x = L \left(\frac{1}{\sqrt{P_2}} - \frac{1}{\sqrt{P_1}}\right) = L \frac{\sqrt{P_1} - \sqrt{P_2}}{\sqrt{P_1} \sqrt{P_2}}$$
//!    $$\Delta y = L (\sqrt{P_2} - \sqrt{P_1})$$
//! 2. **Multi-Tick Discrete Traversal**: Stepwise boundary walking across initialized ticks,
//!    updating active liquidity $L \leftarrow L \pm \Delta L(i)$ and deducting per-step fees.
//! 3. **DEX-CLOB Spatial Arbitrage Optimization**:
//!    - **Marginal Price Equivalence**:
//!      $$P_{\text{clob}}^{\text{marginal}}(q^*) = P_{\text{dex}}^{\text{marginal}}(q^*)$$
//!    - **Closed-Form Optimal Sizing $q^*$**:
//!      $$q^* = \frac{P_{\text{bid}, 0}(1 - f_{\text{clob}}) - P_{\text{dex}, 0}(1 + f_{\text{dex}})}{\lambda_{\text{clob}}(1 - f_{\text{clob}}) + \frac{2 P_0^{3/2}}{L}(1 + f_{\text{dex}})}$$
//!    - **Gas Hurdle & Break-Even Dislocation**:
//!      $$\text{SpreadBps}_{\text{hurdle}} = (f_{\text{clob}} + f_{\text{dex}}) \times 10^4 + \frac{2 \sqrt{C_{\text{gas}} \cdot \Lambda_{\text{eff}}}}{P_0} \times 10^4$$
//! 4. **Loss-Versus-Rebalancing (LVR)**: Instantaneous profit extraction velocity:
//!    $$\frac{d\text{LVR}}{dt} = \frac{1}{8} \sigma^2 L \sqrt{P}$$

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Scale factor $2^{96}$ used in Uniswap v3 / Slipstream fixed-point arithmetic.
pub const Q96: f64 = 79228162514264337593543950336.0;

/// Converts `sqrtPriceX96` fixed point integer to human-readable price $P = (\sqrt{P})^2$.
#[inline]
pub fn sqrt_price_x96_to_price(sqrt_price_x96: u128) -> f64 {
    let sqrt_p = (sqrt_price_x96 as f64) / Q96;
    sqrt_p * sqrt_p
}

/// Converts human-readable price $P$ to `sqrtPriceX96` representation.
#[inline]
pub fn price_to_sqrt_price_x96(price: f64) -> u128 {
    if price <= 0.0 {
        return 0;
    }
    let sqrt_p = price.sqrt();
    (sqrt_p * Q96).floor() as u128
}

/// Converts discrete tick index $i \in \mathbb{Z}$ to `sqrtPriceX96`:
/// $$\sqrt{P(i)} = 1.0001^{i / 2}$$
#[inline]
pub fn tick_to_sqrt_price_x96(tick: i32) -> u128 {
    // 1.0001^(tick / 2) = exp(tick * ln(1.0001) / 2)
    let ln_1_0001 = 0.00009999500033330557_f64; // ln(1.0001)
    let sqrt_p = ((tick as f64) * 0.5 * ln_1_0001).exp();
    (sqrt_p * Q96).floor() as u128
}

/// Converts `sqrtPriceX96` to the nearest lower discrete tick index $i$:
/// $$i = \left\lfloor \frac{2 \ln(\sqrt{P})}{\ln(1.0001)} \right\rfloor$$
#[inline]
pub fn sqrt_price_x96_to_tick(sqrt_price_x96: u128) -> i32 {
    if sqrt_price_x96 == 0 {
        return i32::MIN;
    }
    let sqrt_p = (sqrt_price_x96 as f64) / Q96;
    let ln_1_0001 = 0.00009999500033330557_f64;
    (2.0 * sqrt_p.ln() / ln_1_0001).floor() as i32
}

/// Computes the exact amount of Token 0 required/produced for a price movement
/// between `sqrt_p_a` and `sqrt_p_b` with active liquidity $L$:
/// $$\Delta x = L \cdot \left(\frac{1}{\sqrt{P_a}} - \frac{1}{\sqrt{P_b}}\right)$$
#[inline]
pub fn compute_dx(liquidity: f64, sqrt_p_a: f64, sqrt_p_b: f64) -> f64 {
    if liquidity <= 0.0 || sqrt_p_a <= 0.0 || sqrt_p_b <= 0.0 {
        return 0.0;
    }
    let (p_low, p_high) = if sqrt_p_a < sqrt_p_b {
        (sqrt_p_a, sqrt_p_b)
    } else {
        (sqrt_p_b, sqrt_p_a)
    };
    liquidity * (1.0 / p_low - 1.0 / p_high)
}

/// Computes the exact amount of Token 1 required/produced for a price movement
/// between `sqrt_p_a` and `sqrt_p_b` with active liquidity $L$:
/// $$\Delta y = L \cdot (\sqrt{P_b} - \sqrt{P_a})$$
#[inline]
pub fn compute_dy(liquidity: f64, sqrt_p_a: f64, sqrt_p_b: f64) -> f64 {
    if liquidity <= 0.0 || sqrt_p_a <= 0.0 || sqrt_p_b <= 0.0 {
        return 0.0;
    }
    let (p_low, p_high) = if sqrt_p_a < sqrt_p_b {
        (sqrt_p_a, sqrt_p_b)
    } else {
        (sqrt_p_b, sqrt_p_a)
    };
    liquidity * (p_high - p_low)
}

/// Computes next $\sqrt{P_{\text{next}}}$ when selling Token 0 into pool ($\Delta x_{\text{net}} > 0$).
/// Price decreases:
/// $$\sqrt{P_{\text{next}}} = \frac{L \sqrt{P}}{L + \Delta x_{\text{net}} \sqrt{P}}$$
#[inline]
pub fn compute_next_sqrt_p_from_dx_in(liquidity: f64, current_sqrt_p: f64, dx_net: f64) -> f64 {
    if liquidity <= 0.0 || current_sqrt_p <= 0.0 || dx_net <= 0.0 {
        return current_sqrt_p;
    }
    (liquidity * current_sqrt_p) / (liquidity + dx_net * current_sqrt_p)
}

/// Computes next $\sqrt{P_{\text{next}}}$ when selling Token 1 into pool ($\Delta y_{\text{net}} > 0$).
/// Price increases:
/// $$\sqrt{P_{\text{next}}} = \sqrt{P} + \frac{\Delta y_{\text{net}}}{L}$$
#[inline]
pub fn compute_next_sqrt_p_from_dy_in(liquidity: f64, current_sqrt_p: f64, dy_net: f64) -> f64 {
    if liquidity <= 0.0 || current_sqrt_p <= 0.0 || dy_net <= 0.0 {
        return current_sqrt_p;
    }
    current_sqrt_p + (dy_net / liquidity)
}

/// Initialized tick state within a concentrated liquidity pool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InitializedTick {
    pub tick_index: i32,
    /// Net liquidity change $\Delta L$ when price crosses this tick from left to right ($P$ increasing).
    /// When crossing from right to left ($P$ decreasing), $-\Delta L$ is applied.
    pub liquidity_net: i128,
}

/// Swap execution simulation result through a concentrated liquidity AMM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConcentratedSwapResult {
    /// Total amount of output tokens received.
    pub amount_out: f64,
    /// Total fee deducted from input token in input units.
    pub fee_paid: f64,
    /// Effective volume-weighted average price (VWAP) = output / input.
    pub effective_vwap: f64,
    /// Final square-root price $X96$ after completing the swap.
    pub final_sqrt_price_x96: u128,
    /// Number of initialized tick boundaries traversed during execution.
    pub ticks_crossed: usize,
    /// Price slippage relative to initial spot price, in basis points.
    pub slippage_bps: f64,
}

/// State of a concentrated liquidity pool (Uniswap v3 / Slipstream).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcentratedPoolState {
    pub current_sqrt_price_x96: u128,
    pub current_liquidity: u128,
    /// Pool fee in basis points (e.g. 5.0 for 0.05%, 30.0 for 0.30%, 100.0 for 1.00%).
    pub fee_bps: f64,
    /// Initialized tick bitmap/map indexed by tick.
    pub ticks: BTreeMap<i32, InitializedTick>,
}

impl ConcentratedPoolState {
    pub fn new(current_sqrt_price_x96: u128, current_liquidity: u128, fee_bps: f64) -> Self {
        Self {
            current_sqrt_price_x96,
            current_liquidity,
            fee_bps: fee_bps.max(0.0),
            ticks: BTreeMap::new(),
        }
    }

    /// Adds or updates an initialized tick.
    pub fn insert_tick(&mut self, tick_index: i32, liquidity_net: i128) {
        self.ticks.insert(
            tick_index,
            InitializedTick {
                tick_index,
                liquidity_net,
            },
        );
    }

    /// Computes current spot price $P = (\sqrt{P})^2$.
    pub fn spot_price(&self) -> f64 {
        sqrt_price_x96_to_price(self.current_sqrt_price_x96)
    }

    /// Simulates exact input of Token 0 (selling Token 0, buying Token 1).
    /// Price moves DOWN ($\sqrt{P}$ decreases).
    pub fn simulate_swap_token0_in(&self, amount_in: f64) -> Option<ConcentratedSwapResult> {
        if amount_in <= 0.0 || self.current_liquidity == 0 {
            return None;
        }

        let fee_rate = self.fee_bps / 10_000.0;
        let mut rem_in = amount_in;
        let mut total_out = 0.0;
        let mut total_fee = 0.0;
        let mut curr_sqrt_p = (self.current_sqrt_price_x96 as f64) / Q96;
        let mut curr_l = self.current_liquidity as f64;
        let mut ticks_crossed = 0;
        let initial_spot = curr_sqrt_p * curr_sqrt_p;

        // Traverse ticks descending (price moving down)
        let current_tick = sqrt_price_x96_to_tick(self.current_sqrt_price_x96);
        let relevant_ticks: Vec<&InitializedTick> = self
            .ticks
            .range(..=current_tick)
            .rev()
            .map(|(_, t)| t)
            .collect();

        for tick in relevant_ticks {
            if rem_in <= 1e-12 || curr_l <= 1e-6 {
                break;
            }
            let target_sqrt_p = (tick_to_sqrt_price_x96(tick.tick_index) as f64) / Q96;
            if target_sqrt_p >= curr_sqrt_p {
                continue;
            }

            // Max dx to reach target tick: dx_max = L * (1/target - 1/curr) / (1 - fee)
            let dx_max = compute_dx(curr_l, target_sqrt_p, curr_sqrt_p) / (1.0 - fee_rate);

            if rem_in <= dx_max {
                // Swap finishes strictly within this tick range
                let fee = rem_in * fee_rate;
                let dx_net = rem_in - fee;
                let next_sqrt_p = compute_next_sqrt_p_from_dx_in(curr_l, curr_sqrt_p, dx_net);
                let dy = compute_dy(curr_l, next_sqrt_p, curr_sqrt_p);

                total_out += dy;
                total_fee += fee;
                curr_sqrt_p = next_sqrt_p;
                rem_in = 0.0;
                break;
            } else {
                // Exhaust this tick range, cross boundary
                let fee = dx_max * fee_rate;
                let dy = compute_dy(curr_l, target_sqrt_p, curr_sqrt_p);

                total_out += dy;
                total_fee += fee;
                rem_in -= dx_max;
                curr_sqrt_p = target_sqrt_p;
                ticks_crossed += 1;

                // When price moves down (crossing tick right to left), subtract liquidity_net
                curr_l = (curr_l - (tick.liquidity_net as f64)).max(0.0);
            }
        }

        // If remaining input left after passing all ticks, execute in infinite tail
        if rem_in > 1e-12 && curr_l > 1e-6 {
            let fee = rem_in * fee_rate;
            let dx_net = rem_in - fee;
            let next_sqrt_p = compute_next_sqrt_p_from_dx_in(curr_l, curr_sqrt_p, dx_net);
            let dy = compute_dy(curr_l, next_sqrt_p, curr_sqrt_p);
            total_out += dy;
            total_fee += fee;
            curr_sqrt_p = next_sqrt_p;
        }

        let effective_vwap = if amount_in > 0.0 {
            total_out / amount_in
        } else {
            0.0
        };
        let final_spot = curr_sqrt_p * curr_sqrt_p;
        let slippage_bps = ((initial_spot - final_spot) / initial_spot).max(0.0) * 10_000.0;
        let final_sqrt_price_x96 = (curr_sqrt_p * Q96).floor() as u128;

        Some(ConcentratedSwapResult {
            amount_out: total_out,
            fee_paid: total_fee,
            effective_vwap,
            final_sqrt_price_x96,
            ticks_crossed,
            slippage_bps,
        })
    }

    /// Simulates exact input of Token 1 (selling Token 1, buying Token 0).
    /// Price moves UP ($\sqrt{P}$ increases).
    pub fn simulate_swap_token1_in(&self, amount_in: f64) -> Option<ConcentratedSwapResult> {
        if amount_in <= 0.0 || self.current_liquidity == 0 {
            return None;
        }

        let fee_rate = self.fee_bps / 10_000.0;
        let mut rem_in = amount_in;
        let mut total_out = 0.0;
        let mut total_fee = 0.0;
        let mut curr_sqrt_p = (self.current_sqrt_price_x96 as f64) / Q96;
        let mut curr_l = self.current_liquidity as f64;
        let mut ticks_crossed = 0;
        let initial_spot = curr_sqrt_p * curr_sqrt_p;

        let current_tick = sqrt_price_x96_to_tick(self.current_sqrt_price_x96);
        let relevant_ticks: Vec<&InitializedTick> =
            self.ticks.range(current_tick..).map(|(_, t)| t).collect();

        for tick in relevant_ticks {
            if rem_in <= 1e-12 || curr_l <= 1e-6 {
                break;
            }
            let target_sqrt_p = (tick_to_sqrt_price_x96(tick.tick_index) as f64) / Q96;
            if target_sqrt_p <= curr_sqrt_p {
                continue;
            }

            // Max dy to reach target tick: dy_max = L * (target - curr) / (1 - fee)
            let dy_max = compute_dy(curr_l, curr_sqrt_p, target_sqrt_p) / (1.0 - fee_rate);

            if rem_in <= dy_max {
                let fee = rem_in * fee_rate;
                let dy_net = rem_in - fee;
                let next_sqrt_p = compute_next_sqrt_p_from_dy_in(curr_l, curr_sqrt_p, dy_net);
                let dx = compute_dx(curr_l, curr_sqrt_p, next_sqrt_p);

                total_out += dx;
                total_fee += fee;
                curr_sqrt_p = next_sqrt_p;
                rem_in = 0.0;
                break;
            } else {
                let fee = dy_max * fee_rate;
                let dx = compute_dx(curr_l, curr_sqrt_p, target_sqrt_p);

                total_out += dx;
                total_fee += fee;
                rem_in -= dy_max;
                curr_sqrt_p = target_sqrt_p;
                ticks_crossed += 1;

                // When price moves up (crossing tick left to right), add liquidity_net
                curr_l = (curr_l + (tick.liquidity_net as f64)).max(0.0);
            }
        }

        if rem_in > 1e-12 && curr_l > 1e-6 {
            let fee = rem_in * fee_rate;
            let dy_net = rem_in - fee;
            let next_sqrt_p = compute_next_sqrt_p_from_dy_in(curr_l, curr_sqrt_p, dy_net);
            let dx = compute_dx(curr_l, curr_sqrt_p, next_sqrt_p);
            total_out += dx;
            total_fee += fee;
            curr_sqrt_p = next_sqrt_p;
        }

        let effective_vwap = if total_out > 0.0 {
            amount_in / total_out // Quote per base
        } else {
            0.0
        };
        let final_spot = curr_sqrt_p * curr_sqrt_p;
        let slippage_bps = ((final_spot - initial_spot) / initial_spot).max(0.0) * 10_000.0;
        let final_sqrt_price_x96 = (curr_sqrt_p * Q96).floor() as u128;

        Some(ConcentratedSwapResult {
            amount_out: total_out,
            fee_paid: total_fee,
            effective_vwap,
            final_sqrt_price_x96,
            ticks_crossed,
            slippage_bps,
        })
    }
}

/// Parameters for spatial arbitrage optimization between a CLOB and a concentrated DEX pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexClobArbInput {
    /// Best available bid on the CLOB (buying base asset on DEX, selling on CLOB).
    pub clob_bid_price: f64,
    /// Available depth quantity on the CLOB at top-of-book.
    pub clob_depth_qty: f64,
    /// CLOB taker fee in basis points (e.g. 2.0 bps).
    pub clob_fee_bps: f64,
    /// Spot price on the concentrated liquidity DEX.
    pub dex_spot_price: f64,
    /// Active virtual liquidity $L$ of the concentrated DEX pool.
    pub dex_active_liquidity: f64,
    /// Concentrated DEX pool fee in basis points (e.g. 5.0 bps or 30.0 bps).
    pub dex_fee_bps: f64,
    /// Estimated gas units for the on-chain swap transaction (e.g. 145,000).
    pub gas_units: u64,
    /// Current gas price in Gwei.
    pub gas_price_gwei: f64,
    /// Native gas token price in USD (e.g. ETH at $2,700).
    pub native_token_usd: f64,
    /// Estimated sequencer/block mining latency in seconds.
    pub estimated_latency_sec: f64,
    /// Maximum allowable block inclusion window in seconds before abort (e.g. 0.350s).
    pub max_latency_sec: f64,
    /// Minimum required net profit in USD to authorize execution (e.g. $2.50).
    pub min_profit_threshold_usd: f64,
}

/// Evaluated execution verdict and optimal trade sizing for DEX-CLOB spatial arbitrage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexClobArbVerdict {
    /// Whether all hurdle, gas, depth, and latency conditions are satisfied for execution.
    pub should_execute: bool,
    /// Depth-optimal trade quantity in base token units $q^*$.
    pub optimal_qty: f64,
    /// Expected dollar net profit after fees, quadratic slippage, and gas cost.
    pub expected_net_profit_usd: f64,
    /// Initial gross price spread in basis points.
    pub gross_spread_bps: f64,
    /// Minimum required spread hurdle in basis points to break even after gas and fees.
    pub hurdle_spread_bps: f64,
    /// Total gas cost in USD.
    pub gas_cost_usd: f64,
    /// Abort or skip explanation if `should_execute` is false.
    pub skip_reason: Option<String>,
}

/// Solves the depth-optimal trade size $q^*$ and evaluates economic viability
/// for DEX-CLOB spatial arbitrage under finite orderbook depth and quadratic AMM slippage.
pub fn evaluate_dex_clob_spatial_arbitrage(input: &DexClobArbInput) -> DexClobArbVerdict {
    // 1. Gas cost calculation in USD: G * g_gas * 10^-9 * P_native
    let gas_cost_usd =
        (input.gas_units as f64) * input.gas_price_gwei * 1e-9 * input.native_token_usd;

    // 2. Fee fractions
    let f_clob = input.clob_fee_bps / 10_000.0;
    let f_dex = input.dex_fee_bps / 10_000.0;

    let net_clob_bid = input.clob_bid_price * (1.0 - f_clob);
    let gross_dex_ask = input.dex_spot_price * (1.0 + f_dex);

    let spread_diff = net_clob_bid - gross_dex_ask;
    let gross_spread_bps = if input.dex_spot_price > 0.0 {
        ((input.clob_bid_price - input.dex_spot_price) / input.dex_spot_price) * 10_000.0
    } else {
        0.0
    };

    // Pre-flight check: positive initial dislocation
    if spread_diff <= 0.0 {
        return DexClobArbVerdict {
            should_execute: false,
            optimal_qty: 0.0,
            expected_net_profit_usd: 0.0,
            gross_spread_bps,
            hurdle_spread_bps: (input.clob_fee_bps + input.dex_fee_bps),
            gas_cost_usd,
            skip_reason: Some(
                "Spread negative: CLOB net bid <= DEX gross ask after taker fees".into(),
            ),
        };
    }

    // 3. Marginal price impact slopes
    // CLOB impact: lambda_clob = tick_size / depth ~ 0.01 / depth
    let lambda_clob = 0.01 / input.clob_depth_qty.max(0.01);
    // AMM impact: lambda_dex = 2 * P^(3/2) / L
    let lambda_dex = (2.0 * input.dex_spot_price.powf(1.5)) / input.dex_active_liquidity.max(1.0);
    let lambda_total = lambda_clob * (1.0 - f_clob) + lambda_dex * (1.0 + f_dex);
    let lambda_eff = 0.5 * (lambda_clob + lambda_dex);

    // 4. Closed-form optimal trade size q*:
    // q* = (P_bid(1 - f_clob) - P_dex(1 + f_dex)) / [2 * (lambda_clob(1-f_clob) + lambda_dex(1+f_dex))]
    let unconstrained_q = spread_diff / (2.0 * lambda_total.max(1e-12));
    let optimal_qty = unconstrained_q.clamp(0.0001, input.clob_depth_qty).max(0.0);

    // 5. Expected Net Profit Pi(q*) = q* * spread_diff - lambda_total * (q*)^2 - gas_cost_usd
    let net_profit_usd =
        optimal_qty * spread_diff - lambda_total * optimal_qty * optimal_qty - gas_cost_usd;

    // 6. Minimum required spread hurdle in bps to overcome gas and fees:
    // SpreadBps_hurdle = (f_clob + f_dex) * 10^4 + [2 * sqrt(C_gas * lambda_eff) / P_0] * 10^4
    let gas_spread_penalty = if input.dex_spot_price > 0.0 {
        (2.0 * (gas_cost_usd * lambda_eff).sqrt() / input.dex_spot_price) * 10_000.0
    } else {
        0.0
    };
    let hurdle_spread_bps = (input.clob_fee_bps + input.dex_fee_bps) + gas_spread_penalty;

    // 7. Hurdle and latency verifications
    if net_profit_usd < input.min_profit_threshold_usd {
        return DexClobArbVerdict {
            should_execute: false,
            optimal_qty,
            expected_net_profit_usd: net_profit_usd,
            gross_spread_bps,
            hurdle_spread_bps,
            gas_cost_usd,
            skip_reason: Some(format!(
                "Net profit ${:.2} below minimum threshold ${:.2} after gas and slippage",
                net_profit_usd, input.min_profit_threshold_usd
            )),
        };
    }

    if input.estimated_latency_sec > input.max_latency_sec {
        return DexClobArbVerdict {
            should_execute: false,
            optimal_qty,
            expected_net_profit_usd: net_profit_usd,
            gross_spread_bps,
            hurdle_spread_bps,
            gas_cost_usd,
            skip_reason: Some(format!(
                "Latency hazard: estimated {:.3}s exceeds max allowable {:.3}s window",
                input.estimated_latency_sec, input.max_latency_sec
            )),
        };
    }

    DexClobArbVerdict {
        should_execute: true,
        optimal_qty,
        expected_net_profit_usd: net_profit_usd,
        gross_spread_bps,
        hurdle_spread_bps,
        gas_cost_usd,
        skip_reason: None,
    }
}

/// Computes the instantaneous Loss-Versus-Rebalancing (LVR) extraction velocity:
/// $$\frac{d\text{LVR}}{dt} = \frac{1}{8} \sigma^2 L \sqrt{P}$$
///
/// Quantifies the theoretical maximum economic rent accessible to latency-advantaged arbitrageurs
/// against passive liquidity providers.
#[inline]
pub fn compute_instantaneous_lvr(liquidity: f64, spot_price: f64, annualized_vol: f64) -> f64 {
    if liquidity <= 0.0 || spot_price <= 0.0 || annualized_vol <= 0.0 {
        return 0.0;
    }
    // Convert annualized volatility to per-second volatility variance rate
    let seconds_per_year = 365.25 * 86_400.0;
    let sigma_sq_per_sec = (annualized_vol * annualized_vol) / seconds_per_year;
    0.125 * sigma_sq_per_sec * liquidity * spot_price.sqrt()
}

/// Concentrated liquidity LP position with active tick range, analytical Greek hedging,
/// and optimal perpetual delta-rebalancing bandwidth under Milionis-Moallemi-Roughgarden (2022).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConcentratedLpPosition {
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub price_lower: f64,
    pub price_upper: f64,
    pub liquidity: f64,
    pub entry_price: f64,
}

impl ConcentratedLpPosition {
    pub fn new(tick_lower: i32, tick_upper: i32, liquidity: f64, entry_price: f64) -> Self {
        let p_lower = 1.0001f64.powi(tick_lower);
        let p_upper = 1.0001f64.powi(tick_upper);
        Self {
            tick_lower,
            tick_upper,
            price_lower: p_lower,
            price_upper: p_upper,
            liquidity: liquidity.max(0.0),
            entry_price: entry_price.max(1e-6),
        }
    }

    /// Checks if a given spot price is within active tick range.
    pub fn is_in_range(&self, spot_price: f64) -> bool {
        spot_price >= self.price_lower && spot_price <= self.price_upper
    }

    /// Analytical portfolio Delta w.r.t underlying spot price S:
    /// Delta_LP(S) = L * (1/sqrt(S) - 1/sqrt(P_b)) when S in [P_a, P_b]
    /// Delta_LP(S) = L * (1/sqrt(P_a) - 1/sqrt(P_b)) when S < P_a (100% Token 0)
    /// Delta_LP(S) = 0 when S > P_b (100% Token 1)
    pub fn delta(&self, spot_price: f64) -> f64 {
        if self.liquidity <= 0.0 || spot_price <= 0.0 {
            return 0.0;
        }
        let inv_sqrt_ub = 1.0 / self.price_upper.sqrt();
        if spot_price < self.price_lower {
            self.liquidity * (1.0 / self.price_lower.sqrt() - inv_sqrt_ub)
        } else if spot_price > self.price_upper {
            0.0
        } else {
            self.liquidity * (1.0 / spot_price.sqrt() - inv_sqrt_ub)
        }
    }

    /// Analytical portfolio Gamma w.r.t underlying spot price S:
    /// Gamma_LP(S) = -L / (2 * S^(3/2)) when S in [P_a, P_b], else 0.
    pub fn gamma(&self, spot_price: f64) -> f64 {
        if !self.is_in_range(spot_price) || spot_price <= 0.0 || self.liquidity <= 0.0 {
            return 0.0;
        }
        -self.liquidity / (2.0 * spot_price.powf(1.5))
    }

    /// Instantaneous LVR rate in USD per second:
    /// dLVR/dt = 1/8 * sigma^2 * L * sqrt(S) when in-range, else 0.
    pub fn instantaneous_lvr_sec(&self, spot_price: f64, annualized_vol: f64) -> f64 {
        if !self.is_in_range(spot_price) {
            return 0.0;
        }
        compute_instantaneous_lvr(self.liquidity, spot_price, annualized_vol)
    }

    /// Target perpetual short hedge quantity to maintain delta-neutrality:
    /// Q_perp* = -Delta_LP(S)
    pub fn target_perp_hedge_qty(&self, spot_price: f64) -> f64 {
        -self.delta(spot_price)
    }

    /// Optimal spot price rebalancing tolerance bandwidth Delta S*:
    /// Delta S* = S * [ (3 * c_taker * sqrt(S)) / (gamma_risk * L) ]^(1/3)
    /// Returns (delta_s_star, lower_rebalance_trigger, upper_rebalance_trigger).
    pub fn compute_rebalance_bandwidth(
        &self,
        spot_price: f64,
        taker_fee_rate: f64,
        risk_aversion_gamma: f64,
    ) -> (f64, f64, f64) {
        let s = spot_price.max(1e-6);
        let c = taker_fee_rate.max(1e-5);
        let gamma = risk_aversion_gamma.max(1e-6);
        let l = self.liquidity.max(1.0);

        let inner = (3.0 * c * s.sqrt()) / (gamma * l);
        let delta_s_star = s * inner.cbrt();

        let lower = (s - delta_s_star).max(self.price_lower);
        let upper = (s + delta_s_star).min(self.price_upper);
        (delta_s_star, lower, upper)
    }

    /// Checks economic viability of providing liquidity:
    /// Net yield rate = Fee rate - LVR rate - Amortized rehedging cost >= Hurdle
    pub fn is_economically_viable(
        &self,
        spot_price: f64,
        annualized_vol: f64,
        expected_fee_yield_annual_usd: f64,
        amortized_rehedge_annual_usd: f64,
        hurdle_yield_annual_usd: f64,
    ) -> bool {
        let lvr_sec = self.instantaneous_lvr_sec(spot_price, annualized_vol);
        let lvr_annual_usd = lvr_sec * 365.25 * 86_400.0;
        let net_yield_usd =
            expected_fee_yield_annual_usd - lvr_annual_usd - amortized_rehedge_annual_usd;
        net_yield_usd >= hurdle_yield_annual_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_and_sqrt_price_roundtrip() {
        let tick = 200_000; // Price ~ exp(200000 * 0.0001) ~ e^20 ~ 4.85e8
        let sqrt_x96 = tick_to_sqrt_price_x96(tick);
        let recovered_tick = sqrt_price_x96_to_tick(sqrt_x96);
        assert!(
            (recovered_tick - tick).abs() <= 1,
            "Tick recovery error: original {}, recovered {}",
            tick,
            recovered_tick
        );

        let price = 2500.0; // ETH price $2,500
        let p_x96 = price_to_sqrt_price_x96(price);
        let recovered_price = sqrt_price_x96_to_price(p_x96);
        assert!(
            (recovered_price - price).abs() < 1e-4,
            "Price recovery error: original {}, recovered {}",
            price,
            recovered_price
        );
    }

    #[test]
    fn test_concentrated_swap_simulation() {
        let eth_price = 2500.0;
        let sqrt_p_x96 = price_to_sqrt_price_x96(eth_price);
        let mut pool = ConcentratedPoolState::new(sqrt_p_x96, 1_000_000_000_000, 5.0); // 5 bps fee

        // Insert lower and upper ticks
        let curr_tick = sqrt_price_x96_to_tick(sqrt_p_x96);
        pool.insert_tick(curr_tick - 100, 500_000_000_000);
        pool.insert_tick(curr_tick + 100, 500_000_000_000);

        // Swap 0.5 ETH in (Token 0 -> Token 1)
        let res = pool
            .simulate_swap_token0_in(0.5)
            .expect("Swap should succeed");
        assert!(res.amount_out > 0.0);
        assert!(res.fee_paid > 0.0);
        assert!(res.effective_vwap > 0.0);
        assert!(res.effective_vwap < eth_price); // Price impact + fee lowers effective proceeds
    }

    #[test]
    fn test_dex_clob_optimal_arbitrage_verdict() {
        let input = DexClobArbInput {
            clob_bid_price: 2515.0, // Dislocation: CLOB bid $2,515 vs DEX $2,500
            clob_depth_qty: 5.0,
            clob_fee_bps: 2.0,
            dex_spot_price: 2500.0,
            dex_active_liquidity: 50_000_000.0,
            dex_fee_bps: 5.0,
            gas_units: 145_000,
            gas_price_gwei: 1.5,
            native_token_usd: 2500.0,
            estimated_latency_sec: 0.150,
            max_latency_sec: 0.350,
            min_profit_threshold_usd: 2.50,
        };

        let verdict = evaluate_dex_clob_spatial_arbitrage(&input);
        assert!(verdict.should_execute);
        assert!(verdict.optimal_qty > 0.0);
        assert!(verdict.expected_net_profit_usd >= 2.50);
        assert!(verdict.gas_cost_usd > 0.0);
    }

    #[test]
    fn test_dex_clob_gas_hurdle_rejection() {
        let input = DexClobArbInput {
            clob_bid_price: 2500.20, // Tiny 0.8 bps spread, not enough for gas
            clob_depth_qty: 0.1,
            clob_fee_bps: 2.0,
            dex_spot_price: 2500.0,
            dex_active_liquidity: 50_000_000.0,
            dex_fee_bps: 5.0,
            gas_units: 145_000,
            gas_price_gwei: 20.0, // High gas price
            native_token_usd: 2500.0,
            estimated_latency_sec: 0.100,
            max_latency_sec: 0.350,
            min_profit_threshold_usd: 2.50,
        };

        let verdict = evaluate_dex_clob_spatial_arbitrage(&input);
        assert!(!verdict.should_execute);
        assert!(verdict.skip_reason.is_some());
    }

    #[test]
    fn test_instantaneous_lvr() {
        let lvr_rate = compute_instantaneous_lvr(10_000_000.0, 2500.0, 0.80);
        assert!(lvr_rate > 0.0);
        assert!(lvr_rate.is_finite());
    }

    #[test]
    fn test_concentrated_lp_position_delta_gamma_and_rebalance() {
        // Range: 2000 to 3000, spot 2500, liquidity 100_000
        let tick_lower = (2000.0f64.ln() / 1.0001f64.ln()).round() as i32;
        let tick_upper = (3000.0f64.ln() / 1.0001f64.ln()).round() as i32;
        let pos = ConcentratedLpPosition::new(tick_lower, tick_upper, 100_000.0, 2500.0);

        assert!(pos.is_in_range(2500.0));
        assert!(!pos.is_in_range(1500.0));
        assert!(!pos.is_in_range(3500.0));

        let delta_mid = pos.delta(2500.0);
        assert!(delta_mid > 0.0);
        let gamma_mid = pos.gamma(2500.0);
        assert!(gamma_mid < 0.0); // LP is fundamentally short gamma!

        let perp_hedge = pos.target_perp_hedge_qty(2500.0);
        assert_eq!(perp_hedge, -delta_mid);

        // Optimal rebalance bandwidth
        let (bandwidth, lower_trig, upper_trig) =
            pos.compute_rebalance_bandwidth(2500.0, 0.0005, 0.01);
        assert!(bandwidth > 0.0);
        assert!(lower_trig < 2500.0);
        assert!(upper_trig > 2500.0);

        // Viability check: on ~$5M notional pool, LVR at 60% vol is ~$225k/yr.
        // A high-volume pool generating $300k/yr in fees easily dominates $225k LVR + $10k rehedge.
        let is_viable = pos.is_economically_viable(2500.0, 0.60, 300_000.0, 10_000.0, 50_000.0);
        assert!(is_viable);
        // Low fee pool ($100k/yr fees) fails economic hurdle against $225k LVR
        let is_unviable = pos.is_economically_viable(2500.0, 0.60, 100_000.0, 10_000.0, 50_000.0);
        assert!(!is_unviable);
    }
}
