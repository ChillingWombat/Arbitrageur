use arbitrageur_core::types::{dec, Decimal};

/// Kelly Criterion capital allocation for statistical arbitrage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KellySizing {
    /// Full Kelly fraction of portfolio f*
    pub full_kelly: f64,
    /// Half Kelly fraction (standard conservative practice in quantitative trading)
    pub half_kelly: f64,
    /// Quarter Kelly fraction
    pub quarter_kelly: f64,
}

impl KellySizing {
    /// Compute Kelly optimal leverage/fraction for a continuous strategy:
    /// f* = (\mu - r) / \sigma^2
    /// where \mu is annualized expected return, r is risk-free borrowing rate,
    /// and \sigma is annualized volatility.
    pub fn compute_continuous(expected_return: f64, risk_free_rate: f64, volatility: f64) -> Self {
        if volatility <= 1e-6 {
            return Self {
                full_kelly: 0.0,
                half_kelly: 0.0,
                quarter_kelly: 0.0,
            };
        }
        let var = volatility * volatility;
        let full = (expected_return - risk_free_rate) / var;
        // Cap Kelly at reasonable maximum (e.g., 3x leverage) to prevent ruin
        let bounded = full.clamp(0.0, 3.0);
        Self {
            full_kelly: bounded,
            half_kelly: bounded * 0.5,
            quarter_kelly: bounded * 0.25,
        }
    }

    /// Discrete win-loss Kelly criterion:
    /// f* = (p * b - q) / b
    /// where p is win probability, q = 1 - p, b is payoff ratio (win_amount / loss_amount).
    pub fn compute_discrete(win_prob: f64, win_loss_ratio: f64) -> Self {
        if win_loss_ratio <= 1e-6 || win_prob <= 0.0 || win_prob >= 1.0 {
            return Self {
                full_kelly: 0.0,
                half_kelly: 0.0,
                quarter_kelly: 0.0,
            };
        }
        let q = 1.0 - win_prob;
        let full = (win_prob * win_loss_ratio - q) / win_loss_ratio;
        let bounded = full.clamp(0.0, 1.0);
        Self {
            full_kelly: bounded,
            half_kelly: bounded * 0.5,
            quarter_kelly: bounded * 0.25,
        }
    }
}

/// Parametric 95% and 99% Value at Risk (VaR) and Expected Shortfall (CVaR).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParametricRiskMetrics {
    /// 95% 1-day Value at Risk (in fraction of portfolio, e.g. 0.02 = 2%)
    pub var_95: f64,
    /// 99% 1-day Value at Risk
    pub var_99: f64,
    /// 95% Expected Shortfall (Conditional VaR)
    pub cvar_95: f64,
    /// 99% Expected Shortfall (Conditional VaR)
    pub cvar_99: f64,
}

impl ParametricRiskMetrics {
    pub fn compute(daily_mean: f64, daily_volatility: f64) -> Self {
        // Standard normal quantiles: Z_0.95 = 1.644853, Z_0.99 = 2.326348
        let z_95 = 1.6448536269514722;
        let z_99 = 2.3263478740408408;

        let var_95 = (z_95 * daily_volatility - daily_mean).max(0.0);
        let var_99 = (z_99 * daily_volatility - daily_mean).max(0.0);

        // CVaR for standard normal: E[X | X > Z] = \phi(Z) / (1 - \Phi(Z))
        let pdf_95 = (-0.5 * z_95 * z_95).exp() / (2.0 * std::f64::consts::PI).sqrt();
        let pdf_99 = (-0.5 * z_99 * z_99).exp() / (2.0 * std::f64::consts::PI).sqrt();

        let cvar_95 = (daily_volatility * (pdf_95 / 0.05) - daily_mean).max(0.0);
        let cvar_99 = (daily_volatility * (pdf_99 / 0.01) - daily_mean).max(0.0);

        Self {
            var_95,
            var_99,
            cvar_95,
            cvar_99,
        }
    }
}

/// Calculate maximum order quantity given strict risk limit:
/// Max notional = min(Available Capital * Max Leverage, Max Dollar Drawdown / Stop Loss %)
pub fn calculate_max_risk_sized_qty(
    available_capital: Decimal,
    max_leverage: Decimal,
    price: Decimal,
    max_drawdown_usd: Decimal,
    stop_loss_pct: Decimal,
) -> Decimal {
    if price <= dec!(0) {
        return dec!(0);
    }
    let capital_notional = available_capital * max_leverage;
    let drawdown_notional = if stop_loss_pct > dec!(0) {
        max_drawdown_usd / stop_loss_pct
    } else {
        capital_notional
    };

    let allowed_notional = capital_notional.min(drawdown_notional);
    allowed_notional / price
}
