use serde::{Deserialize, Serialize};

/// Candidate baseline calculation methods for market spread tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaselineEstimatorType {
    /// Simple Moving Average: equal weighting over N updates
    SMA,
    /// Exponential Moving Average: alpha = 2 / (N + 1)
    EMA,
    /// Linearly Weighted Moving Average: linear weights from 1 to N
    WMA,
    /// Volume / Depth-Weighted Moving Average: weighted by available top-of-book depth (USD)
    VWMA,
    /// Smoothed Moving Average (Wilder's MA): alpha = 1 / N
    SMMA,
}

impl BaselineEstimatorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SMA => "SMA",
            Self::EMA => "EMA",
            Self::WMA => "WMA",
            Self::VWMA => "VWMA",
            Self::SMMA => "SMMA",
        }
    }

    pub fn all() -> &'static [BaselineEstimatorType] {
        &[Self::SMA, Self::EMA, Self::WMA, Self::VWMA, Self::SMMA]
    }
}

impl std::str::FromStr for BaselineEstimatorType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "SMA" => Ok(Self::SMA),
            "EMA" => Ok(Self::EMA),
            "WMA" => Ok(Self::WMA),
            "VWMA" => Ok(Self::VWMA),
            "SMMA" => Ok(Self::SMMA),
            _ => Err(format!("Unknown baseline estimator: {}", s)),
        }
    }
}

/// A single quote observation containing instantaneous spread and available liquidity depth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteObservation {
    pub timestamp_ms: u64,
    pub spread_bps: f64,
    pub top_depth_usd: f64,
}

impl QuoteObservation {
    pub fn new(timestamp_ms: u64, spread_bps: f64, top_depth_usd: f64) -> Self {
        Self {
            timestamp_ms,
            spread_bps,
            top_depth_usd: top_depth_usd.max(0.001),
        }
    }
}

/// Computes the Simple Moving Average (SMA) over the given slice of observations.
pub fn compute_sma(quotes: &[QuoteObservation]) -> f64 {
    if quotes.is_empty() {
        return 0.0;
    }
    let sum: f64 = quotes.iter().map(|q| q.spread_bps).sum();
    sum / quotes.len() as f64
}

/// Computes the Exponential Moving Average (EMA) with alpha = 2 / (period + 1).
pub fn compute_ema(quotes: &[QuoteObservation], period: usize) -> f64 {
    if quotes.is_empty() {
        return 0.0;
    }
    let period = period.max(1);
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut ema = quotes[0].spread_bps;
    for q in &quotes[1..] {
        ema = alpha * q.spread_bps + (1.0 - alpha) * ema;
    }
    ema
}

/// Computes the Linearly Weighted Moving Average (WMA).
/// Weight for item i (1-indexed from oldest to newest) is i.
pub fn compute_wma(quotes: &[QuoteObservation]) -> f64 {
    let n = quotes.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return quotes[0].spread_bps;
    }
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;
    for (i, q) in quotes.iter().enumerate() {
        let weight = (i + 1) as f64;
        weighted_sum += q.spread_bps * weight;
        total_weight += weight;
    }
    weighted_sum / total_weight
}

/// Computes the Volume / Depth-Weighted Moving Average (VWMA).
/// Each quote's spread is weighted by its top-of-book depth in USD.
/// This prevents phantom dust spreads ($10 depth) from distorting the baseline.
pub fn compute_vwma(quotes: &[QuoteObservation]) -> f64 {
    if quotes.is_empty() {
        return 0.0;
    }
    let mut weighted_sum = 0.0;
    let mut total_depth = 0.0;
    for q in quotes {
        let depth = q.top_depth_usd.max(0.001);
        weighted_sum += q.spread_bps * depth;
        total_depth += depth;
    }
    if total_depth > 0.0 {
        weighted_sum / total_depth
    } else {
        compute_sma(quotes)
    }
}

/// Computes the Smoothed Moving Average (SMMA / Wilder's MA) with alpha = 1 / period.
pub fn compute_smma(quotes: &[QuoteObservation], period: usize) -> f64 {
    if quotes.is_empty() {
        return 0.0;
    }
    let period = period.max(1);
    let alpha = 1.0 / period as f64;
    let mut smma = quotes[0].spread_bps;
    for q in &quotes[1..] {
        smma = alpha * q.spread_bps + (1.0 - alpha) * smma;
    }
    smma
}

/// Dispatches baseline calculation to the requested estimator.
pub fn estimate_baseline(
    quotes: &[QuoteObservation],
    estimator: BaselineEstimatorType,
    period: usize,
) -> f64 {
    match estimator {
        BaselineEstimatorType::SMA => compute_sma(quotes),
        BaselineEstimatorType::EMA => compute_ema(quotes, period),
        BaselineEstimatorType::WMA => compute_wma(quotes),
        BaselineEstimatorType::VWMA => compute_vwma(quotes),
        BaselineEstimatorType::SMMA => compute_smma(quotes, period),
    }
}

/// Result of evaluating an individual estimator candidate in Walk-Forward Optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatorEvaluation {
    pub estimator: BaselineEstimatorType,
    pub window_size: usize,
    pub in_sample_sharpe: f64,
    pub out_of_sample_sharpe: f64,
    pub tracking_error_bps: f64,
    pub simulated_net_profit_bps: f64,
    pub win_rate_pct: f64,
    pub total_trades: usize,
}

/// Comprehensive result from Walk-Forward Backtesting / Calibration Toolkit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfoOptimizationResult {
    pub optimal_estimator: BaselineEstimatorType,
    pub optimal_window_size: usize,
    pub in_sample_score: f64,
    pub out_of_sample_score: f64,
    pub candidate_evaluations: Vec<EstimatorEvaluation>,
    pub recommendation_notes: String,
}

/// Simulates trading a sequence of quotes given a chosen estimator and window size.
/// Evaluates simulated trades, net profit in BPS, tracking error, and Sharpe ratio.
pub fn simulate_estimator_performance(
    quotes: &[QuoteObservation],
    estimator: BaselineEstimatorType,
    window_size: usize,
    target_profit_bps: f64,
    friction_bps: f64,
) -> EstimatorEvaluation {
    let hurdle = target_profit_bps + friction_bps;
    let mut pnl_series: Vec<f64> = Vec::new();
    let mut errors: Vec<f64> = Vec::new();
    let mut wins = 0usize;
    let mut trades = 0usize;

    if quotes.len() <= window_size + 1 {
        return EstimatorEvaluation {
            estimator,
            window_size,
            in_sample_sharpe: 0.0,
            out_of_sample_sharpe: 0.0,
            tracking_error_bps: 0.0,
            simulated_net_profit_bps: 0.0,
            win_rate_pct: 0.0,
            total_trades: 0,
        };
    }

    let mut in_position = false;
    let mut entry_spread = 0.0;
    let mut _entry_baseline = 0.0;

    for i in window_size..quotes.len() {
        let window = &quotes[(i - window_size)..i];
        let baseline = estimate_baseline(window, estimator, window_size);
        let current_quote = &quotes[i];
        let current_spread = current_quote.spread_bps;

        let diff = current_spread - baseline;
        errors.push(diff);

        if !in_position {
            // Signal condition: Spread exceeds baseline by at least the nominal hurdle
            if diff >= hurdle {
                in_position = true;
                entry_spread = current_spread;
                _entry_baseline = baseline;
                trades += 1;
            }
        } else {
            // Exit condition: Mean reversion (spread converges back toward baseline or below)
            if current_spread <= baseline || current_spread < entry_spread * 0.5 {
                let gross_capture = entry_spread - current_spread;
                let net_trade_bps = gross_capture - friction_bps;
                pnl_series.push(net_trade_bps);
                if net_trade_bps > 0.0 {
                    wins += 1;
                }
                in_position = false;
            }
        }
    }

    // If still in position at the end of the test series, mark to market
    if in_position {
        let last_spread = quotes.last().map(|q| q.spread_bps).unwrap_or(entry_spread);
        let net_trade_bps = (entry_spread - last_spread) - friction_bps;
        pnl_series.push(net_trade_bps);
        if net_trade_bps > 0.0 {
            wins += 1;
        }
    }

    let total_profit: f64 = pnl_series.iter().sum();
    let win_rate = if trades > 0 {
        (wins as f64 / trades as f64) * 100.0
    } else {
        0.0
    };

    // Tracking error (standard deviation of spread difference)
    let mean_err: f64 = if !errors.is_empty() {
        errors.iter().sum::<f64>() / errors.len() as f64
    } else {
        0.0
    };
    let var_err: f64 = if errors.len() > 1 {
        errors.iter().map(|e| (e - mean_err).powi(2)).sum::<f64>() / (errors.len() - 1) as f64
    } else {
        1.0
    };
    let tracking_error = var_err.sqrt();

    // Sharpe ratio of simulated trade PnL
    let mean_pnl = if !pnl_series.is_empty() {
        total_profit / pnl_series.len() as f64
    } else {
        0.0
    };
    let std_pnl = if pnl_series.len() > 1 {
        let var: f64 = pnl_series
            .iter()
            .map(|p| (p - mean_pnl).powi(2))
            .sum::<f64>()
            / (pnl_series.len() - 1) as f64;
        var.sqrt().max(0.001)
    } else {
        1.0
    };
    let sharpe = if std_pnl > 0.0 {
        (mean_pnl / std_pnl) * (trades as f64).sqrt()
    } else {
        0.0
    };

    EstimatorEvaluation {
        estimator,
        window_size,
        in_sample_sharpe: sharpe,
        out_of_sample_sharpe: sharpe,
        tracking_error_bps: tracking_error,
        simulated_net_profit_bps: total_profit,
        win_rate_pct: win_rate,
        total_trades: trades,
    }
}

/// Runs Walk-Forward Optimization across quote history to find the optimal baseline estimator and window size.
pub fn run_walk_forward_optimization(
    history: &[QuoteObservation],
    candidate_windows: &[usize],
    target_profit_bps: f64,
    friction_bps: f64,
) -> WfoOptimizationResult {
    let total_len = history.len();
    if total_len < 40 {
        // Fallback default when data is minimal
        return WfoOptimizationResult {
            optimal_estimator: BaselineEstimatorType::VWMA,
            optimal_window_size: 50,
            in_sample_score: 0.0,
            out_of_sample_score: 0.0,
            candidate_evaluations: vec![],
            recommendation_notes: "Insufficient quote updates for statistical WFO (<40 ticks). Defaulting to depth-weighted VWMA(50); scores are unmeasured.".into(),
        };
    }

    // Split into 60% In-Sample (Calibration) and 40% Out-of-Sample (Validation)
    let split_idx = (total_len as f64 * 0.6).round() as usize;
    let in_sample = &history[..split_idx];
    let out_of_sample = &history[split_idx..];

    let mut evaluations: Vec<EstimatorEvaluation> = Vec::new();
    let estimators = BaselineEstimatorType::all();

    for &w in candidate_windows {
        if w >= in_sample.len() / 2 {
            continue;
        }
        for &est in estimators {
            let eval_is =
                simulate_estimator_performance(in_sample, est, w, target_profit_bps, friction_bps);
            let eval_oos = simulate_estimator_performance(
                out_of_sample,
                est,
                w,
                target_profit_bps,
                friction_bps,
            );

            evaluations.push(EstimatorEvaluation {
                estimator: est,
                window_size: w,
                in_sample_sharpe: eval_is.in_sample_sharpe,
                out_of_sample_sharpe: eval_oos.out_of_sample_sharpe,
                tracking_error_bps: eval_oos.tracking_error_bps,
                simulated_net_profit_bps: eval_oos.simulated_net_profit_bps,
                win_rate_pct: eval_oos.win_rate_pct,
                total_trades: eval_oos.total_trades,
            });
        }
    }

    // Score based on Out-of-Sample Sharpe, Net Profit, and minimum tracking error penalty
    evaluations.sort_by(|a, b| {
        let score_a = a.out_of_sample_sharpe * 0.5 + (a.simulated_net_profit_bps * 0.1)
            - (a.tracking_error_bps * 0.05);
        let score_b = b.out_of_sample_sharpe * 0.5 + (b.simulated_net_profit_bps * 0.1)
            - (b.tracking_error_bps * 0.05);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some(best) = evaluations.first().cloned() else {
        return WfoOptimizationResult {
            optimal_estimator: BaselineEstimatorType::VWMA,
            optimal_window_size: 50,
            in_sample_score: 0.0,
            out_of_sample_score: 0.0,
            candidate_evaluations: vec![],
            recommendation_notes:
                "No estimator/window candidate was evaluable on the in-sample split; widen the \
                 candidate windows or collect more history."
                    .into(),
        };
    };

    let notes = format!(
        "Optimal estimator selected: {} with window of {} ticks. Out-of-sample Sharpe: {:.2}, Net captured: +{:.1} BPS across {} trades (Win rate: {:.1}%).",
        best.estimator.as_str(),
        best.window_size,
        best.out_of_sample_sharpe,
        best.simulated_net_profit_bps,
        best.total_trades,
        best.win_rate_pct
    );

    WfoOptimizationResult {
        optimal_estimator: best.estimator,
        optimal_window_size: best.window_size,
        in_sample_score: best.in_sample_sharpe,
        out_of_sample_score: best.out_of_sample_sharpe,
        candidate_evaluations: evaluations,
        recommendation_notes: notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_mock_quotes(n: usize, base_spread: f64, depth: f64) -> Vec<QuoteObservation> {
        (0..n)
            .map(|i| {
                let noise = ((i % 5) as f64 - 2.0) * 1.5;
                QuoteObservation::new((i * 1000) as u64, base_spread + noise, depth)
            })
            .collect()
    }

    #[test]
    fn test_baseline_estimator_from_str() {
        assert_eq!(
            "SMA".parse::<BaselineEstimatorType>().unwrap(),
            BaselineEstimatorType::SMA
        );
        assert_eq!(
            "ema".parse::<BaselineEstimatorType>().unwrap(),
            BaselineEstimatorType::EMA
        );
        assert_eq!(
            "Vwma".parse::<BaselineEstimatorType>().unwrap(),
            BaselineEstimatorType::VWMA
        );
        assert_eq!(
            "WMA".parse::<BaselineEstimatorType>().unwrap(),
            BaselineEstimatorType::WMA
        );
        assert_eq!(
            "SMMA".parse::<BaselineEstimatorType>().unwrap(),
            BaselineEstimatorType::SMMA
        );
    }

    #[test]
    fn test_baseline_estimator_from_str_invalid() {
        let err = "bogus".parse::<BaselineEstimatorType>().unwrap_err();
        assert_eq!(err, "Unknown baseline estimator: bogus");
    }

    #[test]
    fn test_sma_calculation() {
        let quotes = vec![
            QuoteObservation::new(1, 10.0, 1000.0),
            QuoteObservation::new(2, 20.0, 1000.0),
            QuoteObservation::new(3, 30.0, 1000.0),
        ];
        assert_eq!(compute_sma(&quotes), 20.0);
    }

    #[test]
    fn test_wma_calculation() {
        let quotes = vec![
            QuoteObservation::new(1, 10.0, 1000.0), // weight 1
            QuoteObservation::new(2, 20.0, 1000.0), // weight 2
            QuoteObservation::new(3, 30.0, 1000.0), // weight 3
        ];
        // (10*1 + 20*2 + 30*3) / (1+2+3) = (10 + 40 + 90) / 6 = 140 / 6 = 23.3333
        let wma = compute_wma(&quotes);
        assert!((wma - 23.3333).abs() < 0.001);
    }

    #[test]
    fn test_vwma_filters_phantom_dust_spreads() {
        let quotes = vec![
            QuoteObservation::new(1, 10.0, 500_000.0), // Massive legitimate depth
            QuoteObservation::new(2, 10.0, 500_000.0), // Massive legitimate depth
            QuoteObservation::new(3, 100.0, 10.0),     // $10 dust phantom spike
        ];
        let sma = compute_sma(&quotes); // (10 + 10 + 100) / 3 = 40.0 BPS (heavily distorted!)
        let vwma = compute_vwma(&quotes); // Heavily anchored to legitimate 10 BPS depth

        assert_eq!(sma, 40.0);
        assert!(
            vwma < 10.01,
            "VWMA must filter out dust depth: was {}",
            vwma
        );
    }

    #[test]
    fn test_ema_and_smma_calculation() {
        let quotes = generate_mock_quotes(20, 15.0, 5000.0);
        let ema = compute_ema(&quotes, 10);
        let smma = compute_smma(&quotes, 10);

        assert!(ema > 10.0 && ema < 20.0);
        assert!(smma > 10.0 && smma < 20.0);
    }

    #[test]
    fn test_walk_forward_optimization_selection() {
        let mut quotes = generate_mock_quotes(80, 12.0, 5000.0);
        // Inject periodic dislocations that mean-revert
        for i in (30..80).step_by(10) {
            quotes[i].spread_bps = 25.0; // Dislocation
            quotes[i + 1].spread_bps = 12.0; // Mean reversion
        }

        let wfo = run_walk_forward_optimization(&quotes, &[10, 20], 5.0, 3.5);
        assert!(!wfo.candidate_evaluations.is_empty());
        assert!([
            BaselineEstimatorType::SMA,
            BaselineEstimatorType::EMA,
            BaselineEstimatorType::WMA,
            BaselineEstimatorType::VWMA,
            BaselineEstimatorType::SMMA
        ]
        .contains(&wfo.optimal_estimator));
        assert!(wfo.optimal_window_size == 10 || wfo.optimal_window_size == 20);
        assert!(!wfo.recommendation_notes.is_empty());
    }
}
